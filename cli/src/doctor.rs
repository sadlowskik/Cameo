//! Allowlisted support facts: no environment dump, credentials, prompts, logs,
//! model paths, hostnames, or command output are included in an export.
use std::io::Write;
use std::path::Path;

use anyhow::{bail, Result};
use serde_json::json;

pub fn run(cli: &super::Cli, bundle: Option<&Path>) -> Result<()> {
    let hardware = match super::detect(cli) {
        Ok((topology, assessments)) => json!({
            "status": "observed",
            "device_count": topology.gpus.len(),
            "assessments": assessments,
        }),
        Err(_) => {
            json!({"status": "unverified", "action": "Run on the Linux appliance, or provide hardware captures using the documented fixture flags."})
        }
    };
    let settings = super::settings_from(cli, None);
    let configuration = match settings {
        Ok(settings) => {
            json!({"status":"parsed", "serve_key_configured": settings.serve_api_key.is_some()})
        }
        Err(_) => {
            json!({"status":"invalid", "action":"Check the configured TOML file and CLI overrides locally."})
        }
    };
    let report = json!({
        "schema_version": "cameo-doctor/v1",
        "version": env!("CARGO_PKG_VERSION"),
        "platform": {"os": std::env::consts::OS, "architecture": std::env::consts::ARCH},
        "hardware": hardware,
        "configuration": configuration,
        "models": {"cached_count": cameo_models::cached_models().len(), "cached_bytes": cameo_models::cache_bytes(), "integrity": "not_checked"},
        "runtime": {"vulkan_binary_found": binary_found("llama-server"), "rocm_binary_found": binary_found("llama-server-rocm"), "inference": "not_tested"},
        "privacy": {"contents": "allowlisted support facts only", "prompts": false, "credentials": false, "paths": false, "raw_logs": false},
        "limitations": ["No live inference, daemon health, thermals, or hardware certification is implied.", "This JSON support bundle is not a backup of models, configuration, or identities."]
    });
    let output = serde_json::to_string_pretty(&report)?;
    // The exact export is always visible before any file is created.
    println!("{output}");
    if let Some(path) = bundle {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(path)?;
        if !file.metadata()?.is_file() {
            bail!("support export must be a regular file");
        }
        file.write_all(output.as_bytes())?;
        file.sync_all()?;
        eprintln!("Support report exported. Existing files are never overwritten.");
    }
    Ok(())
}

fn binary_found(name: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| {
        dir.join(name).is_file() || (cfg!(windows) && dir.join(format!("{name}.exe")).is_file())
    })
}
