//! llama.cpp Vulkan backend — Cameo's universal baseline (works on every tier).
//!
//! Execution funnels through the shared boundary in `cameo_placement::command`.
//! This crate's job is to name *which* binary is the Vulkan-built llama.cpp and
//! run a prepared [`CommandSpec`] through it. Building that binary is Phase 1
//! (`scripts/phase1/build-llama.sh`); the binary only exists on validated
//! Linux hosts, so `run` errors cleanly elsewhere.

use cameo_placement::{execute, CommandSpec, ExecError};

/// Default Vulkan-built llama.cpp CLI (on PATH after a Cameo install).
pub const DEFAULT_BINARY: &str = "llama-cli";

/// Run a prepared command through the Vulkan backend.
pub fn run(spec: &CommandSpec) -> Result<(), ExecError> {
    execute(spec)
}
