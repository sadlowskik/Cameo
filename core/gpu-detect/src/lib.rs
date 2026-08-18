//! AMD GPU detection and capability tiering for Cameo.
//!
//! The crate is split so that all decision logic is pure and testable on any OS:
//!
//! - [`parse`]   — turn `lspci` / `rocminfo` / sysfs text into [`GpuInfo`] (no I/O).
//! - [`overrides`] — the versioned compatibility database ([`OverrideDb`]).
//! - [`classify`] — map a [`GpuInfo`] + [`OverrideDb`] to a [`TierAssessment`].
//! - [`collect`] — Linux-only I/O that gathers the raw text and produces [`GpuInfo`].
//!
//! Every "smart" default here is overridable — the classifier never invents a ROCm
//! path; unknown hardware falls back to Vulkan-only (Tier 3), which the user can
//! override in config.

pub mod classify;
pub mod collect;
pub mod detect;
pub mod error;
pub mod hostmem;
pub mod memfacts;
pub mod overrides;
pub mod parse;
pub mod topology;
pub mod types;

pub use classify::{classify, classify_topology};
pub use collect::{collect, collect_topology};
pub use detect::{detect_topology, detect_topology_or_cpu, Captures};
pub use error::Error;
pub use hostmem::{parse_meminfo, HostMemory};
pub use memfacts::{apply_gpu_memory, parse_gpu_memory};
pub use overrides::OverrideDb;
pub use topology::{Link, LinkKind, Topology};
pub use types::{GpuInfo, MemoryKind, Tier, TierAssessment};
