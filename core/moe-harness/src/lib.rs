//! MoE expert offloading / placement.
//!
//! STUB — implemented in Phase 3, in userspace first. Decides which experts live
//! in VRAM vs. system RAM based on detected VRAM and model metadata. Kernel-level
//! placement is explicitly deferred (plan §5/§9) until userspace overhead is
//! measured to be a real bottleneck.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("MoE harness not yet implemented (Phase 3): {0}")]
    NotImplemented(&'static str),
}

/// Plan expert offloading for `model` given available VRAM (MiB).
pub fn plan_offload(_model: &str, _vram_mb: Option<u64>) -> Result<(), Error> {
    Err(Error::NotImplemented("gated on a proven Phase 2 core"))
}
