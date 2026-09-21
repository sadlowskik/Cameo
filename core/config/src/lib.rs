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
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Default port `knossos field --native` listens on.
pub const FIELD_DEFAULT_PORT: u16 = 7749;

/// The `[field]` layer: `cameod` reverse-proxies `/field/` to `knossos field`
/// on loopback so Field shares the console's TLS certificate, key and origin.
/// Every key is optional so the layer merges like the rest of [`Settings`];
/// [`FieldSettings::resolved`] applies the defaults and the loopback rule.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FieldSettings {
    /// Proxy `/field/` at all. Off by default: the daemon must not forward to a
    /// port nothing was asked to listen on.
    pub enabled: Option<bool>,
    /// The port `knossos field` listens on (default [`FIELD_DEFAULT_PORT`]).
    pub port: Option<u16>,
    /// Address Field listens on. Loopback only (default `127.0.0.1`): the whole
    /// point of the proxy is that Field is never exposed on the LAN itself.
    pub bind: Option<IpAddr>,
}

/// Fully resolved `[field]` settings (defaults applied, bind validated).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldConfig {
    pub enabled: bool,
    pub port: u16,
    pub bind: IpAddr,
}

impl FieldSettings {
    /// Overlay `higher` on top of `self`, key by key.
    #[must_use]
    pub fn overlay(mut self, higher: FieldSettings) -> FieldSettings {
        if higher.enabled.is_some() {
            self.enabled = higher.enabled;
        }
        if higher.port.is_some() {
            self.port = higher.port;
        }
        if higher.bind.is_some() {
            self.bind = higher.bind;
        }
        self
    }

    /// Apply defaults (`enabled = false`, `port = 7749`, `bind = 127.0.0.1`)
    /// and refuse a non-loopback `bind`.
    pub fn resolved(&self) -> Result<FieldConfig, Error> {
        let bind = self.bind.unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
        if !bind.is_loopback() {
            return Err(Error::FieldBindNotLoopback(bind));
        }
        Ok(FieldConfig {
            enabled: self.enabled.unwrap_or(false),
            port: self.port.unwrap_or(FIELD_DEFAULT_PORT),
            bind,
        })
    }
}

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
    /// CPU only: no GPU, the whole model runs in system RAM. The universal
    /// fallback — it works on any x86-64 machine, AMD or not, and is what
    /// Cameo selects automatically when no AMD GPU is present.
    Cpu,
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
    /// `[field]`: the `/field/` reverse proxy to `knossos field` (daemon only).
    pub field: FieldSettings,
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
        self.field = self.field.overlay(higher.field);
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
    #[error("[field] bind must be a loopback address, got {0}")]
    FieldBindNotLoopback(IpAddr),
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

    #[test]
    fn field_defaults_off_on_7749_loopback() {
        let field = Settings::default().field.resolved().unwrap();
        assert_eq!(
            field,
            FieldConfig {
                enabled: false,
                port: FIELD_DEFAULT_PORT,
                bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
            }
        );
    }

    #[test]
    fn field_layers_merge_key_by_key() {
        let file = Settings::from_toml("[field]\nenabled = true\nport = 8000\n").unwrap();
        let flags = Settings::from_toml("[field]\nbind = \"::1\"\n").unwrap();
        let field = resolve(Settings::default(), file, flags)
            .field
            .resolved()
            .unwrap();
        assert!(field.enabled);
        assert_eq!(field.port, 8000);
        assert_eq!(field.bind, "::1".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn field_bind_must_be_loopback() {
        let s = Settings::from_toml("[field]\nbind = \"0.0.0.0\"\n").unwrap();
        assert!(matches!(
            s.field.resolved(),
            Err(Error::FieldBindNotLoopback(_))
        ));
    }
}
