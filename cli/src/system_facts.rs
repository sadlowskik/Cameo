//! Privacy-allowlisted host facts shared by support and validation reports.

use serde::{Deserialize, Serialize};

use cameo_gpu_detect::MemoryKind;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SystemFacts {
    pub platform: PlatformFacts,
    pub artifact: ArtifactFacts,
    pub hardware: HardwareFacts,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PlatformFacts {
    pub os: &'static str,
    pub architecture: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kernel_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_model: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct ArtifactFacts {
    pub cameo_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iso_build_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edition: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_dirty: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub knossos_revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arch_snapshot: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rocm_cli_version: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct HardwareFacts {
    pub status: &'static str,
    pub gpus: Vec<GpuFacts>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_ram_total_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_ram_available_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct GpuFacts {
    pub model: String,
    pub vendor: String,
    /// PCI vendor/device identity is useful for grouping but is not a serial.
    pub pci_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gfx_target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vram_total_bytes: Option<u64>,
    pub is_apu: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub driver_version: Option<String>,
    pub cameo_tier: u8,
}

#[derive(Debug, Default, Deserialize)]
struct InstalledBuild {
    #[serde(default)]
    cameo_version: String,
    iso_build_id: Option<String>,
    edition: Option<String>,
    source_revision: Option<String>,
    source_dirty: Option<bool>,
    knossos_revision: Option<String>,
    arch_snapshot: Option<String>,
    rocm_cli_version: Option<String>,
}

pub(crate) fn collect(cli: &super::Cli) -> SystemFacts {
    let (status, gpus, total, available) = match super::detect(cli) {
        Ok((topology, assessments)) => {
            let gpus = assessments
                .into_iter()
                .take(32)
                .map(|assessment| {
                    let vram_total_bytes = assessment.gpu.vram_bytes();
                    GpuFacts {
                        model: bounded_fact(assessment.gpu.model, 200),
                        vendor: assessment.gpu.vendor.label().to_string(),
                        pci_id: assessment.gpu.pci_id,
                        gfx_target: clean_gfx_target(assessment.gpu.gfx_arch),
                        vram_total_bytes,
                        is_apu: assessment.gpu.memory == MemoryKind::Shared,
                        driver_version: clean_fact(assessment.gpu.driver_version, 128),
                        cameo_tier: assessment.tier.as_number(),
                    }
                })
                .collect();
            let (total, available) = topology
                .host_mem
                .map(|memory| (Some(memory.total_bytes), Some(memory.available_bytes)))
                .unwrap_or_default();
            ("observed", gpus, total, available)
        }
        Err(_) => ("unverified", Vec::new(), None, None),
    };

    SystemFacts {
        platform: PlatformFacts {
            os: std::env::consts::OS,
            architecture: std::env::consts::ARCH,
            kernel_version: kernel_version(),
            cpu_model: cpu_model(),
        },
        artifact: installed_build(),
        hardware: HardwareFacts {
            status,
            gpus,
            system_ram_total_bytes: total,
            system_ram_available_bytes: available,
        },
    }
}

fn installed_build() -> ArtifactFacts {
    let fallback = ArtifactFacts {
        cameo_version: env!("CARGO_PKG_VERSION").to_string(),
        ..ArtifactFacts::default()
    };
    let Ok(contents) = std::fs::read_to_string("/etc/cameo/build.json") else {
        return fallback;
    };
    let Ok(installed) = serde_json::from_str::<InstalledBuild>(&contents) else {
        return fallback;
    };
    ArtifactFacts {
        cameo_version: clean_fact(Some(installed.cameo_version), 64)
            .unwrap_or(fallback.cameo_version),
        iso_build_id: clean_fact(installed.iso_build_id, 128),
        edition: clean_fact(installed.edition, 32),
        source_revision: clean_fact(installed.source_revision, 64),
        source_dirty: installed.source_dirty,
        knossos_revision: clean_fact(installed.knossos_revision, 64),
        arch_snapshot: clean_fact(installed.arch_snapshot, 32),
        rocm_cli_version: clean_fact(installed.rocm_cli_version, 64),
    }
}

fn clean_fact(value: Option<String>, max: usize) -> Option<String> {
    value.map(|value| value.trim().to_string()).filter(|value| {
        !value.is_empty() && value.chars().count() <= max && !value.chars().any(char::is_control)
    })
}

fn bounded_fact(value: String, max: usize) -> String {
    let value = value.trim();
    if value.is_empty() || value.chars().any(char::is_control) {
        return "unknown".to_string();
    }
    value.chars().take(max).collect()
}

fn clean_gfx_target(value: Option<String>) -> Option<String> {
    clean_fact(value, 32).filter(|value| {
        value.starts_with("gfx")
            && value.len() > 3
            && value[3..]
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    })
}

#[cfg(target_os = "linux")]
fn kernel_version() -> Option<String> {
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .ok()
        .and_then(|value| clean_fact(Some(value), 128))
}

#[cfg(not(target_os = "linux"))]
fn kernel_version() -> Option<String> {
    None
}

#[cfg(target_os = "linux")]
fn cpu_model() -> Option<String> {
    let contents = std::fs::read_to_string("/proc/cpuinfo").ok()?;
    contents.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        matches!(key.trim(), "model name" | "Hardware")
            .then(|| clean_fact(Some(value.to_string()), 160))
            .flatten()
    })
}

#[cfg(target_os = "windows")]
fn cpu_model() -> Option<String> {
    clean_fact(std::env::var("PROCESSOR_IDENTIFIER").ok(), 160)
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn cpu_model() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_facts_reject_controls_and_excessive_values() {
        assert_eq!(
            clean_fact(Some("  beta.4  ".into()), 16).as_deref(),
            Some("beta.4")
        );
        assert!(clean_fact(Some("bad\nvalue".into()), 32).is_none());
        assert!(clean_fact(Some("x".repeat(33)), 32).is_none());
    }
}
