# Cameo — Project Plan

**Mission: get real use out of any AMD hardware — from one old card lying in a drawer to a cluster — without fighting the stack.**

**Cameo** is an Arch Linux-based distro respin for running, serving, and training LLMs on AMD GPUs. It meets the hardware where it is and scales along a single continuum — a lone old/unsupported Radeon, a multi-GPU box, up to small multi-node clusters — with the same commands, the same tier model, and graceful degradation (Vulkan always works; ROCm accelerates when available).

This document is a build plan intended to be handed to an AI coding agent (Claude Code) to scaffold and implement the project incrementally. It is intentionally detailed and opinionated about sequencing: **build and validate the userspace inference core first, on real hardware, before building distro packaging, GUI, or any multi-node/kernel-level features.**

---

## 0. Guiding Principles

1. **Inference-first, not training-first.** The primary use case is running LLM inference efficiently on AMD GPUs, especially older/unsupported ones. Training is a real but secondary feature, gated behind hardware capability tiers.
2. **Vulkan is the universal baseline. ROCm is the accelerated optional path.** Every feature must work on the Vulkan path before ROCm-specific acceleration is layered on top. Nothing should require ROCm to function at all — ROCm should only make things faster on supported hardware.
3. **Respin, not from-scratch.** Cameo is built on top of Arch Linux via `archiso`. Do not attempt to compile a base system from scratch (no LFS-style bootstrapping). The value of this project is *integration and testing*, not low-level build purity.
4. **Auto-detect, but always overridable.** Every "smart" behavior (backend selection, networking strategy, expert placement, GPU tier detection) must have a sane automatic default AND an explicit manual override path (config file and/or CLI flag).
5. **One core, multiple frontends.** All functionality lives in a core service/library with a stable internal API. The CLI and GUI are both thin clients against that core — never implement a feature in the CLI or GUI that isn't first a core capability.
6. **Ship something real before shipping everything imagined.** This plan has a large long-term scope (see Section 9, "Out of Scope for v1"). Resist scope creep into v1. The success criterion for v1 is: *a user can boot Cameo, and run inference on an LLM (including large MoE models via offloading) on an AMD GPU — old or new — noticeably more easily than on stock Arch/Ubuntu with manual ROCm/Vulkan setup.*

---

## 1. Repository Structure

Single monorepo, workspace-style layout:

```
cameo/
├── core/                 # Core service — Rust. All real logic lives here.
│   ├── gpu-detect/       # AMD GPU detection & capability tiering
│   ├── backend-vulkan/   # Vulkan/llama.cpp inference backend wrapper
│   ├── backend-rocm/     # ROCm inference + training backend wrapper
│   ├── moe-harness/      # MoE expert offloading/placement logic
│   ├── quant-tools/      # Quantization / post-training optimization
│   ├── net-strategy/     # Multi-node networking strategy selection (v2)
│   └── api/              # Stable internal API surface (used by CLI + GUI)
├── cli/                  # `cameo` command-line tool — thin client over core/api
├── gui/                  # Dashboard/control panel — thin client over core/api
├── archiso/              # Arch ISO build profile, packages, install scripts
├── containers/           # Dockerfiles/Podman configs, AMD GPU passthrough helpers
├── k8s/                  # Device plugin config, Helm charts (v2)
├── docs/
└── tests/
```

Rationale: tightly coupled components (CLI calls core API, archiso bundles core binaries) benefit from atomic commits across boundaries at this project's current scale (solo developer, pre-v1). Revisit splitting into multiple repos only if independent release cadences or outside contributor access-control needs emerge later.

---

## 2. Technology Choices

| Component | Choice | Rationale |
|---|---|---|
| Core service language | **Rust** | Memory safety without GC, good for systems-level work, matches existing interest in Rust (Daedalus/Knossos context, OS-dev interest) |
| Base distro | **Arch Linux** | Best-maintained ROCm packaging (AUR + official repos), rolling release avoids stale ROCm versions that plague Debian/Ubuntu LTS bases |
| ISO build tool | **archiso** | Standard, well-documented Arch ISO building tool |
| Inference engine | **llama.cpp** | Mature Vulkan backend (broadest AMD compatibility) and ROCm/HIP backend; strong MoE and quantization (GGUF) support already |
| Training | **PyTorch (ROCm build)** | Standard, ROCm-supported |
| CLI framework | Rust (`clap`) | Native, fast, no separate runtime |
| GUI framework | **TBD — evaluate Tauri vs. web-based (React) dashboard served locally** | Needs a decision spike; not blocking for v1 core work |
| Container runtime | **Podman** (Docker-compatible) preferred over Docker | Rootless-by-default fits a security-conscious distro better |
| TUI installer | Rust (`ratatui`) | Consistent with core language choice |

---

## 3. GPU Compatibility Tier Model

Cameo must never silently fail on unsupported hardware. Every install should classify the detected AMD GPU into a tier and communicate that tier clearly to the user.

- **Tier 1 — ROCm officially supported.** Full training + inference via ROCm, Vulkan available as fallback/comparison.
- **Tier 2 — ROCm unsupported but known-workable via `HSA_OVERRIDE_GFX_VERSION`.** Inference-focused; training available but explicitly flagged as "community-tested, not guaranteed." Maintain a community-sourced override compatibility database (GPU model → known-good override value), auto-suggested at install/runtime.
- **Tier 3 — No usable ROCm path.** Vulkan-only. Full inference support (this is the core "any AMD GPU" promise), no training.

Detection and tiering logic lives in `core/gpu-detect/`. This must run at install time (TUI installer step) and be re-checkable at runtime (`cameo gpu-status`).

---

## 4. v1 Feature Scope (Build in This Order)

### Phase 1 — Validate the Core Path Manually (no code yet)
- Install vanilla Arch on the target AMD machine.
- Get llama.cpp's Vulkan backend building and running a real model (start with Qwen3.8-27B — AMD has published Day-0 Vulkan benchmarks for it as a reference point).
- Separately, attempt ROCm install and get llama.cpp's ROCm/HIP backend running, to confirm the working version combination for the specific GPU.
- Record the exact working combination (kernel version, ROCm version, driver version, llama.cpp build flags) — this becomes the first entry in the "known-good combination" matrix that Phase 2 automates.

**Do not proceed to Phase 2 until Phase 1 produces a real, benchmarked, working manual setup.**

### Phase 2 — Core Service (`core/`)
- `gpu-detect`: detect AMD GPU model, VRAM, driver version; classify into Tier 1/2/3 per Section 3.
- `backend-vulkan`: wrapper around llama.cpp Vulkan backend — model loading, inference serving (basic HTTP/local API), tok/s reporting.
- `backend-rocm`: wrapper around llama.cpp ROCm backend + PyTorch ROCm for training; only activates on Tier 1/2 hardware.
- Runtime fallback logic: if ROCm path fails at runtime (not just absent, but errors), fall back to Vulkan automatically and log why.
- `quant-tools`: GGUF quantization pipeline (wrap `llama.cpp`'s existing quantization tools initially — don't reinvent); support for common quant levels (Q4, Q5, Q8 etc.).
- `api/`: define the stable internal API (likely gRPC or a local Unix socket JSON-RPC) that CLI and GUI will both consume. Design this early and treat it as a contract — this is what keeps CLI/GUI as "thin clients."

### Phase 3 — MoE Harness (`core/moe-harness/`)
- Expert offloading: automatically decide which experts live in VRAM vs. system RAM based on detected VRAM capacity (from `gpu-detect`) and model metadata.
- MoE-aware quantization defaults (may differ from dense-model quantization defaults — research this, don't assume dense-model settings apply).
- Routing-aware caching (v1.x stretch within Phase 3, not blocking): track expert activation frequency per session, prefer keeping "hot" experts in faster memory.
- Expose via a simple interface: `cameo run <moe-model>` should "just work" without the user manually configuring offloading.
- **Explicitly deferred from v1:** kernel-level expert placement (see Section 9). Build this in userspace first; only consider kernel-level implementation after userspace heuristics are proven to matter and userspace overhead is measured and shown to be a real bottleneck.

### Phase 4 — CLI (`cli/`)
- `cameo install` — invoked from the TUI installer or standalone; drives GPU detection, tier classification, backend setup.
- `cameo gpu-status` — show detected GPU(s), tier, active backend.
- `cameo run <model>` — run inference, auto-selecting backend per tier, auto-handling MoE offloading if applicable.
- `cameo quantize <model> --level Q4` — post-training quantization.
- `cameo train <config>` — training entrypoint, gated by tier (refuses cleanly with explanation on Tier 3).
- All commands should have machine-readable (`--json`) output modes for scripting/enterprise automation use.

### Phase 5 — TUI Installer (`archiso/` + installer binary)
- Boot → detect GPU → show tier and what that means in plain language → confirm backend selection (with override option) → proceed with base install.
- Package selection: bundle kernel + amdgpu driver + pinned ROCm version (per tier) + Vulkan userspace + llama.cpp (both backends prebuilt) + PyTorch ROCm build.

### Phase 6 — Container Support (`containers/`)
- Podman preferred, Docker-compatible.
- AMD GPU passthrough helper: wrap the `--device=/dev/kfd --device=/dev/dri` + group permission boilerplate so `cameo docker-run <image>` (or equivalent Podman invocation) "just works" with GPU access.
- Publish Cameo-maintained base container images with the same pinned ROCm/Vulkan versions as the host OS ships, so containerized workloads get the same compatibility guarantees.

### Phase 7 — GUI (`gui/`)
- Decision spike first: Tauri (native, lighter) vs. local web dashboard (React, easier iteration, matches broader web tooling familiarity).
- v1 GUI scope: inference playground (chat interface, prompt testing), basic training dashboard (loss curve, GPU/VRAM utilization, throughput) if training is exercised, model management (browse/download/quantize models visually).
- GUI talks only to `core/api` — never touches hardware/backends directly.

---

## 5. Post-v1 / v2 Scope (Explicitly Deferred, Do Not Build Yet)

- **Multi-machine/multi-node GPU pooling** (small-datacenter use case).
  - Bandwidth-conscious training strategies for consumer networking: gradient compression, pipeline parallelism, less-frequent-sync (local SGD-style) approaches.
  - Full datacenter-interconnect support (InfiniBand, high-speed Ethernet) with standard data-parallel sync.
  - Auto-detection of link quality/type to choose strategy, with manual override to force a specific strategy regardless of detected hardware.
- **Kubernetes integration**: AMD device plugin packaging/improvement, Helm charts for common inference/training workloads, potential positioning of Cameo itself as a K8s node OS (Talos/Flatcar-style).
- **Kernel-level MoE harness**: a kernel module that manages expert placement decisions closer to the memory management layer, using token-level routing signals, to avoid userspace/kernel round-trip overhead for placement decisions. High-risk (kernel bugs can crash the whole system, novel territory with no known prior art for this specific application). Only pursue after userspace MoE harness (Phase 3) is mature and profiling shows userspace overhead is a real, measured bottleneck worth the risk.
- **Post-training optimization suite beyond basic quantization**: pruning, distillation pipelines tailored to specific target GPUs.

---

## 6. Cross-Cutting Concerns (Decide Early, Cheap Now / Expensive Later)

These should get real decisions during Phase 2, even though they're not "features":

- **License**: decide project license (e.g., MIT/Apache-2.0 vs GPL) — affects contributor and enterprise adoption. Recommend Apache-2.0 for compatibility with most of the ML tooling ecosystem (PyTorch, llama.cpp, etc. use permissive licenses).
- **Update/security model**: how are Cameo's own packages signed and verified; how are pinned ROCm/kernel combinations updated deliberately rather than drifting with upstream Arch rolling updates.
- **Logging/observability**: structured logs from day one in `core/` (not bolted on later) — training crashes, OOM events, GPU errors all need to be diagnosable.
- **Model format support**: v1 focuses on GGUF (via llama.cpp) for inference. Note safetensors/raw PyTorch support as a known gap for training workflows — don't design `core/` in a way that makes adding this later painful.
- **Multi-user support**: v1 can reasonably assume single-user. Note this as a known limitation, don't architect against it being added later (e.g., avoid hardcoding single-user assumptions into file paths/permissions in a way that would require a rewrite).

---

## 7. Reference Hardware / Validation Targets

- Primary dev/test machine: Korbin's home PC (AMD GPU — confirm exact model before Phase 1).
- Reference model for Vulkan backend validation: **Qwen3.8-27B** — dense model, AMD has published Day-0 Vulkan benchmarks (~24.5 tok/s on Ryzen AI Max+ 395, ~51.8 tok/s on Radeon AI PRO R9700) usable as a sanity-check baseline for whether Cameo's wrapper is performing comparably to a raw manual setup.
- Reference model for MoE harness validation: pick a mid-size MoE model with published GGUF quantizations (e.g., something in the Mixtral or DeepSeek-family open-weight range) once Phase 3 begins.

---

## 8. Definition of Done for v1

A user can:
1. Boot the Cameo ISO on a machine with any AMD GPU.
2. Get an accurate, plain-language readout of their GPU's support tier during install.
3. End up with a working system where `cameo run <model>` serves inference over Vulkan (or ROCm if Tier 1/2 and available) without manual ROCm/ Vulkan/driver wrangling.
4. Run a MoE model larger than their VRAM would naively allow, via automatic expert offloading, without manual configuration.
5. Optionally train (if Tier 1/2) using ROCm, with the same "no manual version-pinning wrangling" experience.
6. Do all of the above via CLI (scriptable) and, if Phase 7 is reached, via a GUI dashboard.
7. Optionally run the same capabilities inside a container with correct GPU passthrough.

Multi-node clustering, Kubernetes integration, and kernel-level MoE placement are explicitly **not** required for v1 completion.

---

## 9. Notes for the Coding Agent

- Do not attempt Phase 2+ without confirmed Phase 1 results (a real working manual Vulkan + ROCm setup on the actual target GPU). If Phase 1 hasn't been completed and hardware isn't available in the current environment, say so clearly rather than proceeding on assumptions.
- Prefer wrapping/orchestrating existing mature tools (llama.cpp, PyTorch, archiso) over reimplementing their internals. Cameo's value is integration, tiering, and automation — not reinventing inference engines.
- Every "smart" auto-detection feature needs a corresponding manual override — treat this as a hard requirement, not a nice-to-have, when implementing any Phase.
- Flag scope creep back to the user explicitly if a request during implementation starts pulling in v2-scope work (Section 5) — don't silently expand v1.
