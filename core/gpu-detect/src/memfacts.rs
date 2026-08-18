//! Captured GPU memory facts.
//!
//! VRAM, the GTT aperture and the memory *type* come from `/sys/class/drm`,
//! which exists only on the target machine. Without a way to feed them in, the
//! entire memory-planning path — the part that decides whether a model fits an
//! APU's shared pool — could not be exercised until someone booted the ISO.
//!
//! This module is that way in: a small TOML capture of what sysfs would report,
//! keyed by PCI address. It is a dev/testing affordance, not a config file; live
//! detection never consults it.

use crate::error::Error;
use crate::parse::{normalize_pci_addr, parse_memory_kind};
use crate::types::GpuInfo;
use serde::Deserialize;

/// One card's memory facts, as `/sys/class/drm/cardN/device` reports them.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct GpuMemoryFact {
    /// PCI address, with or without the domain (`0000:c5:00.0` / `c5:00.0`).
    pub pci_addr: String,
    /// `mem_info_vram_total`, in MiB.
    #[serde(default)]
    pub vram_mb: Option<u64>,
    /// `mem_info_gtt_total`, in MiB.
    #[serde(default)]
    pub gtt_mb: Option<u64>,
    /// `mem_info_vram_type`, verbatim — e.g. `"GDDR6"`, `"DDR5"`. Classified by
    /// the same function the live collector uses, so a capture cannot drift
    /// from the real thing.
    #[serde(default)]
    pub vram_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawFacts {
    #[serde(default)]
    gpu: Vec<GpuMemoryFact>,
}

/// Parse a memory-facts capture.
pub fn parse_gpu_memory(text: &str) -> Result<Vec<GpuMemoryFact>, Error> {
    let raw: RawFacts = toml::from_str(text)?;
    Ok(raw.gpu)
}

/// Apply captured facts to the matching cards, by PCI address. Returns how many
/// cards were matched; facts naming an address that is not present are ignored.
pub fn apply_gpu_memory(gpus: &mut [GpuInfo], facts: &[GpuMemoryFact]) -> usize {
    let mut applied = 0;
    for fact in facts {
        let Some(want) = normalize_pci_addr(&fact.pci_addr) else {
            continue;
        };
        for gpu in gpus.iter_mut() {
            if gpu.pci_addr.as_deref() != Some(want.as_str()) {
                continue;
            }
            if fact.vram_mb.is_some() {
                gpu.vram_mb = fact.vram_mb;
            }
            if fact.gtt_mb.is_some() {
                gpu.gtt_mb = fact.gtt_mb;
            }
            if let Some(t) = &fact.vram_type {
                gpu.memory = parse_memory_kind(t);
            }
            applied += 1;
        }
    }
    applied
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_lspci;
    use crate::types::MemoryKind;

    const LSPCI: &str = "\
03:00.0 VGA compatible controller [0300]: Advanced Micro Devices, Inc. [AMD/ATI] Navi 31 [Radeon RX 7900 XTX] [1002:744c]
c5:00.0 VGA compatible controller [0300]: Advanced Micro Devices, Inc. [AMD/ATI] Phoenix1 [1002:15bf]";

    const FACTS: &str = r#"
[[gpu]]
pci_addr = "0000:03:00.0"
vram_mb = 24560
vram_type = "GDDR6"

[[gpu]]
pci_addr = "c5:00.0"
vram_mb = 512
gtt_mb = 4096
vram_type = "DDR5"
"#;

    #[test]
    fn facts_land_on_the_card_named_by_address() {
        let mut gpus = parse_lspci(LSPCI);
        let facts = parse_gpu_memory(FACTS).unwrap();
        assert_eq!(apply_gpu_memory(&mut gpus, &facts), 2);

        assert_eq!(gpus[0].vram_mb, Some(24560));
        assert_eq!(gpus[0].memory, MemoryKind::Dedicated);
        assert_eq!(gpus[0].gtt_mb, None);

        // The domain-less form resolves to the same card.
        assert_eq!(gpus[1].vram_mb, Some(512));
        assert_eq!(gpus[1].gtt_mb, Some(4096));
        assert_eq!(gpus[1].memory, MemoryKind::Shared);
    }

    #[test]
    fn facts_for_an_absent_card_are_ignored() {
        let mut gpus = parse_lspci(LSPCI);
        let facts =
            parse_gpu_memory("[[gpu]]\npci_addr = \"0000:99:00.0\"\nvram_mb = 1\n").unwrap();
        assert_eq!(apply_gpu_memory(&mut gpus, &facts), 0);
        assert!(gpus.iter().all(|g| g.vram_mb.is_none()));
    }
}
