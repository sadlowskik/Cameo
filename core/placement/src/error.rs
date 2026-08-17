//! Placement engine errors.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("no GPUs available to plan for")]
    NoGpus,

    #[error("training requires a Tier 1/2 (ROCm) GPU; the top detected GPU is Tier {0}")]
    TrainingUnsupported(u8),

    #[error("no nodes in the cluster to place onto")]
    NoNodes,

    #[error("no node in the cluster can train (every node is Tier 3)")]
    NoTrainableNode,

    #[error("unknown cloud provider '{0}' (known: anthropic, openai)")]
    UnknownProvider(String),

    #[error("node '{0}' not found in the cluster")]
    NodeNotFound(String),

    #[error("local agent '{0}' model does not fit any single node; distributed serving is v2")]
    LocalAgentTooLarge(String),
}
