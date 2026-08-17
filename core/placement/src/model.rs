//! Model metadata and (approximate) memory estimation.
//!
//! The constants here are deliberately coarse first-order estimates — enough to
//! drive placement *decisions*, not to predict bytes exactly. They are the main
//! thing Phase 1 measurements will calibrate. Every one is overridable via the
//! explicit `ModelMeta` fields.

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

    /// Estimated resident bytes of the weights.
    pub fn weights_bytes(&self) -> u64 {
        let bits = self.params_b * 1e9 * self.quant.bits_per_weight();
        (bits / 8.0) as u64
    }

    /// Bytes that can be offloaded to host RAM without hurting latency-critical
    /// paths: MoE experts if this is an MoE model, else 0 (dense models offload
    /// by whole layers, handled in the planner).
    pub fn offloadable_expert_bytes(&self) -> u64 {
        if self.is_moe {
            (self.weights_bytes() as f64 * MOE_EXPERT_PARAM_FRACTION) as u64
        } else {
            0
        }
    }

    /// Estimated KV-cache bytes for this model's context.
    pub fn kv_bytes(&self) -> u64 {
        self.n_layers as u64 * self.context_len as u64 * KV_BYTES_PER_LAYER_PER_TOKEN
    }

    /// Total resident bytes if everything is on the GPU (weights + KV).
    pub fn total_bytes(&self) -> u64 {
        self.weights_bytes() + self.kv_bytes()
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
}
