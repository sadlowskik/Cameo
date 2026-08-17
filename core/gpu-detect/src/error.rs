//! Error type for the detection crate.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("failed to parse overrides TOML: {0}")]
    Toml(#[from] toml::de::Error),

    #[error("invalid tier value {0} in overrides database (expected 1, 2, or 3)")]
    BadTier(u8),

    #[error("GPU collection is only supported on Linux; feed captured text to the parser instead")]
    UnsupportedOs,

    #[error("no AMD GPU detected in lspci output")]
    NoGpu,

    #[error("I/O error during GPU collection: {0}")]
    Io(#[from] std::io::Error),
}
