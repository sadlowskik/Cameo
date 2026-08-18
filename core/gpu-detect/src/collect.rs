//! Linux-only collection: gather raw `lspci` / `rocminfo` / sysfs data and feed
//! it through the pure parsers. On non-Linux hosts this returns
//! [`Error::UnsupportedOs`] — develop and test against captured fixtures instead.
//!
//! ⚠️ **This module is a hardware boundary.** It is not the *execution* boundary
//! (that is `cameo_placement::command::execute`), but it does shell out to
//! `lspci`, `rocminfo` and `rocm-smi` and read `/sys` and `/proc`. Everything it
//! produces is derived from a live machine; everything above it is pure. See
//! `docs/architecture.md`.

use crate::error::Error;
use crate::topology::Topology;
use crate::types::GpuInfo;

/// Detect the full multi-GPU topology on the current machine (Linux only).
#[cfg(target_os = "linux")]
pub fn collect_topology() -> Result<Topology, Error> {
    use crate::hostmem::parse_meminfo;
    use crate::topology::parse_rocm_smi_topo;
    use std::process::Command;

    let gpus = collect()?;
    let links = Command::new("rocm-smi")
        .arg("--showtopo")
        .output()
        .ok()
        .map(|o| parse_rocm_smi_topo(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default();
    let host_mem = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|t| parse_meminfo(&t));
    Ok(Topology::new(gpus, links).with_host_memory(host_mem))
}

/// Non-Linux stub for [`collect_topology`].
#[cfg(not(target_os = "linux"))]
pub fn collect_topology() -> Result<Topology, Error> {
    Err(Error::UnsupportedOs)
}

/// Read host RAM from `/proc/meminfo` on its own — used by the CPU-only fallback,
/// which needs the memory figure even when GPU detection found nothing to size a
/// GPU plan against. `None` off Linux or when the file is unreadable.
#[cfg(target_os = "linux")]
pub fn live_host_memory() -> Option<crate::hostmem::HostMemory> {
    std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|t| crate::hostmem::parse_meminfo(&t))
}

/// Non-Linux stub for [`live_host_memory`].
#[cfg(not(target_os = "linux"))]
pub fn live_host_memory() -> Option<crate::hostmem::HostMemory> {
    None
}

/// Detect AMD GPUs on the current machine (Linux only).
#[cfg(target_os = "linux")]
pub fn collect() -> Result<Vec<GpuInfo>, Error> {
    use crate::parse;
    use std::process::Command;

    // `-D` forces the PCI domain into every address. Without it `lspci` omits
    // the domain on single-domain machines, and sysfs never does — so the two
    // would not compare equal on the one field that correlates them.
    let lspci = Command::new("lspci").args(["-D", "-nn"]).output()?;
    let lspci_txt = String::from_utf8_lossy(&lspci.stdout);
    let mut gpus = parse::parse_lspci(&lspci_txt);
    if gpus.is_empty() {
        return Err(Error::NoGpu);
    }

    // Architecture per card, from rocminfo. Its absence is meaningful: it means
    // no usable ROCm stack, which the classifier reads as Tier 3. Its *presence*
    // is per-agent, and agents are matched to cards by key — never by position,
    // which on an APU + dGPU box attributes the iGPU's architecture to the
    // discrete card and silently misclassifies it.
    if let Ok(out) = Command::new("rocminfo").output() {
        let agents = parse::parse_rocminfo_agents(&String::from_utf8_lossy(&out.stdout));
        parse::correlate_rocm_agents(&mut gpus, &agents);
    }

    let driver_version = read_trimmed("/sys/module/amdgpu/version");
    for g in &mut gpus {
        g.driver_version.clone_from(&driver_version);
        read_drm_memory(g);
    }

    Ok(gpus)
}

/// Fill in a GPU's memory facts from its own DRM node.
///
/// The node is found by PCI address, never by index: `/sys/class/drm/cardN`
/// numbering is assigned in driver-probe order and has no relationship to
/// `lspci` ordering, so `card{i}` for the i-th detected GPU reads a different
/// card's memory on any machine with more than one.
#[cfg(target_os = "linux")]
fn read_drm_memory(gpu: &mut GpuInfo) {
    use crate::parse;

    let Some(base) = drm_node_for(gpu) else {
        return;
    };
    if let Some(t) = read_trimmed(base.join("mem_info_vram_total")) {
        gpu.vram_mb = parse::parse_vram_mib(&t);
    }
    if let Some(t) = read_trimmed(base.join("mem_info_gtt_total")) {
        gpu.gtt_mb = parse::parse_vram_mib(&t);
    }
    if let Some(t) = read_trimmed(base.join("mem_info_vram_type")) {
        gpu.memory = parse::parse_memory_kind(&t);
    }
}

/// The `/sys/class/drm/cardN/device` directory belonging to this GPU, matched
/// by the PCI address the `device` symlink resolves to.
#[cfg(target_os = "linux")]
fn drm_node_for(gpu: &GpuInfo) -> Option<std::path::PathBuf> {
    use crate::parse::normalize_pci_addr;

    let want = gpu.pci_addr.as_deref()?;
    for entry in std::fs::read_dir("/sys/class/drm").ok()?.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // Connectors (`card0-DP-1`) live here too; only bare cards have a device.
        if !name.starts_with("card") || !name[4..].chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let device = entry.path().join("device");
        let Ok(target) = std::fs::canonicalize(&device) else {
            continue;
        };
        let addr = target
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .and_then(|s| normalize_pci_addr(&s));
        if addr.as_deref() == Some(want) {
            return Some(device);
        }
    }
    None
}

/// Read a sysfs file, trimmed. Absent or unreadable is not an error — every
/// caller treats a missing fact as "unknown" and plans conservatively.
#[cfg(target_os = "linux")]
fn read_trimmed(path: impl AsRef<std::path::Path>) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Non-Linux stub: collection needs Linux sysfs and tools. Feed captured text to
/// [`crate::parse`] instead.
#[cfg(not(target_os = "linux"))]
pub fn collect() -> Result<Vec<GpuInfo>, Error> {
    Err(Error::UnsupportedOs)
}
