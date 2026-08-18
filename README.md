# Cameo

> **Get real use out of any AMD hardware — from one old card in a drawer to a cluster — without fighting the stack.**

Cameo is an Arch Linux respin that runs, **serves**, and (on capable cards) trains LLMs on
AMD GPUs. It meets your hardware where it is and scales along one continuum — a single old
Radeon, a multi-GPU box, up to a cluster — with the same commands.

- **Meets the hardware where it is.** Vulkan is the universal baseline that works on *any*
  AMD card; ROCm is the optional accelerator on supported ones. Nothing *requires* ROCm.
- **Auto-detect, always overridable.** Every smart default (tier, backend, placement) has
  a manual override.
- **One core, many frontends.** All logic lives in the Rust `core/`; the `cameo` CLI and
  the `cameod` browser console are thin clients over the same detection/placement brain.
- **Administer it from a browser.** `cameod` serves a self-contained control plane — see
  every GPU and tier, define/start/stop inference endpoints, watch the model cache — with
  no external web stack. It ships in the ISO and starts on boot.

See [`CAMEO_PROJECT_PLAN.md`](CAMEO_PROJECT_PLAN.md) for the full build plan.

## Status

Pre-v1, greenfield. This tree currently contains the **hardware-independent** scaffolding:
core detection logic, the internal API contract, the CLI, and the automated **Phase 1**
hardware-validation runbook. Real Vulkan/ROCm execution is validated on AMD hardware
(a cloud AMD instance) via [`scripts/phase1/`](scripts/phase1/) — see that directory's
`RUNBOOK.md`.

## Repository layout

```
core/                 Rust — all real logic
  gpu-detect/         AMD GPU detection, multi-GPU topology, Tier 1/2/3 classify
  config/             config + override precedence (flag > file > auto-detect)
  placement/          the brain: (topology × model × task) → plan → command
  models/             model cache + acquisition (cameo pull), shared by CLI + daemon
  containers/         AMD GPU passthrough recipe for Podman/Docker
  api/                stable internal JSON-RPC API surface (CLI + GUI bind to this)
  backend-vulkan/     llama.cpp Vulkan executor (universal baseline)
  backend-rocm/       llama.cpp ROCm + PyTorch training executor (Tier 1/2)
  quant-tools/        GGUF quantization (wraps llama-quantize)
  moe-harness/        MoE expert offloading                   (stub — Phase 3)
  net-strategy/       multi-node networking strategy          (stub — v2)
cli/                  `cameo` command-line tool (thin client over core)
cameod/               `cameod` control-plane daemon: browser console + JSON API
archiso/              Arch ISO build profile (ships cameo + cameod)
containers/           container tooling notes (passthrough logic is core/containers)
k8s/                  device plugin / Helm charts             (v2)
scripts/phase1/       automated Phase 1 hardware validation
docs/                 architecture, tiers, API, definition-of-done
tests/                cross-crate integration tests
```

## GPU compatibility tiers

| Tier | Meaning | Capability |
|---|---|---|
| **1** | ROCm officially supported | Full training + inference (Vulkan as fallback) |
| **2** | ROCm workable via `HSA_OVERRIDE_GFX_VERSION` | Inference; training community-tested |
| **3** | No usable ROCm path | Vulkan-only inference; no training |

## Building

```bash
cargo build --workspace
cargo test  --workspace
```

Pure-logic crates build and test on any OS. Linux-only paths (Unix-socket daemon,
`/sys` collectors, backend execution) are `#[cfg(target_os = "linux")]`-gated.

## License

Apache-2.0. See [`LICENSE`](LICENSE).
