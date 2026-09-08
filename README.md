<div align="center">

# Cameo

**Your AMD card. Serves LLMs.**

An Arch-based OS that turns supported AMD GPUs into OpenAI-compatible endpoints.
Download it, plug it in, set an account, play — no internet after that. Network
is for extra models, Cameo Mesh, and opening the console from outside the house.

[![License](https://img.shields.io/badge/license-Apache--2.0-FF7A1A)](LICENSE)
[![Status](https://img.shields.io/badge/status-beta%20(pre--v1)-FFD08A)](#status)
[![Backends](https://img.shields.io/badge/backends-Vulkan%20·%20ROCm-3A3833)](#gpu-tiers)
[![Site](https://img.shields.io/badge/site-cameoconstruct.xyz-3A3833)](https://cameoconstruct.xyz/)

</div>

---

Cameo meets your hardware where it is. **Vulkan is the universal baseline** — it
runs on *any* AMD card. **ROCm is an optional accelerator** that only ever makes the
supported cards faster; nothing requires it. Cameo detects the card, classifies what
it can do, and serves.

Hardware coverage is a product goal, not a certification claim: actual support
depends on the card, driver, Vulkan implementation, model, and memory available.
Use `cameo gpu-status` and the hardware validation runbook before treating a
particular machine as release-qualified.

- **Runs on the card you already have.** A gfx803 RX 580 from 2017 serves inference
  over Vulkan. A 7900 XTX or MI210 trains and serves over ROCm. Same tool.
- **Auto-detect, always overridable.** Every default — tier, backend, placement — is
  chosen for you and can be forced by a flag or config.
- **Administer it from a browser.** `cameod` starts on boot and serves the console
  on `:9090` — GPUs, endpoints, and a playground. No extra web stack.

A coding harness (Knossos) and a coding model (Daedalus) can point at this box
later. They are not required to install or chat.

> **Status: beta, pre-v1.** The core (detection, placement, CLI, console, container,
> ISO) is built and the command surface below is real. GPU execution is validated on
> AMD hardware through the [Phase 1 runbook](scripts/phase1/RUNBOOK.md); treat pinned
> releases as the stable line and `main` as moving. See [Status](#status).

## Quickstart

Download the ISO, flash a USB, plug it into the AMD box, set an account, play.
After that the machine does not need the internet. Full walkthrough:
[docs/quickstart.md](docs/quickstart.md).

### ISO appliance

Flash an `.iso` from [GitHub Releases](https://github.com/sadlowskik/Cameo/releases/latest)
(not the Source code zip). **Universal** includes Vulkan and ROCm; **lite** is the
smaller Vulkan-only image. Both editions also include the Rust-native Knossos
agentic harness. Standalone Windows Knossos is on
[cameoconstruct.xyz](https://cameoconstruct.xyz/).
Rufus **DD Image mode**, Etcher, or `dd`. **Disable Secure Boot** if the stick is
ignored. Boot **Install Cameo to disk**. Log in, open `http://cameo.local:9090`,
press **Start qwen2.5-0.5b and chat**. No Wi-Fi for that. Network is later: extra
models, or attaching this box to Cameo Mesh (`iwctl` on that node only). See
[quickstart](docs/quickstart.md).

Building the ISO yourself still needs an Arch host:

```bash
git clone https://github.com/sadlowskik/Cameo
sudo ./scripts/build-iso.sh                       # or: sudo CAMEO_EDITION=lite ./scripts/build-iso.sh
sudo dd if=archiso/out/cameo-*.iso of=/dev/sdX bs=4M status=progress oflag=sync
```

### When you want a network

`cameo pull` gets extra models. Cameo Link pairs boxes into Cameo Mesh; the
legacy `cameo fleet` command remains available for static node lists. The same
`:9090` console, forwarded, if you want it from outside the house.

### Container (developers)

Host owns the GPU driver. Not the appliance path.

```bash
podman build -f containers/Containerfile -t cameo:vulkan .
podman run --rm -p 9090:9090 -v cameo-models:/var/lib/cameo/models \
  --device=/dev/kfd --device=/dev/dri --group-add video --group-add render \
  cameo:vulkan
```

## The CLI

`cameo` is a thin client over the core. Every command works identically whether you
installed the ISO, ran the container, or built from source.

```bash
cameo gpu-status                 # detected GPU(s), topology, tier, chosen backend
cameo recommend --workload agent # choose a checksum-pinned model for this hardware
cameo setup --workload agent     # preflight, verify, configure, serve on loopback
cameo serve qwen2.5-0.5b         # starter model, already on the image (no network)
cameo pull tinyllama             # optional — fetch a model when you have a network
cameo run   tinyllama            # one-shot inference
cameo plan  qwen2.5-32b          # show the placement plan without running it
cameo train mistral-7b           # needs torchrun (not on the ISO/container)
cameo quantize model.gguf Q4_K_M # quantize to a target level
cameo model ls                   # list the local model cache
cameo fleet place qwen2.5-32b    # preview placement across a static node list
cameo install-plan               # packages this card would use (does not install)
```

## What first boot prints

```
================  Cameo  ================
GPU 0  Radeon RX 580 8G
  pci  0000:01:00.0
  vram 8192 MiB
  arch gfx803
  tier ● Tier 3   no training
  why  gfx803 has no usable ROCm path:
       Vulkan-only inference, no training.
-----------------------------------------
Web console:  http://192.168.1.40:9090
  Open it from a browser on this LAN. No internet required.
  Starter model qwen2.5-0.5b is on disk — open the console and chat.
```

Even a tier-3 drawer card serves. The tier is a smart default, not a verdict — flags
and config always win.

## GPU tiers

Cameo never silently fails on unsupported hardware. It classifies the card and says so.

| Tier | Meaning | Capability |
|---|---|---|
| **1** | ROCm officially supported (7900 XTX, MI210, …) | Full training + inference (Vulkan fallback) |
| **2** | ROCm workable via `HSA_OVERRIDE_GFX_VERSION` (RX 6800, 6700 XT, …) | Inference; training community-tested |
| **3** | No usable ROCm path (RX 580, APUs, …) | Vulkan-only inference; no training |

Check yours with `cameo gpu-status`.

## One card → Cameo Mesh

Cameo Mesh fronts several `cameod` machines as one request-level pool. Cameo Link
phones home from each node, paired with a single-use code and a per-node identity;
the scheduler places each complete request on one healthy, trusted node. It does
not pool VRAM or shard one inference across LAN machines. See the
[Cameo Mesh guide](docs/cameo-mesh.md) for pairing, strict dispatch, durability,
and the current mTLS limitation.

## Building from source

```bash
cargo build --workspace
cargo test  --workspace
```

Pure-logic crates build and test on any OS. Linux-only paths (the Unix-socket daemon,
`/sys` collectors, backend execution) are `#[cfg(target_os = "linux")]`-gated.

```
core/                 Rust — all real logic
  gpu-detect/         AMD GPU detection, multi-GPU topology, Tier 1/2/3 classify
  config/             config + override precedence (flag > file > auto-detect)
  placement/          the brain: (topology × model × task) → plan → command
  models/             model cache + acquisition (cameo pull), shared by CLI + daemon
  containers/         AMD GPU passthrough recipe for Podman/Docker
  api/                versioned JSON-RPC message types (the control-plane contract)
  backend-vulkan/     llama.cpp Vulkan executor (universal baseline)
  backend-rocm/       llama.cpp ROCm + PyTorch training executor (Tier 1/2)
  quant-tools/        GGUF quantization (wraps llama-quantize)
  moe-harness/        MoE expert offloading                   (Phase 3)
  net-strategy/       multi-node networking strategy          (v2)
cli/                  `cameo` command-line tool
cameod/               `cameod` control-plane daemon: browser console + JSON API
archiso/              Arch ISO build profile (ships cameo + cameod)
containers/           Containerfile + entrypoint (the recommended delivery)
k8s/                  device plugin / Helm charts             (v2)
scripts/phase1/       automated Phase 1 hardware validation
docs/                 architecture, tiers, API, definition-of-done
```

## Testers

Cameo is open source. The people who run it on real AMD hardware before v1 get
a permanent line in git: [the roll](docs/testers.md). File a
[hardware report](.github/ISSUE_TEMPLATE/hardware-report.yml) and check consent
if you want your name there.

## Documentation

- [Production roadmap audit](docs/production-audit.md) — implemented work, fixes, and remaining release blockers.
- [Generated capabilities](docs/capabilities.md) — shared manifest; also available with `cameo capabilities`.

- [Cameo Mesh](docs/cameo-mesh.md) - pair nodes and dispatch whole requests securely.

- [Quickstart](docs/quickstart.md) — download, plug in, account, play offline.
- [HTTP API reference](docs/api-reference.md) — the `cameod` control-plane surface.
- [Harness integration](docs/harness-integration.md) — point Knossos at Cameo.
- [Updating](docs/updating.md) — container / installed / ISO update paths.
- [Secure Boot](docs/secure-boot.md) — the shim chain, and the fallback that works now.
- [Architecture](docs/architecture.md) · [Tiers](docs/tiers.md) · [Road to v1](docs/remediation-plan.md)

## Status

The hardware-independent core is complete: GPU detection and tier classification,
override precedence, the placement engine, the model cache, the CLI, the `cameod`
console and its versioned API, the container, and the ISO profile. Vulkan and ROCm
execution is validated on a cloud AMD instance through the automated
[Phase 1 runbook](scripts/phase1/RUNBOOK.md). MoE expert offloading (Phase 3) and
certificate-bound mesh identity, distributed model sharding, and Kubernetes are
still in progress. Request-level Cameo Mesh scheduling and pairing are preview.
See
[`CAMEO_PROJECT_PLAN.md`](CAMEO_PROJECT_PLAN.md) for the full plan.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Private security reports:
[SECURITY.md](SECURITY.md).

## License

Apache-2.0. See [`LICENSE`](LICENSE).

AMD, Radeon, and ROCm are trademarks of Advanced Micro Devices, Inc. Vulkan is a
trademark of the Khronos Group. Cameo is an independent project, not affiliated with or
endorsed by AMD or any trademark owner.
