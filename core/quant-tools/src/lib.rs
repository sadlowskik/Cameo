//! GGUF quantization pipeline.
//!
//! Wraps llama.cpp's `llama-quantize` (don't reinvent it). Command construction
//! lives in `cameo_placement::command`; execution funnels through the shared
//! boundary, so this runs only on validated Linux hosts.

use cameo_placement::command::build_quantize;
use cameo_placement::{execute, ExecError};

/// Quantize `model_in` to `model_out` at `level` (e.g. `Q4_K_M`).
pub fn quantize(model_in: &str, model_out: &str, level: &str) -> Result<(), ExecError> {
    execute(&build_quantize(model_in, model_out, level))
}
