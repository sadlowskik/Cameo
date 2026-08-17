//! The execution boundary.
//!
//! Everything above this module is pure planning. This module translates a
//! [`PlacementPlan`] into an exact command line (`CommandSpec`) — still pure and
//! unit-tested — and [`execute`] is the *only* function that actually spawns a
//! process against the GPU. That keeps all hardware risk in one small, clearly
//! marked place for the Phase 1 validation run.
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
    /// A copy-pasteable shell rendering, for `--dry-run`.
    pub fn display(&self) -> String {
        let env: String = self.env.iter().map(|(k, v)| format!("{k}={v} ")).collect();
        format!("{env}{} {}", self.program, self.args.join(" "))
    }
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
pub fn build_llama_server(
    plan: &PlacementPlan,
    model: &ModelMeta,
    model_path: &str,
    binary: &str,
    host: &str,
    port: u16,
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
pub fn build_training(plan: &PlacementPlan, config: &str) -> CommandSpec {
    let shards = match plan.multi_gpu {
        MultiGpu::Fsdp { shards } => shards,
        _ => 1,
    };
    let args = vec![
        "--standalone".into(),
        "--nproc_per_node".into(),
        shards.to_string(),
        "train.py".into(),
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

/// THE execution boundary: spawn the command and wait. Linux only — everything
/// that reaches real GPU work funnels through here.
#[cfg(target_os = "linux")]
pub fn execute(spec: &CommandSpec) -> Result<(), ExecError> {
    use std::process::Command;
    let mut cmd = Command::new(&spec.program);
    cmd.args(&spec.args);
    for (k, v) in &spec.env {
        cmd.env(k, v);
    }
    let status = cmd.status().map_err(|e| ExecError::Spawn {
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
    use cameo_gpu_detect::{classify, GpuInfo, Link, LinkKind, OverrideDb, Topology};

    fn gpu(gfx: &str, vram_mb: u64) -> GpuInfo {
        GpuInfo {
            model: gfx.into(),
            pci_id: "1002:0000".into(),
            vram_mb: Some(vram_mb),
            gfx_arch: Some(gfx.into()),
            driver_version: None,
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
    fn training_uses_fsdp_degree() {
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
        let spec = build_training(&p, "cfg.toml");
        let i = spec
            .args
            .iter()
            .position(|a| a == "--nproc_per_node")
            .unwrap();
        assert_eq!(spec.args[i + 1], "2");
    }

    #[test]
    fn server_command_has_host_port_and_carries_offload() {
        let m = ModelMeta::moe("mixtral", 47.0, QuantLevel::Q4_K_M);
        let p = plan_for(vec![gpu("gfx1030", 16384)], vec![], &m, Task::Inference);
        let spec = build_llama_server(&p, &m, "/m.gguf", "llama-server", "0.0.0.0", 8080);
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
    fn dry_run_display_is_readable() {
        let m = ModelMeta::dense("llama-7b", 7.0, QuantLevel::Q4_K_M);
        let p = plan_for(vec![gpu("gfx1030", 16384)], vec![], &m, Task::Inference);
        let spec = build_llama_run(&p, &m, "/m.gguf", "llama-cli");
        let shown = spec.display();
        assert!(shown.contains("HSA_OVERRIDE_GFX_VERSION=10.3.0"));
        assert!(shown.contains("llama-cli -m /m.gguf"));
    }
}
