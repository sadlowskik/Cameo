//! Core data types shared across the detection pipeline.

use serde::{Deserialize, Serialize};

/// Facts about a single detected AMD GPU. Optional fields are best-effort:
/// `gfx_arch` in particular is only available when a ROCm stack (`rocminfo`)
/// is present, which is exactly what distinguishes Tier 3 from Tier 1/2.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpuInfo {
    /// Human-readable model string, e.g. `"Navi 22 [Radeon RX 6700 XT]"`.
    pub model: String,
    /// PCI `vendor:device` id, lowercased, e.g. `"1002:73df"`.
    pub pci_id: String,
    /// Total VRAM in MiB, if readable from sysfs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vram_mb: Option<u64>,
    /// AMD GCN/RDNA architecture string, e.g. `"gfx1030"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gfx_arch: Option<String>,
    /// amdgpu / kernel driver version, if known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub driver_version: Option<String>,
}

/// GPU support tier (see `docs/tiers.md` and plan §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tier {
    /// ROCm officially supported: full training + inference.
    Tier1,
    /// ROCm workable via `HSA_OVERRIDE_GFX_VERSION`: inference; training community-tested.
    Tier2,
    /// No usable ROCm path: Vulkan-only inference, no training.
    Tier3,
}

impl Tier {
    /// The tier as a plain number (1/2/3) for display and `--json`.
    pub fn as_number(self) -> u8 {
        match self {
            Tier::Tier1 => 1,
            Tier::Tier2 => 2,
            Tier::Tier3 => 3,
        }
    }

    /// Whether training is supported on this tier (Tier 3 is inference-only).
    pub fn training_supported(self) -> bool {
        !matches!(self, Tier::Tier3)
    }
}

/// The result of classifying a [`GpuInfo`]: the tier plus the plain-language
/// reasoning and any suggested `HSA_OVERRIDE_GFX_VERSION`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TierAssessment {
    pub gpu: GpuInfo,
    pub tier: Tier,
    /// Suggested `HSA_OVERRIDE_GFX_VERSION` value (Tier 2 only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hsa_override: Option<String>,
    pub training_supported: bool,
    /// User-facing explanation of why this tier was chosen.
    pub rationale: String,
}
