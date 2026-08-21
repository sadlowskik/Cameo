//! Userspace MoE expert offload.
//!
//! Decides which experts live in VRAM vs. host RAM from a model's footprint and
//! usable VRAM. Placement calls this; llama.cpp then gets `--override-tensor
//! exps=CPU` (and layer/KV host offload when even the non-expert weights do not
//! fit). Kernel-level placement is still deferred until this path is measured
//! as a real bottleneck.

/// Fraction of an MoE's weights that live in the experts (and are the prime
/// offload target). Same placeholder as `cameo-placement::model`.
const EXPERT_FRACTION_NOTE: &str = "experts";

/// How many transformer layers live on the GPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuLayers {
    /// All layers resident on GPU(s).
    All,
    /// Only this many layers on GPU; the rest run on the host.
    Count(u32),
}

/// Inputs the MoE planner needs. Callers own model metadata; this crate stays
/// free of placement types so it can be tested in isolation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OffloadRequest {
    /// Weights + KV if everything stayed on the GPU.
    pub total_bytes: u64,
    /// Weight bytes only (no KV).
    pub weights_bytes: u64,
    /// Bytes that can leave VRAM without hurting the latency-critical path
    /// (expert tensors). Zero for a dense model — do not call this planner.
    pub offloadable_expert_bytes: u64,
    pub n_layers: u32,
    /// Usable GPU-local bytes after headroom (and GTT on an APU).
    pub usable_vram_bytes: u64,
}

/// Where the experts, layers, and KV cache should live.
#[derive(Debug, Clone, PartialEq)]
pub struct OffloadPlan {
    pub gpu_layers: GpuLayers,
    /// MoE experts kept in host RAM, streamed on demand.
    pub experts_on_host: bool,
    /// KV cache kept in host RAM (last resort; hurts latency).
    pub kv_on_host: bool,
    /// Human-readable explanation of the decision.
    pub notes: Vec<String>,
}

/// Plan expert (and, if still tight, layer/KV) offload for a model that does
/// not fit in VRAM as a whole.
///
/// Callers that already fit in VRAM should not call this — experts stay on the
/// GPU so expert-parallel multi-GPU can still fire.
pub fn plan_offload(req: &OffloadRequest) -> OffloadPlan {
    let mut notes = Vec::new();
    let expert = req.offloadable_expert_bytes;
    let resident = req.total_bytes.saturating_sub(expert);

    if resident <= req.usable_vram_bytes {
        notes.push(format!(
            "MoE {EXPERT_FRACTION_NOTE} (~{:.1} GiB) offloaded to host RAM; ~{:.1} GiB stays resident.",
            gib(expert),
            gib(resident)
        ));
        return OffloadPlan {
            gpu_layers: GpuLayers::All,
            experts_on_host: true,
            kv_on_host: false,
            notes,
        };
    }

    // Experts and KV are on host, so only the non-expert weights stay
    // resident — size GPU layers against those, not the full weights.
    let resident_weights = req.weights_bytes.saturating_sub(expert);
    let layers = estimate_gpu_layers(resident_weights, req.n_layers, req.usable_vram_bytes);
    notes.push(
        "MoE model tight even with experts on host; also offloading layers/KV — expect lower throughput."
            .to_string(),
    );
    OffloadPlan {
        gpu_layers: GpuLayers::Count(layers),
        experts_on_host: true,
        kv_on_host: true,
        notes,
    }
}

fn estimate_gpu_layers(resident_bytes: u64, n_layers: u32, usable: u64) -> u32 {
    let layers = n_layers.max(1);
    let per_layer = resident_bytes as f64 / layers as f64;
    if per_layer <= 0.0 {
        return layers;
    }
    ((usable as f64 / per_layer).floor() as u32).min(layers)
}

fn gib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0 * 1024.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1024 * 1024 * 1024;

    fn req(total: u64, weights: u64, experts: u64, layers: u32, usable: u64) -> OffloadRequest {
        OffloadRequest {
            total_bytes: total,
            weights_bytes: weights,
            offloadable_expert_bytes: experts,
            n_layers: layers,
            usable_vram_bytes: usable,
        }
    }

    #[test]
    fn experts_on_host_when_non_expert_working_set_fits() {
        // ~20 GiB total, 16 GiB experts, ~14.4 GiB usable (16 GiB card @ 90%).
        let p = plan_offload(&req(
            20 * GIB,
            18 * GIB,
            16 * GIB,
            40,
            (16.0 * 0.90 * GIB as f64) as u64,
        ));
        assert!(p.experts_on_host);
        assert!(!p.kv_on_host);
        assert_eq!(p.gpu_layers, GpuLayers::All);
        assert!(p.notes.iter().any(|n| n.contains("offloaded to host RAM")));
    }

    #[test]
    fn tight_moe_offloads_layers_and_kv_sized_from_resident_weights() {
        // Non-expert remainder still exceeds VRAM: layers must be counted from
        // the ~10% resident weights, not the full footprint.
        let usable = (16.0 * 0.90 * GIB as f64) as u64;
        let p = plan_offload(&req(200 * GIB, 190 * GIB, 180 * GIB, 80, usable));
        assert!(p.experts_on_host);
        assert!(p.kv_on_host);
        match p.gpu_layers {
            GpuLayers::Count(n) => assert!(n > 30, "expected many resident layers, got {n}"),
            other => panic!("expected Count, got {other:?}"),
        }
    }

    #[test]
    fn dense_caller_passing_zero_experts_still_offloads_layers() {
        // A misplaced dense call: no offloadable experts, so resident == total
        // and we fall into the tight branch.
        let p = plan_offload(&req(40 * GIB, 36 * GIB, 0, 80, 8 * GIB));
        assert!(p.experts_on_host);
        assert!(p.kv_on_host);
        assert!(matches!(p.gpu_layers, GpuLayers::Count(_)));
    }
}
