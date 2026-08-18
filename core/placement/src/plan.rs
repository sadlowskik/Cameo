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

/// Fraction of *available* host RAM we are willing to commit to offload. The
/// rest is the operating system's: page cache, the shell you typed this into,
/// and the model file being read off disk.
pub(crate) const HOST_HEADROOM: f64 = 0.75;

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

/// What the machine actually has to spend, after headroom.
///
/// The distinction that matters is whether GPU memory is a *separate* pool. On
/// a discrete card it is, and VRAM plus host RAM really do add up. On an APU the
/// carve-out and the GTT aperture are both system RAM, so counting "VRAM" and
/// "host RAM to offload into" as independent budgets spends the same DIMM twice
/// and produces a plan the kernel OOM-kills.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MemoryBudget {
    /// Bytes of GPU-local memory after headroom: a discrete card's VRAM, an
    /// APU's BIOS carve-out, or the sum of both on a mixed machine.
    pub vram_bytes: u64,
    /// Bytes of host RAM an APU can additionally address through its GTT
    /// aperture. Real GPU capacity, but the *same bytes* as `usable_host_bytes`
    /// — which is why it is tracked apart rather than folded in.
    pub gtt_reachable_bytes: u64,
    /// Bytes of host RAM available for offload. `None` when `/proc/meminfo` was
    /// not readable — which is *unknown*, not zero, so the ceiling is not enforced.
    pub usable_host_bytes: Option<u64>,
    /// Whether every GPU reported its VRAM.
    pub vram_known: bool,
    /// Whether any GPU's memory is carved out of system RAM.
    pub shared_memory: bool,
}

impl MemoryBudget {
    /// Derive the budget from a detected topology.
    pub fn of(topo: &Topology) -> Self {
        let vram_known = !topo.gpus.is_empty() && topo.gpus.iter().all(|g| g.vram_mb.is_some());
        let shared_memory = topo.gpus.iter().any(|g| g.memory.is_shared_with_host());

        let vram_total: u64 = topo
            .gpus
            .iter()
            .filter_map(|g| g.vram_bytes())
            .fold(0, u64::saturating_add);
        let vram_usable = scale(vram_total, VRAM_HEADROOM);

        let usable_host_bytes = topo
            .host_mem
            .map(|h| scale(h.available_bytes, HOST_HEADROOM));

        // An APU can also address host RAM through its GTT aperture. That is
        // real GPU-resident capacity, but it is *the same bytes* as the host
        // budget — so it raises the GPU ceiling without raising the total one.
        let gtt_reachable = if shared_memory {
            let gtt: u64 = topo
                .gpus
                .iter()
                .filter(|g| g.memory.is_shared_with_host())
                .filter_map(|g| g.gtt_bytes())
                .fold(0, u64::saturating_add);
            match usable_host_bytes {
                Some(host) => gtt.min(host),
                None => 0,
            }
        } else {
            0
        };

        Self {
            vram_bytes: vram_usable,
            gtt_reachable_bytes: gtt_reachable,
            usable_host_bytes,
            vram_known,
            shared_memory,
        }
    }

    /// Everything the GPU(s) can hold: their own memory plus, on an APU, the
    /// host RAM they can reach through GTT.
    pub fn usable_vram(&self) -> u64 {
        self.vram_bytes.saturating_add(self.gtt_reachable_bytes)
    }

    /// The most this machine can hold in total, counting the shared pool once.
    /// `None` when host memory is unknown and no honest ceiling exists.
    ///
    /// GTT is deliberately absent from this sum: it is a *window onto* the host
    /// budget, not memory in addition to it.
    pub fn ceiling_bytes(&self) -> Option<u64> {
        Some(self.vram_bytes.saturating_add(self.usable_host_bytes?))
    }
}

/// Multiply a byte count by a headroom fraction without overflowing.
fn scale(bytes: u64, factor: f64) -> u64 {
    let v = bytes as f64 * factor;
    if !v.is_finite() || v <= 0.0 {
        0
    } else if v >= u64::MAX as f64 {
        u64::MAX
    } else {
        v as u64
    }
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
    /// The memory this plan was sized against.
    pub budget: MemoryBudget,
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
    // Before any arithmetic: a description that cannot be reasoned about must
    // fail here with a message, not downstream as a nonsense byte count.
    model.validate()?;
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

    let budget = MemoryBudget::of(topo);

    let mut notes = Vec::new();
    if !budget.vram_known {
        notes.push("VRAM unknown for at least one GPU; planning conservatively.".to_string());
    }
    if assessments.iter().any(|a| a.tier != top.tier) {
        notes.push("Mixed GPU tiers detected; planning to the top GPU's tier.".to_string());
    }
    match budget.usable_host_bytes {
        Some(h) => notes.push(format!(
            "~{:.1} GiB of host RAM available for offload.",
            gib(h)
        )),
        None => notes.push(
            "Host RAM unknown (no /proc/meminfo); offload sizing is unchecked. \
             Plans that spill to host RAM here are not verified to fit."
                .to_string(),
        ),
    }
    if budget.shared_memory {
        notes.push(
            "Integrated GPU: its memory is carved from system RAM, so GPU and host \
             offload draw on one pool rather than two."
                .to_string(),
        );
    }

    match task {
        Task::Inference => plan_inference(topo, model, backend, env, budget, settings, notes),
        Task::Training => plan_training(topo, model, backend, env, budget, settings, notes),
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

/// Refuse a workload that does not fit VRAM *and* host RAM combined.
///
/// Only enforced when both figures are actually known. Unknown host memory is
/// unknown, not zero — refusing there would break every dev-host plan fed from
/// captured fixtures. `allow_oversize` exists because every smart default in
/// Cameo is overridable, and mmap-from-disk is a real (slow) fallback.
fn check_ceiling(budget: &MemoryBudget, need: u64, settings: &Settings) -> Result<(), Error> {
    if settings.allow_oversize.unwrap_or(false) || !budget.vram_known {
        return Ok(());
    }
    let Some(ceiling) = budget.ceiling_bytes() else {
        return Ok(());
    };
    if need > ceiling {
        return Err(Error::InsufficientMemory {
            needed_gib: gib(need),
            available_gib: gib(ceiling),
        });
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn plan_inference(
    topo: &Topology,
    model: &ModelMeta,
    backend: Backend,
    env: Vec<(String, String)>,
    budget: MemoryBudget,
    settings: &Settings,
    mut notes: Vec<String>,
) -> Result<PlacementPlan, Error> {
    let need = model.total_bytes();
    check_ceiling(&budget, need, settings)?;

    let usable = budget.usable_vram();
    let vram_known = budget.vram_known;
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
        if resident <= usable {
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
            model.weights_bytes().saturating_add(model.kv_bytes()),
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

    let multi_gpu =
        choose_inference_multi_gpu(topo, model, &budget, offload.experts_on_host, &mut notes);
    let gpu_count = gpu_count_for(&multi_gpu, topo.gpu_count());

    Ok(PlacementPlan {
        task: Task::Inference,
        backend,
        gpu_count,
        multi_gpu,
        offload,
        budget,
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
    budget: MemoryBudget,
    settings: &Settings,
    mut notes: Vec<String>,
) -> Result<PlacementPlan, Error> {
    let train_need = model
        .weights_bytes()
        .saturating_mul(TRAINING_FOOTPRINT_MULT);
    check_ceiling(&budget, train_need, settings)?;

    let usable = budget.usable_vram();
    let vram_known = budget.vram_known;
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
        budget,
        env,
        fits_in_vram: fits,
        notes,
    })
}

fn choose_inference_multi_gpu(
    topo: &Topology,
    model: &ModelMeta,
    budget: &MemoryBudget,
    experts_on_host: bool,
    notes: &mut Vec<String>,
) -> MultiGpu {
    let n = topo.gpu_count();
    if n <= 1 {
        return MultiGpu::Single;
    }
    let link = topo.bottleneck_link().unwrap_or(LinkKind::HostOnly);
    let single_usable = single_gpu_usable(topo, 0);
    let fits_one = budget.vram_known && model.total_bytes() <= single_usable;

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
    if model.is_moe && !experts_on_host {
        notes.push(format!(
            "Distributing MoE experts across {n} GPUs (expert-parallel) over {link:?} link."
        ));
        MultiGpu::ExpertParallel
    } else {
        // Expert-parallel and experts-on-host are mutually exclusive: one asks
        // the cards to share the expert tensors, the other pins those same
        // tensors to the CPU. Emitting both produced a command that told
        // llama.cpp to do each. With experts on host, what is left to spread
        // across cards is the resident (non-expert) weights — a layer split.
        let fractions = vram_fractions(topo);
        if model.is_moe {
            notes.push(format!(
                "MoE experts are on host RAM, so the {n} GPUs split the resident \
                 (non-expert) weights by VRAM proportion over {link:?} link."
            ));
        } else {
            notes.push(format!(
                "Splitting dense model across {n} GPUs by VRAM proportion over {link:?} link."
            ));
        }
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
        .and_then(|g| g.vram_bytes())
        .map(|b| scale(b, VRAM_HEADROOM))
        .unwrap_or(0)
}

/// Per-GPU VRAM fractions (sum ~1.0). Falls back to an even split if VRAM is
/// unknown.
fn vram_fractions(topo: &Topology) -> Vec<f32> {
    let vrams: Vec<u64> = topo.gpus.iter().map(|g| g.vram_mb.unwrap_or(0)).collect();
    let total: u64 = vrams.iter().fold(0, |a, b| a.saturating_add(*b));
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
    use cameo_gpu_detect::{classify, GpuInfo, HostMemory, MemoryKind, OverrideDb, Tier};

    fn gpu(gfx: &str, vram_mb: u64) -> GpuInfo {
        GpuInfo {
            model: gfx.into(),
            pci_id: "1002:0000".into(),
            vram_mb: Some(vram_mb),
            gfx_arch: Some(gfx.into()),
            memory: MemoryKind::Dedicated,
            ..Default::default()
        }
    }

    /// An integrated GPU: a small BIOS carve-out plus a GTT window, both of
    /// which are system RAM.
    fn igpu(gfx: &str, carveout_mb: u64, gtt_mb: u64) -> GpuInfo {
        GpuInfo {
            model: gfx.into(),
            pci_id: "1002:15bf".into(),
            vram_mb: Some(carveout_mb),
            gtt_mb: Some(gtt_mb),
            gfx_arch: Some(gfx.into()),
            memory: MemoryKind::Shared,
            ..Default::default()
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

    /// Gibibytes as bytes, for readable fixtures.
    fn gb(n: f64) -> u64 {
        (n * 1024.0 * 1024.0 * 1024.0) as u64
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

    // ---- host memory --------------------------------------------------------

    #[test]
    fn model_beyond_vram_and_host_is_refused_not_planned() {
        // 16 GiB card, 8 GiB of RAM: a 70B Q4 (~40 GiB) fits neither, and the
        // old planner happily emitted a layer-split command for it.
        let (topo, a) = topo_with(vec![gpu("gfx1100", 16384)], vec![]);
        let topo = topo.with_host_memory(Some(HostMemory {
            total_bytes: gb(8.0),
            available_bytes: gb(6.0),
        }));
        let m = ModelMeta::dense("llama-70b", 70.0, QuantLevel::Q4_K_M);
        let err = plan(&topo, &a, &m, Task::Inference, &Settings::default()).unwrap_err();
        assert!(
            matches!(err, Error::InsufficientMemory { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn oversize_is_allowed_when_explicitly_asked_for() {
        let (topo, a) = topo_with(vec![gpu("gfx1100", 16384)], vec![]);
        let topo = topo.with_host_memory(Some(HostMemory {
            total_bytes: gb(8.0),
            available_bytes: gb(6.0),
        }));
        let m = ModelMeta::dense("llama-70b", 70.0, QuantLevel::Q4_K_M);
        let settings = Settings {
            allow_oversize: Some(true),
            ..Default::default()
        };
        assert!(plan(&topo, &a, &m, Task::Inference, &settings).is_ok());
    }

    #[test]
    fn unknown_host_memory_does_not_refuse() {
        // Unknown is not zero. Dev hosts fed from captured fixtures have no
        // /proc/meminfo, and must still be able to plan.
        let (topo, a) = topo_with(vec![gpu("gfx1100", 16384)], vec![]);
        assert!(topo.host_mem.is_none());
        let m = ModelMeta::dense("llama-70b", 70.0, QuantLevel::Q4_K_M);
        let p = plan(&topo, &a, &m, Task::Inference, &Settings::default()).unwrap();
        assert!(p.notes.iter().any(|n| n.contains("Host RAM unknown")));
    }

    #[test]
    fn apu_does_not_count_the_same_ram_twice() {
        // 512 MiB carve-out, 4 GiB GTT, 4 GiB of RAM with ~2.5 GiB available.
        // Counting "VRAM + host" as separate pools makes this look like it can
        // hold ~2.4 GiB of GPU memory *plus* ~1.9 GiB of host offload; it cannot.
        let (topo, a) = topo_with(vec![igpu("gfx1103", 512, 4096)], vec![]);
        let topo = topo.with_host_memory(Some(HostMemory {
            total_bytes: gb(4.0),
            available_bytes: gb(2.5),
        }));
        let budget = MemoryBudget::of(&topo);
        assert!(budget.shared_memory);

        let host = budget.usable_host_bytes.unwrap();
        let ceiling = budget.ceiling_bytes().unwrap();
        assert!(
            ceiling < budget.usable_vram() + host,
            "ceiling {ceiling} must be less than the naive sum"
        );

        // A 7B Q4 (~4.2 GiB) does not fit this machine in any arrangement.
        let m = ModelMeta::dense("llama-7b", 7.0, QuantLevel::Q4_K_M);
        assert!(matches!(
            plan(&topo, &a, &m, Task::Inference, &Settings::default()),
            Err(Error::InsufficientMemory { .. })
        ));

        // A 1.1B Q4 (~0.7 GiB) does, and gets the GTT window it needs.
        let small = ModelMeta::dense("tinyllama", 1.1, QuantLevel::Q4_K_M);
        let p = plan(&topo, &a, &small, Task::Inference, &Settings::default()).unwrap();
        assert!(p.notes.iter().any(|n| n.contains("Integrated GPU")));
        assert!(p.fits_in_vram, "notes: {:?}", p.notes);
    }

    // ---- validation ---------------------------------------------------------

    #[test]
    fn nonsense_params_are_refused_before_any_math() {
        let (topo, a) = topo_with(vec![gpu("gfx1100", 16384)], vec![]);
        for bad in [f64::INFINITY, f64::NAN, -1.0] {
            let m = ModelMeta::dense("x", bad, QuantLevel::Q4_K_M);
            let err = plan(&topo, &a, &m, Task::Inference, &Settings::default()).unwrap_err();
            assert!(matches!(err, Error::InvalidModel(_)), "{bad}: {err:?}");
        }
    }

    // ---- MoE consistency ----------------------------------------------------

    #[test]
    fn experts_on_host_never_coexists_with_expert_parallel() {
        // Two cards, fast link, an MoE far too big for their combined VRAM.
        // The offload decision pins experts to the CPU; the multi-GPU decision
        // used to independently choose to distribute those same experts.
        let links = vec![cameo_gpu_detect::Link {
            a: 0,
            b: 1,
            kind: LinkKind::Xgmi,
        }];
        let (topo, a) = topo_with(vec![gpu("gfx1100", 8192), gpu("gfx1100", 8192)], links);
        let m = ModelMeta::moe("huge-moe", 200.0, QuantLevel::Q4_K_M);
        let p = plan(&topo, &a, &m, Task::Inference, &Settings::default()).unwrap();
        assert!(p.offload.experts_on_host);
        assert_ne!(
            p.multi_gpu,
            MultiGpu::ExpertParallel,
            "cannot both pin experts to host and spread them across cards"
        );
        assert!(matches!(p.multi_gpu, MultiGpu::LayerSplit { .. }));
    }

    #[test]
    fn moe_that_fits_the_cards_still_goes_expert_parallel() {
        let links = vec![cameo_gpu_detect::Link {
            a: 0,
            b: 1,
            kind: LinkKind::Xgmi,
        }];
        let (topo, a) = topo_with(vec![gpu("gfx1100", 24576), gpu("gfx1100", 24576)], links);
        let m = ModelMeta::moe("mixtral-47b", 47.0, QuantLevel::Q4_K_M);
        let p = plan(&topo, &a, &m, Task::Inference, &Settings::default()).unwrap();
        assert!(!p.offload.experts_on_host);
        assert_eq!(p.multi_gpu, MultiGpu::ExpertParallel);
    }
}
