//! Pure parsers: text in, [`GpuInfo`] fields out. No I/O, testable anywhere.

use crate::types::{GpuInfo, MemoryKind};
use regex::Regex;
use std::sync::OnceLock;

/// Matches a bracketed PCI `vendor:device` id, e.g. `[1002:73df]`.
fn pci_id_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\[([0-9a-fA-F]{4}):([0-9a-fA-F]{4})\]").unwrap())
}

/// Matches an AMD architecture token, e.g. `gfx1030`, `gfx900`, `gfx1103`.
fn gfx_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"gfx[0-9a-fA-F]+").unwrap())
}

/// Matches an `lspci` address with or without the PCI domain.
fn pci_addr_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^(?:([0-9a-fA-F]{4}):)?([0-9a-fA-F]{2}):([0-9a-fA-F]{2})\.([0-7])$").unwrap()
    })
}

/// A PCI bus/device/function triple — the part of an address that `rocminfo`
/// reports and that therefore correlates the two sources. The domain is
/// deliberately excluded: `BDFID` does not carry one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bdf {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
}

impl Bdf {
    /// Decode ROCm's packed `BDFID`: `bus << 8 | device << 3 | function`.
    pub fn from_bdfid(id: u32) -> Self {
        Self {
            bus: ((id >> 8) & 0xff) as u8,
            device: ((id >> 3) & 0x1f) as u8,
            function: (id & 0x7) as u8,
        }
    }

    /// Parse from a full or domain-less PCI address (`0000:0a:00.0`, `0a:00.0`).
    pub fn parse(addr: &str) -> Option<Self> {
        let c = pci_addr_re().captures(addr.trim())?;
        Some(Self {
            bus: u8::from_str_radix(&c[2], 16).ok()?,
            device: u8::from_str_radix(&c[3], 16).ok()?,
            function: c[4].parse().ok()?,
        })
    }
}

/// Normalise a PCI address to include an explicit domain.
///
/// `lspci` omits the domain when every device sits in domain 0000, but sysfs
/// always spells it out. Normalising here means the two can be compared.
pub fn normalize_pci_addr(addr: &str) -> Option<String> {
    let c = pci_addr_re().captures(addr.trim())?;
    let domain = c.get(1).map(|m| m.as_str()).unwrap_or("0000");
    Some(format!(
        "{}:{}:{}.{}",
        domain.to_lowercase(),
        c[2].to_lowercase(),
        c[3].to_lowercase(),
        &c[4]
    ))
}

/// Parse `lspci -nn` output into one [`GpuInfo`] per AMD display device.
///
/// Only VGA / display / 3D-controller lines with AMD vendor id `1002` are kept,
/// so the HDMI audio function that shares the card is ignored.
pub fn parse_lspci(output: &str) -> Vec<GpuInfo> {
    let mut gpus = Vec::new();
    for line in output.lines() {
        let lower = line.to_lowercase();
        let is_display = lower.contains("vga compatible controller")
            || lower.contains("display controller")
            || lower.contains("3d controller");
        if !is_display {
            continue;
        }
        // The AMD device is the bracketed id whose vendor is 1002.
        let Some(cap) = pci_id_re()
            .captures_iter(line)
            .find(|c| c[1].eq_ignore_ascii_case("1002"))
        else {
            continue;
        };
        let pci_id = format!("{}:{}", cap[1].to_lowercase(), cap[2].to_lowercase());
        let model = extract_model(line).unwrap_or_else(|| pci_id.clone());
        let mut gpu = GpuInfo::new(model, pci_id);
        gpu.pci_addr = line.split_whitespace().next().and_then(normalize_pci_addr);
        gpus.push(gpu);
    }
    gpus
}

/// Extract a readable model name from an `lspci` line: the text between the
/// `[AMD/ATI]` vendor marker and the trailing `[1002:xxxx]` id.
fn extract_model(line: &str) -> Option<String> {
    let after = line.split("[AMD/ATI]").nth(1)?;
    // The AMD vendor id marker "[1002:" is pure ASCII, so search the original
    // string directly. Slicing by an offset found in a `to_lowercase()` copy is a
    // panic waiting to happen on non-ASCII input, where lowercasing can change the
    // byte length and push the offset off a char boundary.
    let end = after.find("[1002:").unwrap_or(after.len());
    let model = after.get(..end)?.trim();
    if model.is_empty() {
        None
    } else {
        Some(model.to_string())
    }
}

/// One GPU agent as `rocminfo` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RocmAgent {
    /// Architecture token, lowercased, e.g. `"gfx1103"`.
    pub gfx: String,
    /// `Chip ID` — the PCI *device* id, which pairs an agent with an `lspci` line.
    pub chip_id: Option<u16>,
    /// Decoded `BDFID`, the other correlation key.
    pub bdf: Option<Bdf>,
}

/// Extract every GPU agent from `rocminfo` output, in report order.
///
/// CPU agents are skipped: `rocminfo` lists the host CPU as agent 1 and its
/// `Name` is a marketing string, not a `gfx` token.
pub fn parse_rocminfo_agents(output: &str) -> Vec<RocmAgent> {
    let mut agents = Vec::new();
    let mut block = AgentBlock::default();

    for line in output.lines() {
        let t = line.trim();
        // `Agent N` headers delimit blocks.
        if let Some(rest) = t.strip_prefix("Agent ") {
            if rest.trim().parse::<u32>().is_ok() {
                block.flush_into(&mut agents);
                continue;
            }
        }
        let Some((key, value)) = t.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            // Only a bare gfx token is the architecture; "Marketing Name" is prose.
            "Name" => {
                if gfx_re().find(value).is_some_and(|m| m.len() == value.len()) {
                    block.gfx = Some(value.to_lowercase());
                }
            }
            "Device Type" => block.is_gpu = value.eq_ignore_ascii_case("GPU"),
            "Chip ID" => block.chip_id = parse_chip_id(value),
            "BDFID" => block.bdf = value.parse::<u32>().ok().map(Bdf::from_bdfid),
            _ => {}
        }
    }
    block.flush_into(&mut agents);
    agents
}

/// Fields accumulated while walking one `rocminfo` agent block.
#[derive(Default)]
struct AgentBlock {
    gfx: Option<String>,
    is_gpu: bool,
    chip_id: Option<u16>,
    bdf: Option<Bdf>,
}

impl AgentBlock {
    /// Emit the block if it described a GPU, then reset for the next one. A
    /// block without a `gfx` token is a CPU agent or a header artefact.
    fn flush_into(&mut self, agents: &mut Vec<RocmAgent>) {
        let done = std::mem::take(self);
        if let (true, Some(gfx)) = (done.is_gpu, done.gfx) {
            agents.push(RocmAgent {
                gfx,
                chip_id: done.chip_id,
                bdf: done.bdf,
            });
        }
    }
}

/// Parse a `Chip ID` value: `rocminfo` renders it as `29663(0x73df)`.
fn parse_chip_id(value: &str) -> Option<u16> {
    if let Some(open) = value.find("(0x") {
        let hex = value[open + 3..].trim_end_matches(')');
        if let Ok(v) = u32::from_str_radix(hex.trim(), 16) {
            return Some(v as u16);
        }
    }
    value
        .split_whitespace()
        .next()?
        .parse::<u32>()
        .ok()
        .map(|v| v as u16)
}

/// Attach each agent's architecture to the GPU it actually belongs to.
///
/// The three sources — `lspci`, `rocminfo`, `/sys/class/drm` — order cards
/// independently, so pairing them by position is guesswork that happens to be
/// right on single-GPU boxes. Matching is therefore by key, strongest first:
///
/// 1. `Chip ID` against the PCI device id (skipped where either side is
///    ambiguous, as it is for two identical cards).
/// 2. `BDFID` against the PCI address.
/// 3. A single leftover on each side, which is unambiguous by elimination.
///
/// Anything still unmatched keeps `gfx_arch = None` and classifies as Tier 3.
/// That is the honest outcome: a conservative tier costs performance, while a
/// wrong architecture produces a plan for a card that is not there.
///
/// Returns the number of GPUs that were given an architecture.
pub fn correlate_rocm_agents(gpus: &mut [GpuInfo], agents: &[RocmAgent]) -> usize {
    let mut taken = vec![false; agents.len()];
    let mut matched = 0usize;

    // Device ids up front: a card only identifies itself by chip id if no other
    // card in the machine shares that id. Two identical cards do.
    let dev_ids: Vec<Option<u16>> = gpus.iter().map(|g| pci_device_id(&g.pci_id)).collect();
    let distinctive: Vec<Option<u16>> = dev_ids
        .iter()
        .map(|d| d.filter(|_| dev_ids.iter().filter(|o| *o == d).count() == 1))
        .collect();

    // Pass 1: chip id, only where the value is unique on both sides.
    for (gi, gpu) in gpus.iter_mut().enumerate() {
        if gpu.gfx_arch.is_some() {
            continue;
        }
        let Some(dev_id) = distinctive[gi] else {
            continue;
        };
        let hits: Vec<usize> = (0..agents.len())
            .filter(|&ai| !taken[ai] && agents[ai].chip_id == Some(dev_id))
            .collect();
        if let [ai] = hits[..] {
            gpu.gfx_arch = Some(agents[ai].gfx.clone());
            taken[ai] = true;
            matched += 1;
        }
    }

    // Pass 2: PCI bus/device/function.
    for gpu in gpus.iter_mut() {
        if gpu.gfx_arch.is_some() {
            continue;
        }
        let Some(bdf) = gpu.pci_addr.as_deref().and_then(Bdf::parse) else {
            continue;
        };
        let hits: Vec<usize> = (0..agents.len())
            .filter(|&ai| !taken[ai] && agents[ai].bdf == Some(bdf))
            .collect();
        if let [ai] = hits[..] {
            gpu.gfx_arch = Some(agents[ai].gfx.clone());
            taken[ai] = true;
            matched += 1;
        }
    }

    // Pass 3: exactly one unclaimed card and one unclaimed agent. This is what
    // keeps the ordinary single-GPU box working when `rocminfo` is too old to
    // report either key.
    let free_gpus: Vec<usize> = (0..gpus.len())
        .filter(|&i| gpus[i].gfx_arch.is_none())
        .collect();
    let free_agents: Vec<usize> = (0..agents.len()).filter(|&i| !taken[i]).collect();
    if let ([gi], [ai]) = (&free_gpus[..], &free_agents[..]) {
        gpus[*gi].gfx_arch = Some(agents[*ai].gfx.clone());
        matched += 1;
    }

    matched
}

/// The device half of a `vendor:device` PCI id.
fn pci_device_id(pci_id: &str) -> Option<u16> {
    u16::from_str_radix(pci_id.split(':').nth(1)?, 16).ok()
}

/// Extract the first AMD `gfxNNNN` architecture token from `rocminfo` output.
///
/// Kept for callers that only need "is there any ROCm agent at all". Prefer
/// [`parse_rocminfo_agents`] + [`correlate_rocm_agents`] for anything that
/// assigns an architecture to a specific card.
pub fn parse_rocminfo_gfx(output: &str) -> Option<String> {
    parse_rocminfo_agents(output)
        .first()
        .map(|a| a.gfx.clone())
        .or_else(|| gfx_re().find(output).map(|m| m.as_str().to_lowercase()))
}

/// Parse the contents of `mem_info_vram_total` (a byte count) into MiB.
pub fn parse_vram_mib(sysfs_contents: &str) -> Option<u64> {
    sysfs_contents
        .trim()
        .parse::<u64>()
        .ok()
        .map(|bytes| bytes / (1024 * 1024))
}

/// Classify `mem_info_vram_type` into [`MemoryKind`].
///
/// An APU reports the system DRAM type (`DDR5`, `LPDDR5`) because that is
/// literally what backs its carve-out; a discrete card reports `GDDR*`/`HBM*`.
/// GDDR is checked first — `"GDDR6"` contains `"DDR6"`.
pub fn parse_memory_kind(sysfs_contents: &str) -> MemoryKind {
    let t = sysfs_contents.trim().to_uppercase();
    if t.is_empty() || t == "UNKNOWN" {
        return MemoryKind::Unknown;
    }
    if t.starts_with("GDDR") || t.starts_with("HBM") {
        return MemoryKind::Dedicated;
    }
    if t.starts_with("DDR") || t.starts_with("LPDDR") {
        return MemoryKind::Shared;
    }
    MemoryKind::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_audio_function_keeps_gpu() {
        let txt = "\
0a:00.0 VGA compatible controller [0300]: Advanced Micro Devices, Inc. [AMD/ATI] Navi 22 [Radeon RX 6700 XT] [1002:73df] (rev c5)
0a:00.1 Audio device [0403]: Advanced Micro Devices, Inc. [AMD/ATI] Navi 21/23 HDMI/DP Audio Controller [1002:ab28]";
        let gpus = parse_lspci(txt);
        assert_eq!(gpus.len(), 1);
        assert_eq!(gpus[0].pci_id, "1002:73df");
        assert_eq!(gpus[0].pci_addr.as_deref(), Some("0000:0a:00.0"));
        assert!(gpus[0].model.contains("Radeon RX 6700 XT"));
    }

    #[test]
    fn non_ascii_before_id_does_not_panic() {
        // 'İ' (U+0130) lowercases to a longer byte sequence; the parser must not
        // slice by a mismatched offset. This used to panic.
        let txt = "0a:00.0 VGA compatible controller [0300]: Advanced Micro Devices, Inc. [AMD/ATI] İNavi [1002:73df]";
        let gpus = parse_lspci(txt);
        assert_eq!(gpus.len(), 1);
        assert_eq!(gpus[0].pci_id, "1002:73df");
    }

    #[test]
    fn skips_non_amd_display() {
        let txt = "01:00.0 VGA compatible controller [0300]: NVIDIA Corporation GA104 [1de:2484] (rev a1)";
        assert!(parse_lspci(txt).is_empty());
    }

    #[test]
    fn reads_gfx_from_rocminfo() {
        let txt = "Agent 2\n  Name:                    gfx1030\n  Device Type:             GPU";
        assert_eq!(parse_rocminfo_gfx(txt).as_deref(), Some("gfx1030"));
    }

    #[test]
    fn vram_bytes_to_mib() {
        assert_eq!(parse_vram_mib("12884901888\n"), Some(12288));
    }

    #[test]
    fn bdfid_decodes_to_bus_device_function() {
        // 0a:00.0 packs as bus 0x0a << 8.
        assert_eq!(
            Bdf::from_bdfid(2560),
            Bdf {
                bus: 0x0a,
                device: 0,
                function: 0
            }
        );
        assert_eq!(Bdf::parse("0000:0a:00.0"), Some(Bdf::from_bdfid(2560)));
        assert_eq!(Bdf::parse("0a:00.0"), Some(Bdf::from_bdfid(2560)));
    }

    #[test]
    fn address_normalises_to_include_domain() {
        assert_eq!(
            normalize_pci_addr("0a:00.0").as_deref(),
            Some("0000:0a:00.0")
        );
        assert_eq!(
            normalize_pci_addr("0000:0A:00.0").as_deref(),
            Some("0000:0a:00.0")
        );
        assert!(normalize_pci_addr("not-an-address").is_none());
    }

    const APU_PLUS_DGPU_ROCMINFO: &str = "\
Agent 1
  Name:                    AMD Ryzen 9 7940HS
  Device Type:             CPU
Agent 2
  Name:                    gfx1103
  Marketing Name:          AMD Radeon 780M Graphics
  Device Type:             GPU
  Chip ID:                 5567(0x15bf)
  BDFID:                   50432
Agent 3
  Name:                    gfx1100
  Marketing Name:          AMD Radeon RX 7900 XTX
  Device Type:             GPU
  Chip ID:                 29772(0x744c)
  BDFID:                   768
";

    #[test]
    fn parses_every_gpu_agent_and_skips_the_cpu() {
        let agents = parse_rocminfo_agents(APU_PLUS_DGPU_ROCMINFO);
        assert_eq!(agents.len(), 2);
        assert_eq!(agents[0].gfx, "gfx1103");
        assert_eq!(agents[0].chip_id, Some(0x15bf));
        assert_eq!(agents[1].gfx, "gfx1100");
        assert_eq!(agents[1].bdf, Some(Bdf::from_bdfid(768)));
    }

    #[test]
    fn each_card_gets_its_own_architecture() {
        // The dGPU is listed first by lspci and second by rocminfo: any
        // positional pairing labels the 7900 XTX as an iGPU.
        let lspci = "\
03:00.0 VGA compatible controller [0300]: Advanced Micro Devices, Inc. [AMD/ATI] Navi 31 [Radeon RX 7900 XTX] [1002:744c] (rev cc)
c5:00.0 VGA compatible controller [0300]: Advanced Micro Devices, Inc. [AMD/ATI] Phoenix1 [1002:15bf] (rev cc)";
        let mut gpus = parse_lspci(lspci);
        let n = correlate_rocm_agents(&mut gpus, &parse_rocminfo_agents(APU_PLUS_DGPU_ROCMINFO));
        assert_eq!(n, 2);
        assert_eq!(gpus[0].gfx_arch.as_deref(), Some("gfx1100"));
        assert_eq!(gpus[1].gfx_arch.as_deref(), Some("gfx1103"));
    }

    #[test]
    fn identical_cards_are_separated_by_address_not_chip_id() {
        let lspci = "\
03:00.0 VGA compatible controller [0300]: Advanced Micro Devices, Inc. [AMD/ATI] Navi 31 [Radeon RX 7900 XTX] [1002:744c] (rev cc)
0a:00.0 VGA compatible controller [0300]: Advanced Micro Devices, Inc. [AMD/ATI] Navi 31 [Radeon RX 7900 XTX] [1002:744c] (rev cc)";
        let mut gpus = parse_lspci(lspci);
        let agents = vec![
            RocmAgent {
                gfx: "gfx1100".into(),
                chip_id: Some(0x744c),
                bdf: Some(Bdf::parse("0a:00.0").unwrap()),
            },
            RocmAgent {
                gfx: "gfx1100".into(),
                chip_id: Some(0x744c),
                bdf: Some(Bdf::parse("03:00.0").unwrap()),
            },
        ];
        assert_eq!(correlate_rocm_agents(&mut gpus, &agents), 2);
        assert!(gpus
            .iter()
            .all(|g| g.gfx_arch.as_deref() == Some("gfx1100")));
    }

    #[test]
    fn single_card_still_matches_without_any_key() {
        let lspci = "0a:00.0 VGA compatible controller [0300]: Advanced Micro Devices, Inc. [AMD/ATI] Navi 22 [Radeon RX 6700 XT] [1002:73df]";
        let mut gpus = parse_lspci(lspci);
        let agents = vec![RocmAgent {
            gfx: "gfx1030".into(),
            chip_id: None,
            bdf: None,
        }];
        assert_eq!(correlate_rocm_agents(&mut gpus, &agents), 1);
        assert_eq!(gpus[0].gfx_arch.as_deref(), Some("gfx1030"));
    }

    #[test]
    fn unmatchable_cards_stay_unknown_rather_than_guessing() {
        // Two cards, two keyless agents: nothing can be attributed, and
        // guessing would mislabel both.
        let lspci = "\
03:00.0 VGA compatible controller [0300]: Advanced Micro Devices, Inc. [AMD/ATI] Navi 31 [Radeon RX 7900 XTX] [1002:744c]
c5:00.0 VGA compatible controller [0300]: Advanced Micro Devices, Inc. [AMD/ATI] Phoenix1 [1002:15bf]";
        let mut gpus = parse_lspci(lspci);
        let agents = vec![
            RocmAgent {
                gfx: "gfx1100".into(),
                chip_id: None,
                bdf: None,
            },
            RocmAgent {
                gfx: "gfx1103".into(),
                chip_id: None,
                bdf: None,
            },
        ];
        assert_eq!(correlate_rocm_agents(&mut gpus, &agents), 0);
        assert!(gpus.iter().all(|g| g.gfx_arch.is_none()));
    }

    #[test]
    fn vram_type_separates_apu_from_discrete() {
        assert_eq!(parse_memory_kind("GDDR6\n"), MemoryKind::Dedicated);
        assert_eq!(parse_memory_kind("HBM2E"), MemoryKind::Dedicated);
        assert_eq!(parse_memory_kind("DDR5\n"), MemoryKind::Shared);
        assert_eq!(parse_memory_kind("LPDDR5"), MemoryKind::Shared);
        assert_eq!(parse_memory_kind("unknown"), MemoryKind::Unknown);
    }
}
