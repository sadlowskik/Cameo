//! Core data types shared across the detection pipeline.

use serde::{Deserialize, Serialize};

/// Where a GPU's "VRAM" physically lives.
///
/// This is not cosmetic. On a discrete card VRAM is a separate pool that host
/// RAM offload can spill into. On an APU the carve-out *is* system RAM, so the
/// planner must not count it twice — that double-count is what turns a 4 GB
/// laptop into a plan the kernel OOM-kills.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    /// Discrete card with its own memory (GDDR/HBM).
    Dedicated,
    /// Integrated GPU: the carve-out and the GTT aperture are both system RAM.
    Shared,
    /// Not determinable from sysfs — treated as [`MemoryKind::Dedicated`] for
    /// VRAM accounting but flagged in the plan notes.
    Unknown,
}

impl MemoryKind {
    /// Whether this GPU's memory competes with host RAM for the same physical pages.
    pub fn is_shared_with_host(self) -> bool {
        matches!(self, MemoryKind::Shared)
    }
}

/// Which silicon vendor a GPU is, keyed off its PCI vendor id.
///
/// Cameo validates and ROCm-accelerates **AMD**; every other vendor runs on the
/// Vulkan universal backend (Tier 3-equivalent). Recognising them by vendor (F6)
/// is what turns "no AMD GPU detected" into a first-class Vulkan target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Vendor {
    /// AMD/ATI (`1002`) — the validated, ROCm-capable path.
    #[default]
    Amd,
    /// NVIDIA (`10de`) — Vulkan baseline (CUDA accel is a container-first future).
    Nvidia,
    /// Intel (`8086`) — Vulkan baseline.
    Intel,
    /// A display device from some other vendor.
    Other,
}

impl Vendor {
    /// Map a PCI `vendor:device` id (or a bare 4-hex vendor) to a [`Vendor`].
    pub fn from_pci_id(pci_id: &str) -> Self {
        match pci_id
            .split(':')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str()
        {
            "1002" => Vendor::Amd,
            "10de" => Vendor::Nvidia,
            "8086" => Vendor::Intel,
            _ => Vendor::Other,
        }
    }

    /// Whether this is a GPU vendor Cameo detects (AMD/NVIDIA/Intel).
    pub fn is_known_gpu(self) -> bool {
        matches!(self, Vendor::Amd | Vendor::Nvidia | Vendor::Intel)
    }

    /// Human-readable vendor name.
    pub fn label(self) -> &'static str {
        match self {
            Vendor::Amd => "AMD",
            Vendor::Nvidia => "NVIDIA",
            Vendor::Intel => "Intel",
            Vendor::Other => "other",
        }
    }
}

/// Facts about a single detected GPU. Optional fields are best-effort: `gfx_arch`
/// in particular is only available when a ROCm stack (`rocminfo`) is present,
/// which is exactly what distinguishes an AMD Tier 3 from Tier 1/2.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpuInfo {
    /// Human-readable model string, e.g. `"Navi 22 [Radeon RX 6700 XT]"`.
    pub model: String,
    /// Silicon vendor, derived from the PCI vendor id.
    #[serde(default)]
    pub vendor: Vendor,
    /// PCI `vendor:device` id, lowercased, e.g. `"1002:73df"`.
    pub pci_id: String,
    /// Full PCI address, domain-normalised, e.g. `"0000:0a:00.0"`. This is the
    /// only reliable key between `lspci`, `rocminfo` and `/sys/class/drm`; card
    /// *ordering* in those three sources is unrelated, so anything that pairs
    /// them by index is reading another card's facts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pci_addr: Option<String>,
    /// Total VRAM in MiB, if readable from sysfs. On an APU this is the BIOS
    /// carve-out, not the memory the GPU can actually address — see `gtt_mb`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vram_mb: Option<u64>,
    /// GTT (system-memory) aperture in MiB: host RAM the GPU can address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gtt_mb: Option<u64>,
    /// Whether `vram_mb` is a dedicated pool or carved out of system RAM.
    #[serde(default = "unknown_memory")]
    pub memory: MemoryKind,
    /// AMD GCN/RDNA architecture string, e.g. `"gfx1030"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gfx_arch: Option<String>,
    /// amdgpu / kernel driver version, if known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub driver_version: Option<String>,
}

fn unknown_memory() -> MemoryKind {
    MemoryKind::Unknown
}

impl GpuInfo {
    /// A GPU known only by model and PCI id — every best-effort field unset.
    /// Detection fills the rest in; tests build from here so that adding a
    /// field does not mean editing every fixture.
    pub fn new(model: impl Into<String>, pci_id: impl Into<String>) -> Self {
        let pci_id = pci_id.into();
        let vendor = Vendor::from_pci_id(&pci_id);
        Self {
            model: model.into(),
            vendor,
            pci_id,
            pci_addr: None,
            vram_mb: None,
            gtt_mb: None,
            memory: MemoryKind::Unknown,
            gfx_arch: None,
            driver_version: None,
        }
    }

    /// Bytes of GPU-local memory (the carve-out on an APU), if known.
    pub fn vram_bytes(&self) -> Option<u64> {
        self.vram_mb.map(|mb| mb.saturating_mul(1024 * 1024))
    }

    /// Bytes of host RAM this GPU can address through its GTT aperture, if known.
    pub fn gtt_bytes(&self) -> Option<u64> {
        self.gtt_mb.map(|mb| mb.saturating_mul(1024 * 1024))
    }
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

impl Default for GpuInfo {
    /// An empty GPU record. Useful mainly as the tail of a struct-update
    /// expression, so adding a best-effort field does not touch every caller.
    ///
    /// `vendor` defaults to AMD: production always builds a `GpuInfo` through
    /// [`GpuInfo::new`], which derives the vendor from the real PCI id, so the
    /// only users of this default are struct-update expressions that fill an AMD
    /// `pci_id` — matching [`Vendor`]'s own default keeps them consistent.
    fn default() -> Self {
        let mut gpu = Self::new(String::new(), String::new());
        gpu.vendor = Vendor::Amd;
        gpu
    }
}
