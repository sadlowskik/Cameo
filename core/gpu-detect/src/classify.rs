//! Tier classification: [`GpuInfo`] + [`OverrideDb`] -> [`TierAssessment`].

use crate::overrides::OverrideDb;
use crate::topology::Topology;
use crate::types::{GpuInfo, Tier, TierAssessment, Vendor};

/// Classify every GPU in a [`Topology`], preserving card order.
pub fn classify_topology(topo: &Topology, db: &OverrideDb) -> Vec<TierAssessment> {
    topo.gpus
        .iter()
        .cloned()
        .map(|gpu| classify(gpu, db))
        .collect()
}

/// Classify a GPU into a support tier using the compatibility database.
///
/// The rules, in order:
/// 1. Architecture found in the database -> use its tier + override.
/// 2. Architecture detected but unknown -> conservative Tier 3 (Vulkan-only).
/// 3. No architecture detected at all (no ROCm stack) -> Tier 3.
///
/// The conservative default matters: Cameo never silently claims a ROCm path it
/// hasn't been told works. The user can always override in config.
pub fn classify(gpu: GpuInfo, db: &OverrideDb) -> TierAssessment {
    // Non-AMD GPUs have no ROCm path: Cameo runs them on the Vulkan universal
    // backend (Tier 3-equivalent, inference-only). Recognising them by vendor
    // makes an NVIDIA/Intel box a first-class Vulkan target rather than a "no GPU
    // detected" failure (F6). Per-vendor acceleration (CUDA) is a container-first
    // future; the bare-metal/ISO path stays AMD-validated.
    if gpu.vendor != Vendor::Amd {
        let rationale = match gpu.vendor {
            Vendor::Nvidia => "NVIDIA GPU: running on the Vulkan universal backend \
                 (no CUDA acceleration configured). Inference only; ROCm training is AMD-only."
                .to_string(),
            Vendor::Intel => "Intel GPU: running on the Vulkan universal backend. \
                 Inference only."
                .to_string(),
            _ => format!(
                "{} GPU: running on the Vulkan universal backend. Inference only.",
                gpu.vendor.label()
            ),
        };
        return TierAssessment {
            tier: Tier::Tier3,
            hsa_override: None,
            training_supported: false,
            rationale,
            gpu,
        };
    }

    let Some(arch) = gpu.gfx_arch.clone() else {
        return TierAssessment {
            tier: Tier::Tier3,
            hsa_override: None,
            training_supported: false,
            rationale: "No gfx architecture detected (rocminfo unavailable). \
                        Defaulting to Vulkan-only inference (Tier 3)."
                .to_string(),
            gpu,
        };
    };

    if let Some(entry) = db.lookup(&arch) {
        let rationale = match entry.tier {
            Tier::Tier1 => {
                format!("{arch} is officially ROCm-supported: full training and inference.")
            }
            Tier::Tier2 => {
                let via = entry
                    .hsa_override
                    .as_ref()
                    .map(|o| format!(" via HSA_OVERRIDE_GFX_VERSION={o}"))
                    .unwrap_or_default();
                format!(
                    "{arch} has no official ROCm support but is community-workable{via}. \
                     Inference supported; training is community-tested, not guaranteed."
                )
            }
            Tier::Tier3 => {
                format!("{arch} has no usable ROCm path: Vulkan-only inference, no training.")
            }
        };
        return TierAssessment {
            tier: entry.tier,
            hsa_override: entry.hsa_override.clone(),
            training_supported: entry.tier.training_supported(),
            rationale,
            gpu,
        };
    }

    TierAssessment {
        tier: Tier::Tier3,
        hsa_override: None,
        training_supported: false,
        rationale: format!(
            "{arch} is not in the compatibility database; defaulting to Vulkan-only (Tier 3). \
             If you know a working ROCm override for this card, set it in config."
        ),
        gpu,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gpu_with(arch: Option<&str>) -> GpuInfo {
        GpuInfo {
            model: "test".into(),
            pci_id: "1002:0000".into(),
            vram_mb: None,
            gfx_arch: arch.map(str::to_string),
            driver_version: None,
            ..Default::default()
        }
    }

    #[test]
    fn tier2_arch_gets_override_and_training_flag() {
        let db = OverrideDb::embedded();
        let a = classify(gpu_with(Some("gfx1030")), &db);
        assert_eq!(a.tier, Tier::Tier2);
        assert_eq!(a.hsa_override.as_deref(), Some("10.3.0"));
        assert!(a.training_supported);
    }

    #[test]
    fn tier1_arch_has_no_override() {
        let db = OverrideDb::embedded();
        let a = classify(gpu_with(Some("gfx1100")), &db);
        assert_eq!(a.tier, Tier::Tier1);
        assert!(a.hsa_override.is_none());
        assert!(a.training_supported);
    }

    #[test]
    fn unknown_arch_is_tier3() {
        let db = OverrideDb::embedded();
        let a = classify(gpu_with(Some("gfx1201")), &db);
        assert_eq!(a.tier, Tier::Tier3);
        assert!(!a.training_supported);
    }

    #[test]
    fn no_arch_is_tier3() {
        let db = OverrideDb::embedded();
        let a = classify(gpu_with(None), &db);
        assert_eq!(a.tier, Tier::Tier3);
    }
}
