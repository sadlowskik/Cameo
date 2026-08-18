//! Cameo configuration and the project-wide override precedence rule.
//!
//! Every "smart" auto-detected default in Cameo must be overridable. The
//! precedence, from lowest to highest, is:
//!
//! ```text
//!   auto-detected  <  config file  <  CLI flag / env
//! ```
//!
//! [`Settings`] is a bag of `Option`s so each layer only sets what it knows;
//! [`Settings::overlay`] merges a higher-priority layer on top, and
//! [`resolve`] applies the full precedence chain.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Which inference/training backend to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    /// Pick automatically from the detected GPU tier.
    Auto,
    /// Force the Vulkan backend (works on any tier).
    Vulkan,
    /// Force the ROCm backend (Tier 1/2 only).
    Rocm,
}

/// A single configuration layer. Unset (`None`) fields defer to lower layers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Backend selection.
    pub backend: Option<Backend>,
    /// Explicit `HSA_OVERRIDE_GFX_VERSION` value, overriding tier detection.
    pub hsa_override: Option<String>,
    /// Directory to search for / download models into.
    pub model_dir: Option<PathBuf>,
    /// Unix socket path for the core API service.
    pub socket_path: Option<PathBuf>,
    /// Plan a model even when it exceeds VRAM + host RAM. Off by default: the
    /// planner refuses rather than emitting a command the kernel will OOM-kill.
    pub allow_oversize: Option<bool>,
    /// API key `llama-server` requires from clients. Serving on anything other
    /// than loopback is refused without one.
    pub serve_api_key: Option<String>,
}

impl Settings {
    /// Parse a settings layer from TOML text.
    pub fn from_toml(text: &str) -> Result<Self, Error> {
        Ok(toml::from_str(text)?)
    }

    /// Load a settings layer from a file. A missing file yields defaults (all
    /// `None`) rather than an error — config is optional.
    pub fn load_file(path: &Path) -> Result<Self, Error> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::from_toml(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(Error::Io(e)),
        }
    }

    /// Overlay `higher` on top of `self`: any field set in `higher` wins.
    #[must_use]
    pub fn overlay(mut self, higher: Settings) -> Settings {
        if higher.backend.is_some() {
            self.backend = higher.backend;
        }
        if higher.hsa_override.is_some() {
            self.hsa_override = higher.hsa_override;
        }
        if higher.model_dir.is_some() {
            self.model_dir = higher.model_dir;
        }
        if higher.allow_oversize.is_some() {
            self.allow_oversize = higher.allow_oversize;
        }
        if higher.serve_api_key.is_some() {
            self.serve_api_key = higher.serve_api_key;
        }
        if higher.socket_path.is_some() {
            self.socket_path = higher.socket_path;
        }
        self
    }
}

/// Apply the full precedence chain: `auto` (lowest) < `file` < `flags` (highest).
#[must_use]
pub fn resolve(auto: Settings, file: Settings, flags: Settings) -> Settings {
    auto.overlay(file).overlay(flags)
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("failed to parse config TOML: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("failed to read config file: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_beat_file_beat_auto() {
        let auto = Settings {
            backend: Some(Backend::Auto),
            hsa_override: None,
            model_dir: Some(PathBuf::from("/var/lib/cameo/models")),
            socket_path: None,
            ..Default::default()
        };
        let file = Settings {
            backend: Some(Backend::Rocm),
            hsa_override: Some("10.3.0".into()),
            ..Default::default()
        };
        let flags = Settings {
            backend: Some(Backend::Vulkan),
            ..Default::default()
        };

        let r = resolve(auto, file, flags);
        assert_eq!(r.backend, Some(Backend::Vulkan)); // flag wins
        assert_eq!(r.hsa_override.as_deref(), Some("10.3.0")); // from file
        assert_eq!(r.model_dir, Some(PathBuf::from("/var/lib/cameo/models"))); // from auto
    }

    #[test]
    fn missing_field_defers_downward() {
        let base = Settings {
            backend: Some(Backend::Vulkan),
            ..Default::default()
        };
        let empty = Settings::default();
        assert_eq!(base.clone().overlay(empty).backend, Some(Backend::Vulkan));
    }

    #[test]
    fn parses_toml() {
        let s = Settings::from_toml("backend = \"rocm\"\nhsa_override = \"10.3.0\"\n").unwrap();
        assert_eq!(s.backend, Some(Backend::Rocm));
        assert_eq!(s.hsa_override.as_deref(), Some("10.3.0"));
    }
}
