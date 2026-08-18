//! Cameo's container integration.
//!
//! Cameo does not reimplement Docker or Podman — it *integrates* them and adds
//! the one thing they lack: AMD GPU awareness. This crate is the pure half of
//! that integration:
//!
//! - [`passthrough`] — the AMD GPU passthrough recipe (`/dev/kfd`, DRM nodes,
//!   groups, seccomp) and the `podman`/`docker run` argument builder. This is
//!   Cameo's differentiator and it is fully unit-tested on any OS.
//!
//! The socket I/O that actually talks to a running Podman/Docker daemon is a
//! separate, Linux-gated boundary (a later increment), mirroring the detection
//! boundary in `cameo_gpu_detect::collect` and the execution boundary in
//! `cameo_placement::command`. Everything here stays pure and testable, so the
//! console's Containers view can be built and exercised with no daemon present.

pub mod passthrough;

pub use passthrough::{run_args, GpuPassthrough, RunOpts};
