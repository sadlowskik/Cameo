//! Pure parsers: text in, [`GpuInfo`] fields out. No I/O, testable anywhere.

use crate::types::GpuInfo;
use regex::Regex;
use std::sync::OnceLock;

/// Matches a bracketed PCI `vendor:device` id, e.g. `[1002:73df]`.
fn pci_id_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\[([0-9a-fA-F]{4}):([0-9a-fA-F]{4})\]").unwrap())
}

/// Matches an AMD architecture token, e.g. `gfx1030`, `gfx900`.
fn gfx_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"gfx[0-9a-fA-F]+").unwrap())
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
        gpus.push(GpuInfo {
            model,
            pci_id,
            vram_mb: None,
            gfx_arch: None,
            driver_version: None,
        });
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

/// Extract the first AMD `gfxNNNN` architecture token from `rocminfo` output.
///
/// Returns `None` when `rocminfo` is absent or produced no GPU agent — which is
/// itself the signal that the machine has no usable ROCm path (Tier 3).
pub fn parse_rocminfo_gfx(output: &str) -> Option<String> {
    gfx_re().find(output).map(|m| m.as_str().to_lowercase())
}

/// Parse the contents of `mem_info_vram_total` (a byte count) into MiB.
pub fn parse_vram_mib(sysfs_contents: &str) -> Option<u64> {
    sysfs_contents
        .trim()
        .parse::<u64>()
        .ok()
        .map(|bytes| bytes / (1024 * 1024))
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
        let txt = "  Name:                    gfx1030\n  Device Type:             GPU";
        assert_eq!(parse_rocminfo_gfx(txt).as_deref(), Some("gfx1030"));
    }

    #[test]
    fn vram_bytes_to_mib() {
        assert_eq!(parse_vram_mib("12884901888\n"), Some(12288));
    }
}
