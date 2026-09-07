//! Model metadata and (approximate) memory estimation.
//!
//! The constants here are deliberately coarse first-order estimates — enough to
//! drive placement *decisions*, not to predict bytes exactly. They are the main
//! thing Phase 1 measurements will calibrate. Every one is overridable via the
//! explicit `ModelMeta` fields.
//!
//! All arithmetic saturates. These numbers come from user-supplied flags, and a
//! description that overflows should be rejected by [`ModelMeta::validate`] with
//! a message — not panic in a debug build and silently wrap in a release one.

use crate::error::Error;
use serde::{Deserialize, Serialize};

/// Quantization level, with its effective bits-per-weight. Variant names follow
/// llama.cpp's GGUF naming (e.g. `Q4_K_M`) rather than Rust camel-case.
#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QuantLevel {
    F16,
    Q8_0,
    Q6_K,
    Q5_K_M,
    Q4_K_M,
    Q4_0,
}

impl QuantLevel {
    /// Effective bits per weight (GGUF k-quants include block overhead).
    /// PLACEHOLDER values — confirm against real GGUF file sizes in Phase 1.
    pub fn bits_per_weight(self) -> f64 {
        match self {
            QuantLevel::F16 => 16.0,
            QuantLevel::Q8_0 => 8.5,
            QuantLevel::Q6_K => 6.56,
            QuantLevel::Q5_K_M => 5.5,
            QuantLevel::Q4_K_M => 4.85,
            QuantLevel::Q4_0 => 4.5,
        }
    }

    /// Parse a llama.cpp-style level name (case-insensitive).
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_uppercase().as_str() {
            "F16" | "FP16" => Some(QuantLevel::F16),
            "Q8_0" | "Q8" => Some(QuantLevel::Q8_0),
            "Q6_K" | "Q6" => Some(QuantLevel::Q6_K),
            "Q5_K_M" | "Q5" => Some(QuantLevel::Q5_K_M),
            "Q4_K_M" | "Q4" => Some(QuantLevel::Q4_K_M),
            "Q4_0" => Some(QuantLevel::Q4_0),
            _ => None,
        }
    }
}

/// KV-cache bytes per layer per token. PLACEHOLDER (~GQA with 8 KV heads, head
/// dim 128, fp16): 2 (K+V) * 8 * 128 * 2 B ≈ 4 KiB. Calibrate in Phase 1.
const KV_BYTES_PER_LAYER_PER_TOKEN: u64 = 4096;

/// Fraction of an MoE model's weights that live in the experts (and are thus the
/// prime offload target). PLACEHOLDER — most MoE params are in experts.
const MOE_EXPERT_PARAM_FRACTION: f64 = 0.9;

/// Largest parameter count (in billions) that is a plausible model rather than a
/// typo or an overflow probe. Frontier models are ~2 orders of magnitude below.
const MAX_PARAMS_B: f64 = 100_000.0;

/// Largest context the planner will size a KV cache for.
const MAX_CONTEXT_LEN: u32 = 1 << 24;

/// Largest transformer depth the planner will accept.
const MAX_LAYERS: u32 = 4096;

/// What we know about a model, for planning purposes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelMeta {
    pub name: String,
    /// Total parameters, in billions.
    pub params_b: f64,
    pub quant: QuantLevel,
    pub is_moe: bool,
    /// Transformer layers (for KV-cache sizing). Defaulted when unknown.
    pub n_layers: u32,
    /// Context length to plan the KV cache for.
    pub context_len: u32,
    /// Model-native context from GGUF/Hugging Face metadata when known. The
    /// serving allocation is clamped to 80% of this value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_context_len: Option<u32>,
    /// Concurrent llama-server slots. `context_len` is per slot; placement
    /// accounts for the full physical KV pool.
    #[serde(default = "default_parallel_slots")]
    pub parallel_slots: u16,
    /// Exact GGUF file bytes when a local model was resolved. This is a much
    /// better resident-weight estimate than parameter-count quantization math.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weights_bytes_override: Option<u64>,
    /// Structural KV metadata from GGUF/model config when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kv_heads: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_dim: Option<u32>,
    /// Bytes per K/V element (2 for fp16, 1 for q8 cache).
    #[serde(default = "default_kv_element_bytes")]
    pub kv_element_bytes: u8,
    /// Exact llama.cpp KV cache format. When set this supersedes the legacy
    /// whole-byte width above and permits honest q4 accounting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kv_cache_type: Option<KvCacheType>,
    #[serde(default = "default_batch_size")]
    pub batch_size: u32,
    #[serde(default = "default_ubatch_size")]
    pub ubatch_size: u32,
    #[serde(default)]
    pub flash_attention: bool,
    #[serde(default)]
    pub cache_reuse: u32,
    #[serde(default)]
    pub cache_ram_mib: u32,
    #[serde(default)]
    pub metrics: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot_save_path: Option<String>,
    /// Measured/model-specific expert fraction when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expert_param_fraction: Option<f64>,
}

fn default_kv_element_bytes() -> u8 {
    2
}

fn default_parallel_slots() -> u16 {
    1
}
fn default_batch_size() -> u32 {
    2048
}
fn default_ubatch_size() -> u32 {
    512
}

/// llama.cpp KV cache storage formats. The value is passed directly to
/// `--cache-type-k/v`; the bit width is also used by placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KvCacheType {
    F16,
    Bf16,
    Q8_0,
    Q4_0,
}

impl KvCacheType {
    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "f16" => Some(Self::F16),
            "bf16" => Some(Self::Bf16),
            "q8_0" => Some(Self::Q8_0),
            "q4_0" => Some(Self::Q4_0),
            _ => None,
        }
    }

    pub fn as_llama(self) -> &'static str {
        match self {
            Self::F16 => "f16",
            Self::Bf16 => "bf16",
            Self::Q8_0 => "q8_0",
            Self::Q4_0 => "q4_0",
        }
    }

    fn bits(self) -> u64 {
        match self {
            Self::F16 | Self::Bf16 => 16,
            Self::Q8_0 => 8,
            Self::Q4_0 => 4,
        }
    }
}

impl ModelMeta {
    /// A dense model with sane structural defaults.
    pub fn dense(name: impl Into<String>, params_b: f64, quant: QuantLevel) -> Self {
        Self {
            name: name.into(),
            params_b,
            quant,
            is_moe: false,
            n_layers: default_layers(params_b),
            context_len: 4096,
            native_context_len: None,
            parallel_slots: default_parallel_slots(),
            weights_bytes_override: None,
            kv_heads: None,
            head_dim: None,
            kv_element_bytes: default_kv_element_bytes(),
            kv_cache_type: None,
            batch_size: default_batch_size(),
            ubatch_size: default_ubatch_size(),
            flash_attention: false,
            cache_reuse: 0,
            cache_ram_mib: 0,
            metrics: false,
            slot_save_path: None,
            expert_param_fraction: None,
        }
    }

    /// An MoE model with sane structural defaults.
    pub fn moe(name: impl Into<String>, params_b: f64, quant: QuantLevel) -> Self {
        Self {
            is_moe: true,
            ..Self::dense(name, params_b, quant)
        }
    }

    /// Reject a description the planner cannot reason about.
    ///
    /// `--params inf`, `--params nan` and `--params -5` all used to sail through
    /// into the memory math, where they produced an 18-exabyte model that
    /// reported "fits VRAM: yes". Every planning entry point calls this first.
    pub fn validate(&self) -> Result<(), Error> {
        if !self.params_b.is_finite() {
            return Err(Error::InvalidModel(format!(
                "parameter count must be a finite number, got {}",
                self.params_b
            )));
        }
        if self.params_b <= 0.0 {
            return Err(Error::InvalidModel(format!(
                "parameter count must be greater than zero, got {}",
                self.params_b
            )));
        }
        if self.params_b > MAX_PARAMS_B {
            return Err(Error::InvalidModel(format!(
                "parameter count {} B exceeds the {MAX_PARAMS_B} B ceiling; \
                 pass the size in billions (a 7B model is `--params 7`)",
                self.params_b
            )));
        }
        if self.context_len == 0 || self.context_len > MAX_CONTEXT_LEN {
            return Err(Error::InvalidModel(format!(
                "context length must be between 1 and {MAX_CONTEXT_LEN}, got {}",
                self.context_len
            )));
        }
        if self.parallel_slots == 0 {
            return Err(Error::InvalidModel(
                "parallel slot count must be at least 1".into(),
            ));
        }
        if self
            .native_context_len
            .is_some_and(|n| n == 0 || n > MAX_CONTEXT_LEN)
        {
            return Err(Error::InvalidModel(format!(
                "native context length must be between 1 and {MAX_CONTEXT_LEN}"
            )));
        }
        if self.batch_size == 0 || self.ubatch_size == 0 || self.ubatch_size > self.batch_size {
            return Err(Error::InvalidModel(format!(
                "batch sizes require 1 <= ubatch ({}) <= batch ({})",
                self.ubatch_size, self.batch_size
            )));
        }
        if self.n_layers == 0 || self.n_layers > MAX_LAYERS {
            return Err(Error::InvalidModel(format!(
                "layer count must be between 1 and {MAX_LAYERS}, got {}",
                self.n_layers
            )));
        }
        if self.kv_element_bytes == 0 || self.kv_element_bytes > 8 {
            return Err(Error::InvalidModel(format!(
                "KV element width must be between 1 and 8 bytes, got {}",
                self.kv_element_bytes
            )));
        }
        if self
            .expert_param_fraction
            .is_some_and(|fraction| !fraction.is_finite() || !(0.0..=1.0).contains(&fraction))
        {
            return Err(Error::InvalidModel(
                "expert parameter fraction must be between 0 and 1".into(),
            ));
        }
        Ok(())
    }

    /// Enrich estimates from a resolved local model. File size is exact; KV
    /// structure can be supplied by a GGUF/config inspector when available.
    pub fn with_file_size(mut self, path: &std::path::Path) -> Self {
        self.weights_bytes_override = std::fs::metadata(path).ok().map(|meta| meta.len());
        self
    }

    /// Estimated resident bytes of the weights.
    pub fn weights_bytes(&self) -> u64 {
        self.weights_bytes_override.unwrap_or_else(|| {
            bytes_from_f64(self.params_b * 1e9 * self.quant.bits_per_weight() / 8.0)
        })
    }

    /// Bytes that can be offloaded to host RAM without hurting latency-critical
    /// paths: MoE experts if this is an MoE model, else 0 (dense models offload
    /// by whole layers, handled in the planner).
    pub fn offloadable_expert_bytes(&self) -> u64 {
        if self.is_moe {
            bytes_from_f64(
                self.weights_bytes() as f64
                    * self
                        .expert_param_fraction
                        .unwrap_or(MOE_EXPERT_PARAM_FRACTION),
            )
        } else {
            0
        }
    }

    /// Estimated KV-cache bytes for this model's context.
    pub fn kv_bytes(&self) -> u64 {
        let per_layer_token = match (self.kv_heads, self.head_dim, self.kv_cache_type) {
            (Some(heads), Some(dim), Some(cache)) => {
                2u64.saturating_mul(heads as u64)
                    .saturating_mul(dim as u64)
                    .saturating_mul(cache.bits())
                    .saturating_add(7)
                    / 8
            }
            (Some(heads), Some(dim), None) => 2u64
                .saturating_mul(heads as u64)
                .saturating_mul(dim as u64)
                .saturating_mul(self.kv_element_bytes as u64),
            (_, _, Some(cache)) => KV_BYTES_PER_LAYER_PER_TOKEN.saturating_mul(cache.bits()) / 16,
            _ => KV_BYTES_PER_LAYER_PER_TOKEN,
        };
        (self.n_layers as u64)
            .saturating_mul(self.context_len as u64)
            .saturating_mul(self.parallel_slots as u64)
            .saturating_mul(per_layer_token)
    }

    /// Enforce the policy ceiling when native model metadata is available.
    pub fn clamp_to_native_context(&mut self) {
        if let Some(native) = self.native_context_len {
            let safe = native.saturating_mul(80) / 100;
            self.context_len = self.context_len.min(safe.max(1));
        }
    }

    /// Total resident bytes if everything is on the GPU (weights + KV).
    pub fn total_bytes(&self) -> u64 {
        self.weights_bytes().saturating_add(self.kv_bytes())
    }
}

/// Clamp a byte estimate into `u64`.
///
/// The two infinities are not symmetric. `+inf` means "immeasurably large" and
/// must saturate *up*: mapping it to 0 would report an infinite model as fitting
/// in VRAM, which is the failure this whole function exists to prevent. NaN and
/// anything at or below zero carry no size information and become 0, where the
/// planner's own checks catch them.
fn bytes_from_f64(v: f64) -> u64 {
    if v.is_nan() || v <= 0.0 {
        0
    } else if v >= u64::MAX as f64 {
        u64::MAX
    } else {
        v as u64
    }
}

/// Rough layer count from parameter scale, used only when a real value is absent.
fn default_layers(params_b: f64) -> u32 {
    match params_b {
        p if p < 4.0 => 26,
        p if p < 10.0 => 32,
        p if p < 40.0 => 40,
        p if p < 90.0 => 80,
        _ => 96,
    }
}

/// Convert bytes to GiB for display.
pub fn gib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0 * 1024.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dense_7b_q4_is_about_4gb() {
        let m = ModelMeta::dense("llama-7b", 7.0, QuantLevel::Q4_K_M);
        let gb = gib(m.weights_bytes());
        assert!((3.5..5.0).contains(&gb), "got {gb} GiB");
    }

    #[test]
    fn moe_has_offloadable_experts_dense_does_not() {
        let moe = ModelMeta::moe("mixtral", 47.0, QuantLevel::Q4_K_M);
        let dense = ModelMeta::dense("llama-70b", 70.0, QuantLevel::Q4_K_M);
        assert!(moe.offloadable_expert_bytes() > 0);
        assert_eq!(dense.offloadable_expert_bytes(), 0);
    }

    #[test]
    fn quant_parse_roundtrips_common_names() {
        assert_eq!(QuantLevel::parse("q4_k_m"), Some(QuantLevel::Q4_K_M));
        assert_eq!(QuantLevel::parse("Q8_0"), Some(QuantLevel::Q8_0));
        assert_eq!(QuantLevel::parse("bogus"), None);
    }

    #[test]
    fn non_finite_and_negative_params_are_rejected() {
        for bad in [f64::INFINITY, f64::NEG_INFINITY, f64::NAN, -5.0, 0.0] {
            let m = ModelMeta::dense("x", bad, QuantLevel::Q4_K_M);
            assert!(
                matches!(m.validate(), Err(Error::InvalidModel(_))),
                "{bad} should be rejected"
            );
        }
    }

    #[test]
    fn absurd_but_finite_params_are_rejected() {
        let m = ModelMeta::dense("x", 1e12, QuantLevel::Q4_K_M);
        assert!(matches!(m.validate(), Err(Error::InvalidModel(_))));
    }

    #[test]
    fn memory_math_saturates_instead_of_wrapping() {
        // Even for a description that never reaches `validate`, the arithmetic
        // must not overflow: in a debug build that is a panic, and in a release
        // build it wraps to a small number that reports "fits VRAM: yes".
        let mut m = ModelMeta::dense("x", f64::MAX, QuantLevel::F16);
        m.n_layers = u32::MAX;
        m.context_len = u32::MAX;
        assert_eq!(m.weights_bytes(), u64::MAX);
        assert_eq!(m.kv_bytes(), u64::MAX);
        assert_eq!(m.total_bytes(), u64::MAX);

        let nan = ModelMeta::dense("x", f64::NAN, QuantLevel::F16);
        assert_eq!(nan.weights_bytes(), 0);
    }

    #[test]
    fn sane_description_validates() {
        assert!(ModelMeta::dense("llama-7b", 7.0, QuantLevel::Q4_K_M)
            .validate()
            .is_ok());
    }

    #[test]
    fn kv_cache_accounts_for_format_and_parallel_slots() {
        let mut m = ModelMeta::dense("qwen", 14.0, QuantLevel::Q4_K_M);
        m.n_layers = 48;
        m.context_len = 8_192;
        m.kv_heads = Some(8);
        m.head_dim = Some(128);
        m.kv_cache_type = Some(KvCacheType::Q8_0);
        let one = m.kv_bytes();
        m.parallel_slots = 3;
        assert_eq!(m.kv_bytes(), one * 3);
        m.kv_cache_type = Some(KvCacheType::Q4_0);
        assert_eq!(m.kv_bytes(), one * 3 / 2);
    }

    #[test]
    fn native_context_is_capped_at_eighty_percent() {
        let mut m = ModelMeta::dense("qwen", 14.0, QuantLevel::Q4_K_M);
        m.native_context_len = Some(32_768);
        m.context_len = 32_768;
        m.clamp_to_native_context();
        assert_eq!(m.context_len, 26_214);
    }
}
