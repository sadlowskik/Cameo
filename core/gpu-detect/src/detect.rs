//! Detection orchestration: one entry point that turns either a live machine or
//! a set of captured tool outputs into a [`Topology`].
//!
//! The pure parsers ([`parse`](crate::parse)), the memory facts
//! ([`memfacts`](crate::memfacts)) and the live collector
//! ([`collect`](crate::collect)) are the pieces; this module is the assembly
//! order that every caller needs and none should re-implement. Both the `cameo`
//! CLI and the `cameod` daemon drive detection through here, so the correlation
//! rules (per-card `rocminfo` matching, sysfs memory facts, host RAM) live in
//! exactly one place.
//!
//! Live detection is Linux-only; on any other host, hand it [`Captures`] read
//! from fixtures. Reading the files is the caller's job — this function takes
//! their contents as strings so it stays pure and OS-independent.

use crate::error::Error;
use crate::topology::Topology;

/// Captured tool outputs, standing in for a live machine on a dev host (or for
/// replaying a customer's hardware). Each field is the verbatim text a Cameo
/// operator would capture with the named command.
#[derive(Debug, Default, Clone)]
pub struct Captures {
    /// `lspci -D -nn`. Its presence is what switches detection into replay mode;
    /// when `None`, detection reads the live machine instead.
    pub lspci: Option<String>,
    /// `rocminfo`, for per-card architecture (and thus ROCm tiering).
    pub rocminfo: Option<String>,
    /// `rocm-smi --showtopo`, for inter-GPU links.
    pub topo: Option<String>,
    /// `/proc/meminfo`, for host-RAM sizing of offload plans.
    pub meminfo: Option<String>,
    /// Captured `/sys/class/drm` memory facts (TOML) — VRAM/GTT/type, which an
    /// `lspci` capture cannot carry.
    pub gpu_mem: Option<String>,
}

impl Captures {
    /// Whether detection will read the live machine (no `lspci` capture given).
    pub fn is_live(&self) -> bool {
        self.lspci.is_none()
    }
}

/// Detect the GPU topology: live (Linux) when `captures.lspci` is `None`, or by
/// replaying the given captures on any OS.
///
/// The replay path mirrors the live [`collect_topology`](crate::collect_topology)
/// step for step, so a fixture-driven run on a dev box exercises the same
/// assembly a real machine would.
pub fn detect_topology(captures: &Captures) -> Result<Topology, Error> {
    let Some(lspci_txt) = &captures.lspci else {
        return crate::collect::collect_topology();
    };

    use crate::{hostmem, memfacts, parse, topology};

    let mut gpus = parse::parse_lspci(lspci_txt);

    // Per-card correlation, not a broadcast: a `rocminfo` agent is matched to the
    // card it describes, so a mixed APU + dGPU box is not mislabelled.
    if let Some(rocminfo) = &captures.rocminfo {
        let agents = parse::parse_rocminfo_agents(rocminfo);
        parse::correlate_rocm_agents(&mut gpus, &agents);
    }

    // sysfs memory facts, which an lspci capture cannot carry.
    if let Some(gpu_mem) = &captures.gpu_mem {
        let facts = memfacts::parse_gpu_memory(gpu_mem)?;
        memfacts::apply_gpu_memory(&mut gpus, &facts);
    }

    if gpus.is_empty() {
        return Err(Error::NoGpu);
    }

    let links = captures
        .topo
        .as_deref()
        .map(topology::parse_rocm_smi_topo)
        .unwrap_or_default();
    let host_mem = captures.meminfo.as_deref().and_then(hostmem::parse_meminfo);

    Ok(Topology::new(gpus, links).with_host_memory(host_mem))
}

/// Detect the topology, treating "no AMD GPU" as the CPU-only case rather than
/// an error.
///
/// [`detect_topology`] fails with [`Error::NoGpu`] when there is no AMD display
/// device — the right answer for a tool that strictly needs a GPU. But Cameo
/// also runs models on the CPU, so most callers want that outcome to be a valid
/// (empty-GPU) topology with host RAM still recorded, not a hard failure. Every
/// other error (unreadable capture, malformed sysfs TOML, unsupported OS) still
/// propagates.
pub fn detect_topology_or_cpu(captures: &Captures) -> Result<Topology, Error> {
    match detect_topology(captures) {
        Err(Error::NoGpu) => Ok(Topology::cpu_only(host_memory_for(captures))),
        other => other,
    }
}

/// Host RAM for the CPU fallback: from the capture when replaying, else read live.
fn host_memory_for(captures: &Captures) -> Option<crate::hostmem::HostMemory> {
    match captures.meminfo.as_deref() {
        Some(text) => crate::hostmem::parse_meminfo(text),
        None => crate::collect::live_host_memory(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_captures_are_live() {
        assert!(Captures::default().is_live());
    }

    #[test]
    fn no_gpu_becomes_a_cpu_only_topology_with_ram() {
        // A machine with no AMD GPU is a valid CPU target, not an error — and the
        // host RAM figure must survive so the planner can size against it.
        let caps = Captures {
            lspci: Some("00:00.0 Host bridge [0600]: Not a GPU\n".into()),
            meminfo: Some("MemTotal: 16384000 kB\nMemAvailable: 12000000 kB\n".into()),
            ..Default::default()
        };
        let topo = detect_topology_or_cpu(&caps).expect("CPU fallback, not an error");
        assert!(topo.is_cpu_only());
        assert!(topo.gpus.is_empty());
        assert_eq!(topo.host_mem.unwrap().total_bytes, 16_384_000 * 1024);
    }

    #[test]
    fn a_real_gpu_is_unaffected_by_the_cpu_fallback() {
        let caps = Captures {
            lspci: Some(
                "03:00.0 VGA compatible controller [0300]: Advanced Micro Devices, \
                 Inc. [AMD/ATI] Navi 31 [Radeon RX 7900 XTX] [1002:744c]\n"
                    .into(),
            ),
            ..Default::default()
        };
        let topo = detect_topology_or_cpu(&caps).unwrap();
        assert!(!topo.is_cpu_only());
        assert_eq!(topo.gpus.len(), 1);
    }

    #[test]
    fn a_capture_with_no_gpus_is_an_error_not_a_panic() {
        let caps = Captures {
            lspci: Some("00:00.0 Host bridge [0600]: Not a GPU\n".into()),
            ..Default::default()
        };
        assert!(matches!(detect_topology(&caps), Err(Error::NoGpu)));
    }

    #[test]
    fn replays_a_captured_gpu_line() {
        // A minimal AMD VGA line is enough to prove the replay path parses and
        // returns a topology rather than falling through to live collection.
        let caps = Captures {
            lspci: Some(
                "03:00.0 VGA compatible controller [0300]: Advanced Micro Devices, \
                 Inc. [AMD/ATI] Navi 31 [Radeon RX 7900 XTX] [1002:744c]\n"
                    .into(),
            ),
            ..Default::default()
        };
        let topo = detect_topology(&caps).expect("replay should detect the GPU");
        assert_eq!(topo.gpus.len(), 1);
    }
}
