//! Cameo's placement engine.
//!
//! Turns detected hardware and a model description into a concrete plan for how
//! to place work across GPU(s) and host memory, for both inference and training.
//! It is pure, deterministic logic — the [`plan`] output is what the backend
//! command builders translate into an actual command line. Nothing here touches
//! the GPU, so it is fully testable on any OS.

pub mod agents;
pub mod command;
pub mod error;
pub mod fleet;
pub mod model;
pub mod plan;
pub mod router;

pub use agents::{
    resolve_agent, resolve_agents, AgentRunPlan, AgentSpec, EngineBinding, PlacementTarget,
};
pub use command::{execute, spawn, CommandSpec, ExecError};
pub use error::Error;
pub use fleet::{place_on_fleet, Cluster, FleetPlacement, FleetPlan, NetworkClass, NodeInfo};
pub use model::{gib, ModelMeta, QuantLevel};
pub use plan::{plan, GpuLayers, MemoryBudget, MultiGpu, Offload, PlacementPlan, Task};
pub use router::{route, Candidate, NodeLoad, RouteChoice, RouteError, RouteRequest};
