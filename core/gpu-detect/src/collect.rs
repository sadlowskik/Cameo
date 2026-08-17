//! Linux-only collection: gather raw `lspci` / `rocminfo` / sysfs data and feed
//! it through the pure parsers. On non-Linux hosts this returns
//! [`Error::UnsupportedOs`] — develop and test against captured fixtures instead.

use crate::error::Error;
use crate::topology::Topology;
use crate::types::GpuInfo;

/// Detect the full multi-GPU topology on the current machine (Linux only).
#[cfg(target_os = "linux")]
pub fn collect_topology() -> Result<Topology, Error> {
    use crate::topology::parse_rocm_smi_topo;
    use std::process::Command;

    let gpus = collect()?;
    let links = Command::new("rocm-smi")
        .arg("--showtopo")
        .output()
        .ok()
        .map(|o| parse_rocm_smi_topo(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default();
    Ok(Topology::new(gpus, links))
}

/// Non-Linux stub for [`collect_topology`].
#[cfg(not(target_os = "linux"))]
pub fn collect_topology() -> Result<Topology, Error> {
    Err(Error::UnsupportedOs)
}

/// Detect AMD GPUs on the current machine (Linux only).
#[cfg(target_os = "linux")]
pub fn collect() -> Result<Vec<GpuInfo>, Error> {
    use crate::parse;
    use std::process::Command;

    let lspci = Command::new("lspci").arg("-nn").output()?;
    let lspci_txt = String::from_utf8_lossy(&lspci.stdout);
    let mut gpus = parse::parse_lspci(&lspci_txt);
    if gpus.is_empty() {
        return Err(Error::NoGpu);
    }

    // Best-effort gfx architecture from rocminfo. Its absence is meaningful:
    // it means no usable ROCm stack, which the classifier reads as Tier 3.
    if let Ok(out) = Command::new("rocminfo").output() {
        let txt = String::from_utf8_lossy(&out.stdout);
        if let Some(gfx) = parse::parse_rocminfo_gfx(&txt) {
            for g in &mut gpus {
                g.gfx_arch.get_or_insert_with(|| gfx.clone());
            }
        }
    }

    // Best-effort VRAM from sysfs, matching card index to detection order.
    for (idx, g) in gpus.iter_mut().enumerate() {
        let path = format!("/sys/class/drm/card{idx}/device/mem_info_vram_total");
        if let Ok(contents) = std::fs::read_to_string(&path) {
            g.vram_mb = parse::parse_vram_mib(&contents);
        }
    }

    Ok(gpus)
}

/// Non-Linux stub: collection needs Linux sysfs and tools. Feed captured text to
/// [`crate::parse`] instead.
#[cfg(not(target_os = "linux"))]
pub fn collect() -> Result<Vec<GpuInfo>, Error> {
    Err(Error::UnsupportedOs)
}
