//! AMD GPU passthrough for containers.
//!
//! Giving a container access to an AMD GPU is not one flag — it is a small,
//! easy-to-get-wrong recipe: expose the compute node (`/dev/kfd`) and the DRM
//! render nodes (`/dev/dri`), add the `video`/`render` groups so the in-container
//! user can actually open them, and (for ROCm) relax seccomp. Cameo's whole
//! reason to exist is that the user should never have to assemble this by hand.
//!
//! This module is pure: it *builds* the argument recipe and is unit-tested on any
//! OS. Nothing here runs a container — that is the caller's job, through the same
//! execution boundary the rest of Cameo uses.
//!
//! ⚠️ The exact recipe (which nodes, whether `seccomp=unconfined` and the
//! `render` group are strictly required) is hardware- and runtime-dependent and
//! is a Phase-1 item to confirm on a validated host — centralized here on
//! purpose, the same way the llama.cpp flags are centralized in
//! `cameo_placement::command`.

use serde::{Deserialize, Serialize};

/// The device/group/security recipe that grants a container AMD GPU access.
///
/// Runtime-agnostic: the same recipe renders to Podman or Docker `run` flags
/// (they share this surface) via [`Self::to_run_args`], and is the natural input
/// to a future Kubernetes device-plugin resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpuPassthrough {
    /// Device files to expose, e.g. `/dev/kfd` and DRM render nodes.
    pub devices: Vec<String>,
    /// Supplementary groups the container process must join to open those
    /// devices — `video` (DRI) and, on most stacks, `render`.
    pub groups: Vec<String>,
    /// Security options. ROCm's userspace commonly needs `seccomp=unconfined`;
    /// kept explicit (and overridable) rather than always-on.
    pub security_opts: Vec<String>,
}

impl GpuPassthrough {
    /// The broad, works-everywhere recipe: expose the compute node and the whole
    /// DRM directory, join both groups, relax seccomp. This is what
    /// `cameo containers run-args` emits by default when no specific render nodes
    /// were named — the container sees every AMD GPU on the box.
    pub fn amd_all() -> Self {
        Self {
            devices: vec!["/dev/kfd".into(), "/dev/dri".into()],
            groups: vec!["video".into(), "render".into()],
            security_opts: vec!["seccomp=unconfined".into()],
        }
    }

    /// A narrowed recipe that exposes only the given DRM render nodes (plus the
    /// shared `/dev/kfd`), for pinning a container to specific cards. An empty
    /// list falls back to the whole `/dev/dri` directory rather than exposing no
    /// GPU at all — a caller that asked for passthrough wants *a* GPU.
    pub fn amd_render_nodes(nodes: &[&str]) -> Self {
        let mut devices = vec!["/dev/kfd".to_string()];
        if nodes.is_empty() {
            devices.push("/dev/dri".into());
        } else {
            devices.extend(nodes.iter().map(|n| n.to_string()));
        }
        Self {
            devices,
            ..Self::amd_all()
        }
    }

    /// Render the recipe as `podman`/`docker run` arguments, in a stable order:
    /// `--device` per node, `--group-add` per group, `--security-opt` per option.
    pub fn to_run_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        for d in &self.devices {
            args.push("--device".into());
            args.push(d.clone());
        }
        for g in &self.groups {
            args.push("--group-add".into());
            args.push(g.clone());
        }
        for s in &self.security_opts {
            args.push("--security-opt".into());
            args.push(s.clone());
        }
        args
    }
}

/// Build a full `podman`/`docker run` argument vector for `image`, with GPU
/// passthrough spliced in ahead of the image and any command.
///
/// Layout: `run [--rm] [--name N] <passthrough> [extra…] <image> [cmd…]`. The
/// program (`podman` vs `docker`) and the spawn are the caller's — this returns
/// only the argv so it can flow through the same [execution boundary] the rest of
/// Cameo uses.
///
/// [execution boundary]: cameo_placement::command
pub fn run_args(image: &str, opts: &RunOpts, gpu: &GpuPassthrough) -> Vec<String> {
    let mut args = vec!["run".to_string()];
    if opts.remove {
        args.push("--rm".into());
    }
    if opts.detach {
        args.push("-d".into());
    }
    if let Some(name) = &opts.name {
        args.push("--name".into());
        args.push(name.clone());
    }
    args.extend(gpu.to_run_args());
    args.extend(opts.extra.iter().cloned());
    args.push(image.to_string());
    args.extend(opts.command.iter().cloned());
    args
}

/// Knobs for [`run_args`] beyond GPU passthrough. Defaults are the sensible
/// interactive-throwaway shape; each field maps to one well-known flag.
#[derive(Debug, Clone, Default)]
pub struct RunOpts {
    /// `--rm`: delete the container when it exits.
    pub remove: bool,
    /// `-d`: run detached.
    pub detach: bool,
    /// `--name`: a stable name (also the natural endpoint id later).
    pub name: Option<String>,
    /// Any additional raw flags to pass through verbatim (ports, volumes, env).
    pub extra: Vec<String>,
    /// The command + args to run inside the image, if overriding its entrypoint.
    pub command: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amd_all_exposes_kfd_and_dri_with_both_groups() {
        let args = GpuPassthrough::amd_all().to_run_args();
        assert!(args.windows(2).any(|w| w == ["--device", "/dev/kfd"]));
        assert!(args.windows(2).any(|w| w == ["--device", "/dev/dri"]));
        assert!(args.windows(2).any(|w| w == ["--group-add", "video"]));
        assert!(args.windows(2).any(|w| w == ["--group-add", "render"]));
        assert!(args
            .windows(2)
            .any(|w| w == ["--security-opt", "seccomp=unconfined"]));
    }

    #[test]
    fn render_nodes_narrow_the_dri_exposure() {
        let p = GpuPassthrough::amd_render_nodes(&["/dev/dri/renderD128"]);
        assert!(p.devices.contains(&"/dev/kfd".to_string()));
        assert!(p.devices.contains(&"/dev/dri/renderD128".to_string()));
        // The broad directory is not exposed when specific nodes were named.
        assert!(!p.devices.contains(&"/dev/dri".to_string()));
    }

    #[test]
    fn empty_render_nodes_fall_back_to_the_whole_directory() {
        // Asking for passthrough must never yield a container with no GPU.
        let p = GpuPassthrough::amd_render_nodes(&[]);
        assert!(p.devices.contains(&"/dev/dri".to_string()));
    }

    #[test]
    fn run_args_places_passthrough_before_image_and_command() {
        let opts = RunOpts {
            remove: true,
            name: Some("rocm-test".into()),
            command: vec!["rocminfo".into()],
            ..Default::default()
        };
        let args = run_args("rocm/dev-ubuntu-22.04", &opts, &GpuPassthrough::amd_all());

        let image_at = args
            .iter()
            .position(|a| a == "rocm/dev-ubuntu-22.04")
            .unwrap();
        let device_at = args.iter().position(|a| a == "--device").unwrap();
        let cmd_at = args.iter().position(|a| a == "rocminfo").unwrap();

        assert_eq!(args[0], "run");
        assert!(args.contains(&"--rm".to_string()));
        assert!(device_at < image_at, "passthrough must precede the image");
        assert!(
            image_at < cmd_at,
            "the in-container command follows the image"
        );
    }
}
