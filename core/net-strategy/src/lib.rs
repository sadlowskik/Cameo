//! Multi-node networking strategy selection.
//!
//! STUB — v2 scope (plan §5), explicitly not built yet. Will auto-detect link
//! quality/type to choose a sync strategy (gradient compression, pipeline
//! parallelism, local-SGD, or standard data-parallel), always with a manual
//! override. Present only so the workspace layout matches the plan.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("multi-node networking is v2 scope and not implemented: {0}")]
    NotImplemented(&'static str),
}

/// Select a networking strategy for the current cluster.
pub fn select_strategy() -> Result<(), Error> {
    Err(Error::NotImplemented("deferred to v2"))
}
