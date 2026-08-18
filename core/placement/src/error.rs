//! Placement engine errors.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("no GPUs available to plan for")]
    NoGpus,

    #[error("invalid model description: {0}")]
    InvalidModel(String),

    #[error("model needs ~{needed_gib:.1} GiB but this machine can offer ~{available_gib:.1} GiB of VRAM + host RAM; use a smaller model or a heavier quantization, or pass --allow-oversize to plan it anyway")]
    InsufficientMemory { needed_gib: f64, available_gib: f64 },

    #[error("serving agent '{0}' on a non-loopback address needs an API key; set serve_api_key in config (or give the node a loopback address)")]
    MissingApiKey(String),

    #[error("training requires a Tier 1/2 (ROCm) GPU; the top detected GPU is Tier {0}")]
    TrainingUnsupported(u8),

    #[error(
        "training needs a GPU; this machine has no usable GPU, so only CPU inference is available"
    )]
    CpuInferenceOnly,

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
