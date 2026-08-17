//! The planner: `(topology x model x task x settings) -> PlacementPlan`.
//!
//! This is pure decision logic. It never touches hardware — it decides *what*
//! should run where, and the backend command builders turn that decision into an
//! actual command line. The memory math uses the coarse estimates in [`crate::model`];
//! the *structure* of the decisions is what matters and is unit-tested here.

use crate::error::Error;
use crate::model::{gib, ModelMeta};
use cameo_config::{Backend, Settings};
use cameo_gpu_detect::{LinkKind, TierAssessment, Topology};
use serde::{Deserialize, Serialize};

/// Fraction of VRAM we plan to actually use (leave headroom for fragmentation,
/// driver, and runtime growth).
pub(crate) const VRAM_HEADROOM: f64 = 0.90;

/// Very rough training-footprint multiplier over weight bytes (fp32 Adam states
/// + grads + activations). PLACEHOLDER — calibrate in Phase 1.
pub(crate) const TRAINING_FOOTPRINT_MULT: u64 = 4;

/// What the model is being run for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Task {
    Inference,
    Training,
}

/// How work is spread across multiple GPUs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MultiGpu {
    /// One GPU only.
    Single,
    /// Dense model split across cards by VRAM proportion.
    LayerSplit { fractions: Vec<f32> },
    /// MoE experts distributed across cards.
    ExpertParallel,
    /// Training sharded across cards (FSDP / ZeRO).
    Fsdp { shards: usize },
}

/// How many transformer layers live on the GPU.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuLayers {
    /// All layers resident on GPU(s).
    All,
    /// Only this many layers on GPU; the rest run on the host.
    Count(u32),
}

/// What gets pushed off the GPU into host RAM.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Offload {
    pub gpu_layers: GpuLayers,
    /// MoE experts kept in host RAM, streamed on demand.
    pub experts_on_host: bool,
    /// KV cache kept in host RAM (last resort; hurts latency).
    pub kv_on_host: bool,
}

/// The concrete plan the backend builders consume.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlacementPlan {
    pub task: Task,
    pub backend: Backend,
    /// Number of GPUs this plan uses.
    pub gpu_count: usize,
    pub multi_gpu: MultiGpu,
    pub offload: Offload,
    /// Environment variables to set (e.g. `HSA_OVERRIDE_GFX_VERSION`).
    pub env: Vec<(String, String)>,
    /// Whether the working set is estimated to fit in VRAM without host offload.
    pub fits_in_vram: bool,
    /// Human-readable explanation of every decision.
    pub notes: Vec<String>,
}

/// Produce a placement plan. `assessments` must be parallel to `topo.gpus`.
pub fn plan(
    topo: &Topology,
    assessments: &[TierAssessment],
    model: &ModelMeta,
    task: Task,
    settings: &Settings,
) -> Result<PlacementPlan, Error> {
    if topo.gpus.is_empty() || assessments.is_empty() {
        return Err(Error::NoGpus);
    }
    let top = &assessments[0];

    let backend = resolve_backend(settings.backend, top, task)?;

    let mut env = Vec::new();
    if let Some(h) = settings
        .hsa_override
        .clone()
        .or_else(|| top.hsa_override.clone())
    {
        env.push(("HSA_OVERRIDE_GFX_VERSION".to_string(), h));
    }

    let vram_known = topo.gpus.iter().all(|g| g.vram_mb.is_some());
    let total_vram_bytes: u64 = topo
        .gpus
        .iter()
        .filter_map(|g| g.vram_mb)
        .map(|mb| mb * 1024 * 1024)
        .sum();
    let usable = (total_vram_bytes as f64 * VRAM_HEADROOM) as u64;

    let mut notes = Vec::new();
    if !vram_known {
        notes.push("VRAM unknown for at least one GPU; planning conservatively.".to_string());
    }
    if assessments.iter().any(|a| a.tier != top.tier) {
        notes.push("Mixed GPU tiers detected; planning to the top GPU's tier.".to_string());
    }

    match task {
        Task::Inference => plan_inference(topo, model, backend, env, usable, vram_known, notes),
        Task::Training => plan_training(topo, model, backend, env, usable, vram_known, notes),
    }
}

fn resolve_backend(
    override_backend: Option<Backend>,
    top: &TierAssessment,
    task: Task,
) -> Result<Backend, Error> {
    match task {
        Task::Training => {
            if !top.training_supported {
                return Err(Error::TrainingUnsupported(top.tier.as_number()));
            }
            Ok(Backend::Rocm)
        }
        Task::Inference => Ok(match override_backend {
            Some(Backend::Vulkan) => Backend::Vulkan,
            Some(Backend::Rocm) => Backend::Rocm,
            _ if top.training_supported => Backend::Rocm, // Tier 1/2
            _ => Backend::Vulkan,                         // Tier 3
        }),
    }
}

#[allow(clippy::too_many_arguments)]
fn plan_inference(
    topo: &Topology,
    model: &ModelMeta,
    backend: Backend,
    env: Vec<(String, String)>,
    usable: u64,
    vram_known: bool,
    mut notes: Vec<String>,
) -> Result<PlacementPlan, Error> {
    let need = model.total_bytes();
    let fits = vram_known && need <= usable;
    let mut offload = Offload {
        gpu_layers: GpuLayers::All,
        experts_on_host: false,
        kv_on_host: false,
    };

    if !vram_known {
        // Without a VRAM figure we cannot size offload; attempt full GPU offload
        // and let the runtime fall back rather than pessimistically pinning to CPU.
        notes.push(
            "VRAM unknown; attempting full GPU offload (-ngl 999). If the model OOMs, \
             pass --params for a real estimate or set layers manually."
                .to_string(),
        );
    } else if fits {
        notes.push(format!(
            "Model ~{:.1} GiB fits ~{:.1} GiB usable VRAM; full GPU offload.",
            gib(need),
            gib(usable)
        ));
    } else if model.is_moe {
        offload.experts_on_host = true;
        let resident = need.saturating_sub(model.offloadable_expert_bytes());
        if vram_known && resident <= usable {
            notes.push(format!(
                "MoE experts (~{:.1} GiB) offloaded to host RAM; ~{:.1} GiB stays resident.",
                gib(model.offloadable_expert_bytes()),
                gib(resident)
            ));
        } else {
            // Experts and KV are on host, so only the non-expert weights stay
            // resident — size GPU layers against those, not the full weights.
            let resident_weights = model
                .weights_bytes()
                .saturating_sub(model.offloadable_expert_bytes());
            offload.gpu_layers = GpuLayers::Count(estimate_gpu_layers(
                resident_weights,
                model.n_layers,
                usable,
            ));
            offload.kv_on_host = true;
            notes.push(
                "MoE model tight even with experts on host; also offloading layers/KV — expect lower throughput."
                    .to_string(),
            );
        }
    } else {
        // Dense offload: weights and KV stay on the GPU for the layers that fit.
        let layers = estimate_gpu_layers(
            model.weights_bytes() + model.kv_bytes(),
            model.n_layers,
            usable,
        );
        offload.gpu_layers = GpuLayers::Count(layers);
        if layers == 0 {
            notes.push(
                "Model far exceeds VRAM; running mostly on CPU — low throughput.".to_string(),
            );
        } else {
            notes.push(format!(
                "Dense model exceeds VRAM; {layers} layers on GPU, remainder on host."
            ));
        }
    }

    let multi_gpu = choose_inference_multi_gpu(topo, model, vram_known, &mut notes);
    let gpu_count = gpu_count_for(&multi_gpu, topo.gpu_count());

    Ok(PlacementPlan {
        task: Task::Inference,
        backend,
        gpu_count,
        multi_gpu,
        offload,
        env,
        fits_in_vram: fits,
        notes,
    })
}

#[allow(clippy::too_many_arguments)]
fn plan_training(
    topo: &Topology,
    model: &ModelMeta,
    backend: Backend,
    env: Vec<(String, String)>,
    usable: u64,
    vram_known: bool,
    mut notes: Vec<String>,
) -> Result<PlacementPlan, Error> {
    let train_need = model.weights_bytes() * TRAINING_FOOTPRINT_MULT;
    let fits = vram_known && train_need <= usable;
    let n = topo.gpu_count();

    let multi_gpu = if n <= 1 {
        if !vram_known {
            notes.push(
                "VRAM unknown; assuming the training footprint fits. Enable gradient checkpointing and optimizer offload (ZeRO-Offload) if it OOMs."
                    .to_string(),
            );
        } else if !fits {
            notes.push(
                "Single GPU: training footprint exceeds VRAM; enable gradient checkpointing and optimizer offload (ZeRO-Offload)."
                    .to_string(),
            );
        } else {
            notes.push(format!(
                "Training footprint ~{:.1} GiB fits ~{:.1} GiB usable VRAM.",
                gib(train_need),
                gib(usable)
            ));
        }
        MultiGpu::Single
    } else {
        let link = topo.bottleneck_link().unwrap_or(LinkKind::HostOnly);
        notes.push(format!(
            "Sharding optimizer/grads/params across {n} GPUs (FSDP/ZeRO) over {link:?} link via RCCL."
        ));
        if link == LinkKind::HostOnly {
            notes.push(
                "Host-only inter-GPU link: expect sync to be bandwidth-bound; consider gradient compression / less-frequent sync."
                    .to_string(),
            );
        }
        MultiGpu::Fsdp { shards: n }
    };

    Ok(PlacementPlan {
        task: Task::Training,
        backend,
        gpu_count: gpu_count_for(&multi_gpu, n),
        multi_gpu,
        offload: Offload {
            gpu_layers: GpuLayers::All,
            experts_on_host: false,
            kv_on_host: false,
        },
        env,
        fits_in_vram: fits,
        notes,
    })
}

fn choose_inference_multi_gpu(
    topo: &Topology,
    model: &ModelMeta,
    vram_known: bool,
    notes: &mut Vec<String>,
) -> MultiGpu {
    let n = topo.gpu_count();
    if n <= 1 {
        return MultiGpu::Single;
    }
    let link = topo.bottleneck_link().unwrap_or(LinkKind::HostOnly);
    let single_usable = single_gpu_usable(topo, 0);
    let fits_one = vram_known && model.total_bytes() <= single_usable;

    if fits_one && link == LinkKind::HostOnly {
        notes.push(
            "Multiple GPUs present but host-only linked; model fits one card, so using a single GPU avoids slow cross-card traffic."
                .to_string(),
        );
        return MultiGpu::Single;
    }
    // Deliberate: a model that fits one card is still split across cards on a
    // fast (XGMI/PCIe-P2P) link. The cross-card cost is low there, and the spare
    // VRAM buys KV-cache / batch headroom — which serving especially benefits from.
    // Only a host-only link (above) makes the split not worth it. Override with an
    // explicit backend/placement if you want single-GPU on a fast link.
    if model.is_moe {
        notes.push(format!(
            "Distributing MoE experts across {n} GPUs (expert-parallel) over {link:?} link."
        ));
        MultiGpu::ExpertParallel
    } else {
        let fractions = vram_fractions(topo);
        notes.push(format!(
            "Splitting dense model across {n} GPUs by VRAM proportion over {link:?} link."
        ));
        MultiGpu::LayerSplit { fractions }
    }
}

fn gpu_count_for(multi: &MultiGpu, n: usize) -> usize {
    match multi {
        MultiGpu::Single => 1,
        MultiGpu::LayerSplit { fractions } => fractions.len(),
        MultiGpu::ExpertParallel => n,
        MultiGpu::Fsdp { shards } => *shards,
    }
}

fn single_gpu_usable(topo: &Topology, idx: usize) -> u64 {
    topo.gpus
        .get(idx)
        .and_then(|g| g.vram_mb)
        .map(|mb| (mb * 1024 * 1024) as f64 * VRAM_HEADROOM)
        .map(|b| b as u64)
        .unwrap_or(0)
}

/// Per-GPU VRAM fractions (sum ~1.0). Falls back to an even split if VRAM is
/// unknown.
fn vram_fractions(topo: &Topology) -> Vec<f32> {
    let vrams: Vec<u64> = topo.gpus.iter().map(|g| g.vram_mb.unwrap_or(0)).collect();
    let total: u64 = vrams.iter().sum();
    let n = topo.gpu_count().max(1);
    if total == 0 {
        return vec![1.0 / n as f32; n];
    }
    vrams.iter().map(|&v| v as f32 / total as f32).collect()
}

/// Estimate how many of `n_layers` fit in `usable` bytes, given `resident_bytes`
/// — the footprint that actually stays on the GPU (resident weights + any KV that
/// stays resident). Callers pass only what is truly resident: a branch that
/// offloads experts or KV to host must exclude those bytes here, or it will size
/// per-layer cost far too high and under-fill VRAM.
fn estimate_gpu_layers(resident_bytes: u64, n_layers: u32, usable: u64) -> u32 {
    let layers = n_layers.max(1);
    let per_layer = resident_bytes as f64 / layers as f64;
    if per_layer <= 0.0 {
        return layers;
    }
    ((usable as f64 / per_layer).floor() as u32).min(layers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::QuantLevel;
    use cameo_gpu_detect::{classify, GpuInfo, OverrideDb, Tier};

    fn gpu(gfx: &str, vram_mb: u64) -> GpuInfo {
        GpuInfo {
            model: gfx.into(),
            pci_id: "1002:0000".into(),
            vram_mb: Some(vram_mb),
            gfx_arch: Some(gfx.into()),
            driver_version: None,
        }
    }

    fn topo_with(
        gpus: Vec<GpuInfo>,
        links: Vec<cameo_gpu_detect::Link>,
    ) -> (Topology, Vec<TierAssessment>) {
        let db = OverrideDb::embedded();
        let assessments: Vec<_> = gpus.iter().cloned().map(|g| classify(g, &db)).collect();
        (Topology::new(gpus, links), assessments)
    }

    #[test]
    fn dense_7b_fits_single_tier1_card() {
        let (topo, a) = topo_with(vec![gpu("gfx1100", 16384)], vec![]);
        let m = ModelMeta::dense("llama-7b", 7.0, QuantLevel::Q4_K_M);
        let p = plan(&topo, &a, &m, Task::Inference, &Settings::default()).unwrap();
        assert_eq!(p.backend, Backend::Rocm);
        assert_eq!(p.multi_gpu, MultiGpu::Single);
        assert_eq!(p.offload.gpu_layers, GpuLayers::All);
        assert!(p.fits_in_vram);
    }

    #[test]
    fn big_moe_offloads_experts_to_host() {
        let (topo, a) = topo_with(vec![gpu("gfx1100", 16384)], vec![]);
        let m = ModelMeta::moe("mixtral-47b", 47.0, QuantLevel::Q4_K_M);
        let p = plan(&topo, &a, &m, Task::Inference, &Settings::default()).unwrap();
        assert!(p.offload.experts_on_host);
        assert!(!p.fits_in_vram);
    }

    #[test]
    fn dense_70b_splits_across_two_cards() {
        let links = vec![cameo_gpu_detect::Link {
            a: 0,
            b: 1,
            kind: LinkKind::Xgmi,
        }];
        let (topo, a) = topo_with(vec![gpu("gfx1100", 16384), gpu("gfx1100", 16384)], links);
        let m = ModelMeta::dense("llama-70b", 70.0, QuantLevel::Q4_K_M);
        let p = plan(&topo, &a, &m, Task::Inference, &Settings::default()).unwrap();
        assert!(matches!(p.multi_gpu, MultiGpu::LayerSplit { .. }));
        assert_eq!(p.gpu_count, 2);
    }

    #[test]
    fn host_only_pair_uses_single_when_model_fits_one() {
        let (topo, a) = topo_with(vec![gpu("gfx1100", 24576), gpu("gfx1100", 24576)], vec![]);
        let m = ModelMeta::dense("llama-7b", 7.0, QuantLevel::Q4_K_M);
        let p = plan(&topo, &a, &m, Task::Inference, &Settings::default()).unwrap();
        assert_eq!(p.multi_gpu, MultiGpu::Single);
    }

    #[test]
    fn moe_too_big_sizes_layers_from_resident_weights_not_full() {
        // A huge MoE that doesn't fit even with experts on host hits the "tight"
        // branch. Layers must be sized against the ~10% resident (non-expert)
        // weights; sizing against the full weights gives ~10x too few layers.
        let (topo, a) = topo_with(vec![gpu("gfx1100", 16384)], vec![]);
        let m = ModelMeta::moe("huge-moe", 300.0, QuantLevel::Q4_K_M);
        let p = plan(&topo, &a, &m, Task::Inference, &Settings::default()).unwrap();
        assert!(p.offload.experts_on_host);
        assert!(p.offload.kv_on_host);
        match p.offload.gpu_layers {
            GpuLayers::Count(n) => assert!(n > 30, "expected many resident layers, got {n}"),
            other => panic!("expected Count, got {other:?}"),
        }
    }

    #[test]
    fn unknown_vram_attempts_full_offload_not_cpu() {
        let mut g = gpu("gfx1100", 16384);
        g.vram_mb = None; // VRAM not readable
        let (topo, a) = topo_with(vec![g], vec![]);
        let m = ModelMeta::dense("llama-7b", 7.0, QuantLevel::Q4_K_M);
        let p = plan(&topo, &a, &m, Task::Inference, &Settings::default()).unwrap();
        assert_eq!(p.offload.gpu_layers, GpuLayers::All);
        assert!(!p.offload.experts_on_host);
    }

    #[test]
    fn training_refused_on_tier3() {
        let (topo, a) = topo_with(vec![gpu("gfx803", 8192)], vec![]); // Polaris = Tier 3
        assert_eq!(a[0].tier, Tier::Tier3);
        let m = ModelMeta::dense("small", 1.0, QuantLevel::Q4_K_M);
        let err = plan(&topo, &a, &m, Task::Training, &Settings::default()).unwrap_err();
        assert!(matches!(err, Error::TrainingUnsupported(3)));
    }

    #[test]
    fn training_shards_across_two_tier1_cards() {
        let links = vec![cameo_gpu_detect::Link {
            a: 0,
            b: 1,
            kind: LinkKind::Xgmi,
        }];
        let (topo, a) = topo_with(vec![gpu("gfx1100", 24576), gpu("gfx1100", 24576)], links);
        let m = ModelMeta::dense("llama-7b", 7.0, QuantLevel::Q4_K_M);
        let p = plan(&topo, &a, &m, Task::Training, &Settings::default()).unwrap();
        assert_eq!(p.multi_gpu, MultiGpu::Fsdp { shards: 2 });
        assert_eq!(p.backend, Backend::Rocm);
    }
}
