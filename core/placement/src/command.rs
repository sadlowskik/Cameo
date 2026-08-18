//! The execution boundary.
//!
//! Everything above this module is pure planning. This module translates a
//! [`PlacementPlan`] into an exact command line (`CommandSpec`) — still pure and
//! unit-tested — and [`execute`] is the only function *here* that spawns a
//! process against the GPU. (Detection has its own, separate hardware boundary
//! in `cameo_gpu_detect::collect`; see `docs/architecture.md`.)
//!
//! ⚠️ The llama.cpp / PyTorch flag names below are best-effort and are the main
//! thing to confirm during Phase 1. They are centralized here on purpose.

use crate::model::ModelMeta;
use crate::plan::{GpuLayers, MultiGpu, PlacementPlan};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A fully-resolved command: program, arguments, and environment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

impl CommandSpec {
    /// A copy-pasteable shell rendering, for `--dry-run` and the `shell` field
    /// of `--json`.
    ///
    /// Every token is quoted for POSIX `sh`. This is not cosmetic: the rendering
    /// is published as something to run, and a model path containing a space
    /// used to render as two arguments, while one containing `;` rendered as a
    /// second command. Arguments are program-supplied, but the *paths* in them
    /// come from whoever typed them.
    pub fn display(&self) -> String {
        let mut out = String::new();
        for (k, v) in &self.env {
            out.push_str(&format!("{k}={} ", shell_quote(v)));
        }
        out.push_str(&shell_quote(&self.program));
        for a in &self.args {
            out.push(' ');
            out.push_str(&shell_quote(a));
        }
        out
    }
}

/// Quote one token for POSIX `sh`.
///
/// Anything outside a conservative safe set is wrapped in single quotes, with
/// embedded single quotes rendered the only way `sh` allows: close, escape,
/// reopen.
fn shell_quote(s: &str) -> String {
    const SAFE: &str = "-_./:=@,+";
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || SAFE.contains(c))
    {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Build a llama.cpp inference command from a plan.
///
/// `binary` is the llama.cpp CLI to run (the Vulkan- or ROCm-built one); the
/// caller picks it based on `plan.backend`.
pub fn build_llama_run(
    plan: &PlacementPlan,
    model: &ModelMeta,
    model_path: &str,
    binary: &str,
) -> CommandSpec {
    let mut args = vec![
        "-m".into(),
        model_path.to_string(),
        "-c".into(),
        model.context_len.to_string(),
    ];
    args.extend(placement_flags(plan));

    CommandSpec {
        program: binary.to_string(),
        args,
        env: plan.env.clone(),
    }
}

/// Offload + multi-GPU flags shared by the run and server commands. This is where
/// the llama.cpp flag assumptions live — confirm them in Phase 1.
fn placement_flags(plan: &PlacementPlan) -> Vec<String> {
    let mut args = Vec::new();
    match plan.offload.gpu_layers {
        // 999 = "offload every layer" in llama.cpp convention.
        GpuLayers::All => args.extend(["-ngl".into(), "999".into()]),
        GpuLayers::Count(n) => args.extend(["-ngl".into(), n.to_string()]),
    }
    if plan.offload.experts_on_host {
        // Keep MoE expert tensors in host RAM, streamed on demand.
        args.extend(["--override-tensor".into(), "exps=CPU".into()]);
    }
    if plan.offload.kv_on_host {
        args.push("--no-kv-offload".into());
    }
    match &plan.multi_gpu {
        MultiGpu::LayerSplit { fractions } => {
            args.extend(["--split-mode".into(), "layer".into()]);
            let split = fractions
                .iter()
                .map(|f| format!("{f:.3}"))
                .collect::<Vec<_>>()
                .join(",");
            args.extend(["--tensor-split".into(), split]);
        }
        MultiGpu::ExpertParallel => {
            args.extend(["--split-mode".into(), "layer".into()]);
        }
        MultiGpu::Single | MultiGpu::Fsdp { .. } => {}
    }
    args
}

/// Build a `llama-server` command — a persistent, OpenAI-compatible HTTP endpoint.
/// This is what makes Cameo a serving box (and the engine provider a harness's
/// engine slot can point at).
///
/// `api_key` is the credential clients must present. Callers that bind to
/// anything other than loopback are expected to supply one; the decision of
/// whether it is *required* belongs to the caller, which knows the reachability
/// of the address it chose (see [`crate::agents::resolve_agent`]).
pub fn build_llama_server(
    plan: &PlacementPlan,
    model: &ModelMeta,
    model_path: &str,
    binary: &str,
    host: &str,
    port: u16,
    api_key: Option<&str>,
) -> CommandSpec {
    let mut args = vec![
        "-m".into(),
        model_path.to_string(),
        "-c".into(),
        model.context_len.to_string(),
        "--host".into(),
        host.to_string(),
        "--port".into(),
        port.to_string(),
    ];
    if let Some(key) = api_key {
        args.extend(["--api-key".into(), key.to_string()]);
    }
    args.extend(placement_flags(plan));

    CommandSpec {
        program: binary.to_string(),
        args,
        env: plan.env.clone(),
    }
}

/// Build a `llama-bench` command (used by the Phase 1 benchmark step logic).
pub fn build_llama_bench(plan: &PlacementPlan, model_path: &str, binary: &str) -> CommandSpec {
    let mut args = vec![
        "-m".into(),
        model_path.to_string(),
        "-o".into(),
        "json".into(),
    ];
    if let GpuLayers::Count(n) = plan.offload.gpu_layers {
        args.extend(["-ngl".into(), n.to_string()]);
    }
    CommandSpec {
        program: binary.to_string(),
        args,
        env: plan.env.clone(),
    }
}

/// Build a training launch command (PyTorch, ROCm). Training harness is Phase 2;
/// this encodes the intended shape (torchrun + FSDP degree).
///
/// `script` is the user's training entry point. Cameo used to hardcode
/// `train.py`, a file that exists in neither the repo nor the image, so every
/// `cameo train` resolved to a torchrun invocation that could only fail — the
/// launcher is Cameo's job, the training loop is yours.
pub fn build_training(plan: &PlacementPlan, script: &str, config: &str) -> CommandSpec {
    let shards = match plan.multi_gpu {
        MultiGpu::Fsdp { shards } => shards,
        _ => 1,
    };
    let args = vec![
        "--standalone".into(),
        "--nproc_per_node".into(),
        shards.to_string(),
        script.to_string(),
        "--config".into(),
        config.to_string(),
    ];
    CommandSpec {
        program: "torchrun".into(),
        args,
        env: plan.env.clone(),
    }
}

/// Build a GGUF quantization command (wraps llama.cpp's `llama-quantize`).
pub fn build_quantize(model_in: &str, model_out: &str, level: &str) -> CommandSpec {
    CommandSpec {
        program: "llama-quantize".into(),
        args: vec![model_in.into(), model_out.into(), level.into()],
        env: Vec::new(),
    }
}

/// Errors from actually running a [`CommandSpec`].
#[derive(Debug, Error)]
pub enum ExecError {
    #[error("backend execution is only supported on Linux (this is a dev host)")]
    UnsupportedOs,
    #[error("failed to spawn `{program}`: {source}")]
    Spawn {
        program: String,
        source: std::io::Error,
    },
    #[error("`{program}` exited with status {code}")]
    NonZero { program: String, code: i32 },
}

/// Build a `Command` from a spec with the death-signal wiring attached.
///
/// Shared by [`spawn`] and [`execute`] so the CLI's foreground run and the
/// daemon's background supervisor get identical orphan-proofing: the child is
/// tied to this process's lifetime with `PR_SET_PDEATHSIG`. Without it, killing
/// `cameo`/`cameod` left `llama-server` running — still holding VRAM, still
/// bound to its port, and invisible to the next plan. The child deliberately
/// stays in *this* process group, so Ctrl-C in a terminal still reaches it.
#[cfg(target_os = "linux")]
fn configured_command(spec: &CommandSpec) -> std::process::Command {
    use std::os::unix::process::CommandExt;
    use std::process::Command;

    let mut cmd = Command::new(&spec.program);
    cmd.args(&spec.args);
    for (k, v) in &spec.env {
        cmd.env(k, v);
    }

    // SAFETY: `pre_exec` runs in the forked child between fork and exec, where
    // only async-signal-safe work is permitted. Both calls here are bare
    // syscalls, and the error path only reads `errno`.
    unsafe {
        cmd.pre_exec(|| {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            // The parent can die between fork and prctl, in which case the death
            // signal was already delivered and this child would survive it —
            // exactly the orphan the call was meant to prevent. Re-check.
            if libc::getppid() == 1 {
                return Err(std::io::Error::other("parent exited before spawn"));
            }
            Ok(())
        });
    }
    cmd
}

/// Execution boundary (non-blocking): spawn the command and hand back the live
/// child without waiting. This is what `cameod`'s supervisor tracks so it can
/// stop an endpoint later; the child dies with the daemon (see
/// [`configured_command`]), so a crashed daemon never leaks a serving process.
#[cfg(target_os = "linux")]
pub fn spawn(spec: &CommandSpec) -> Result<std::process::Child, ExecError> {
    configured_command(spec)
        .spawn()
        .map_err(|e| ExecError::Spawn {
            program: spec.program.clone(),
            source: e,
        })
}

/// Execution boundary (blocking): spawn the command and wait. Linux only —
/// everything that reaches real GPU work funnels through here or [`spawn`].
#[cfg(target_os = "linux")]
pub fn execute(spec: &CommandSpec) -> Result<(), ExecError> {
    let status = spawn(spec)?.wait().map_err(|e| ExecError::Spawn {
        program: spec.program.clone(),
        source: e,
    })?;
    if status.success() {
        Ok(())
    } else {
        Err(ExecError::NonZero {
            program: spec.program.clone(),
            code: status.code().unwrap_or(-1),
        })
    }
}

/// Non-Linux stub for [`spawn`].
#[cfg(not(target_os = "linux"))]
pub fn spawn(_spec: &CommandSpec) -> Result<std::process::Child, ExecError> {
    Err(ExecError::UnsupportedOs)
}

/// Non-Linux stub for [`execute`].
#[cfg(not(target_os = "linux"))]
pub fn execute(_spec: &CommandSpec) -> Result<(), ExecError> {
    Err(ExecError::UnsupportedOs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ModelMeta, QuantLevel};
    use crate::plan::{plan, Task};
    use cameo_config::Settings;
    use cameo_gpu_detect::{classify, GpuInfo, Link, LinkKind, MemoryKind, OverrideDb, Topology};

    fn gpu(gfx: &str, vram_mb: u64) -> GpuInfo {
        GpuInfo {
            model: gfx.into(),
            pci_id: "1002:0000".into(),
            vram_mb: Some(vram_mb),
            gfx_arch: Some(gfx.into()),
            memory: MemoryKind::Dedicated,
            ..Default::default()
        }
    }

    fn plan_for(
        gpus: Vec<GpuInfo>,
        links: Vec<Link>,
        model: &ModelMeta,
        task: Task,
    ) -> PlacementPlan {
        let db = OverrideDb::embedded();
        let assessments: Vec<_> = gpus.iter().cloned().map(|g| classify(g, &db)).collect();
        let topo = Topology::new(gpus, links);
        plan(&topo, &assessments, model, task, &Settings::default()).unwrap()
    }

    #[test]
    fn moe_offload_emits_override_tensor() {
        let m = ModelMeta::moe("mixtral", 47.0, QuantLevel::Q4_K_M);
        let p = plan_for(vec![gpu("gfx1100", 16384)], vec![], &m, Task::Inference);
        let spec = build_llama_run(&p, &m, "/models/mixtral.gguf", "llama-cli");
        assert!(spec
            .args
            .windows(2)
            .any(|w| w == ["--override-tensor", "exps=CPU"]));
    }

    #[test]
    fn layer_split_emits_tensor_split() {
        let links = vec![Link {
            a: 0,
            b: 1,
            kind: LinkKind::Xgmi,
        }];
        let m = ModelMeta::dense("llama-70b", 70.0, QuantLevel::Q4_K_M);
        let p = plan_for(
            vec![gpu("gfx1100", 16384), gpu("gfx1100", 16384)],
            links,
            &m,
            Task::Inference,
        );
        let spec = build_llama_run(&p, &m, "/m.gguf", "llama-cli");
        let i = spec
            .args
            .iter()
            .position(|a| a == "--tensor-split")
            .expect("tensor-split");
        assert!(spec.args[i + 1].contains(','));
    }

    #[test]
    fn hsa_override_propagates_to_env() {
        let m = ModelMeta::dense("llama-7b", 7.0, QuantLevel::Q4_K_M);
        // gfx1030 is Tier 2 with a 10.3.0 override in the seed DB.
        let p = plan_for(vec![gpu("gfx1030", 16384)], vec![], &m, Task::Inference);
        let spec = build_llama_run(&p, &m, "/m.gguf", "llama-cli");
        assert!(spec
            .env
            .iter()
            .any(|(k, v)| k == "HSA_OVERRIDE_GFX_VERSION" && v == "10.3.0"));
    }

    #[test]
    fn training_uses_fsdp_degree_and_the_given_script() {
        let links = vec![Link {
            a: 0,
            b: 1,
            kind: LinkKind::Xgmi,
        }];
        let m = ModelMeta::dense("llama-7b", 7.0, QuantLevel::Q4_K_M);
        let p = plan_for(
            vec![gpu("gfx1100", 24576), gpu("gfx1100", 24576)],
            links,
            &m,
            Task::Training,
        );
        let spec = build_training(&p, "/work/finetune.py", "cfg.toml");
        let i = spec
            .args
            .iter()
            .position(|a| a == "--nproc_per_node")
            .unwrap();
        assert_eq!(spec.args[i + 1], "2");
        assert!(spec.args.iter().any(|a| a == "/work/finetune.py"));
        assert!(
            !spec.args.iter().any(|a| a == "train.py"),
            "no hardcoded entry point"
        );
    }

    #[test]
    fn server_command_has_host_port_and_carries_offload() {
        let m = ModelMeta::moe("mixtral", 47.0, QuantLevel::Q4_K_M);
        let p = plan_for(vec![gpu("gfx1030", 16384)], vec![], &m, Task::Inference);
        let spec = build_llama_server(&p, &m, "/m.gguf", "llama-server", "127.0.0.1", 8080, None);
        assert_eq!(spec.program, "llama-server");
        let i = spec.args.iter().position(|a| a == "--port").unwrap();
        assert_eq!(spec.args[i + 1], "8080");
        assert!(spec.args.iter().any(|a| a == "--host"));
        // Offload decisions still flow through the shared helper.
        assert!(spec
            .args
            .windows(2)
            .any(|w| w == ["--override-tensor", "exps=CPU"]));
    }

    #[test]
    fn server_command_passes_an_api_key_when_given() {
        let m = ModelMeta::dense("llama-7b", 7.0, QuantLevel::Q4_K_M);
        let p = plan_for(vec![gpu("gfx1100", 16384)], vec![], &m, Task::Inference);
        let spec = build_llama_server(
            &p,
            &m,
            "/m.gguf",
            "llama-server",
            "10.0.0.4",
            8080,
            Some("s3cret"),
        );
        assert!(spec.args.windows(2).any(|w| w == ["--api-key", "s3cret"]));
    }

    #[test]
    fn dry_run_display_is_readable() {
        let m = ModelMeta::dense("llama-7b", 7.0, QuantLevel::Q4_K_M);
        let p = plan_for(vec![gpu("gfx1030", 16384)], vec![], &m, Task::Inference);
        let spec = build_llama_run(&p, &m, "/m.gguf", "llama-cli");
        let shown = spec.display();
        assert!(shown.contains("HSA_OVERRIDE_GFX_VERSION=10.3.0"));
        assert!(shown.contains("llama-cli -m /m.gguf"));
    }

    #[test]
    fn display_quotes_paths_that_would_otherwise_split_or_inject() {
        let m = ModelMeta::dense("llama-7b", 7.0, QuantLevel::Q4_K_M);
        let p = plan_for(vec![gpu("gfx1100", 16384)], vec![], &m, Task::Inference);

        let spaced = build_llama_run(&p, &m, "/models/My Models/llama 7b.gguf", "llama-cli");
        assert!(
            spaced
                .display()
                .contains("'/models/My Models/llama 7b.gguf'"),
            "got {}",
            spaced.display()
        );

        // The whole rendering, so the separator is provably *inside* the quotes
        // rather than merely present somewhere in the string.
        let nasty = build_llama_run(&p, &m, "/tmp/x.gguf; rm -rf ~", "llama-cli");
        assert_eq!(
            nasty.display(),
            "llama-cli -m '/tmp/x.gguf; rm -rf ~' -c 4096 -ngl 999"
        );
    }

    #[test]
    fn display_escapes_embedded_single_quotes() {
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(shell_quote("plain-path/v1.0"), "plain-path/v1.0");
        assert_eq!(shell_quote(""), "''");
    }
}
