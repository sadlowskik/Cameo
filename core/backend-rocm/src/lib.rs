//! llama.cpp ROCm/HIP inference + PyTorch ROCm training backend.
//!
//! Only meaningfully active on Tier 1/2 hardware. Execution funnels through the
//! shared boundary in `cameo_placement::command`; this crate names the ROCm-built
//! binaries and runs prepared commands. On any runtime failure the caller is
//! expected to fall back to the Vulkan backend (Cameo's baseline) and log why.

use cameo_placement::{execute, CommandSpec, ExecError};

/// Default ROCm-built llama.cpp CLI (on PATH after a Cameo install).
pub const DEFAULT_BINARY: &str = "llama-cli";

/// Default training launcher (PyTorch ROCm).
pub const DEFAULT_TRAIN_LAUNCHER: &str = "torchrun";

/// Run a prepared command (inference or training) through the ROCm backend.
pub fn run(spec: &CommandSpec) -> Result<(), ExecError> {
    execute(spec)
}
