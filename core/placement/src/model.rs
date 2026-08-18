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
        if self.n_layers == 0 || self.n_layers > MAX_LAYERS {
            return Err(Error::InvalidModel(format!(
                "layer count must be between 1 and {MAX_LAYERS}, got {}",
                self.n_layers
            )));
        }
        Ok(())
    }

    /// Estimated resident bytes of the weights.
    pub fn weights_bytes(&self) -> u64 {
        bytes_from_f64(self.params_b * 1e9 * self.quant.bits_per_weight() / 8.0)
    }

    /// Bytes that can be offloaded to host RAM without hurting latency-critical
    /// paths: MoE experts if this is an MoE model, else 0 (dense models offload
    /// by whole layers, handled in the planner).
    pub fn offloadable_expert_bytes(&self) -> u64 {
        if self.is_moe {
            bytes_from_f64(self.weights_bytes() as f64 * MOE_EXPERT_PARAM_FRACTION)
        } else {
            0
        }
    }

    /// Estimated KV-cache bytes for this model's context.
    pub fn kv_bytes(&self) -> u64 {
        (self.n_layers as u64)
            .saturating_mul(self.context_len as u64)
            .saturating_mul(KV_BYTES_PER_LAYER_PER_TOKEN)
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
}
