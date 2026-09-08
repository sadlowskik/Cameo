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

Cameo meets your hardware where it is. **Vulkan is the broad compatibility
baseline** for AMD cards with a working Vulkan stack. **ROCm is an optional
accelerator** for supported cards. Cameo detects the card, classifies what the
software expects it can do, and makes that recommendation overridable.

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

> **Status: beta, pre-v1.** The software core and delivery pipelines exist, but the
> current checkout has no retained certified-hardware record. Treat release tags as
> immutable beta snapshots and `main` as moving. See [release
> readiness](docs/release-readiness.md) for observed evidence and blockers.

## Quickstart

Download the ISO, flash a USB, plug it into the AMD box, set an account, play.
After that the machine does not need the internet. Full walkthrough:
[docs/quickstart.md](docs/quickstart.md).

### ISO appliance

Flash an `.iso` from [GitHub Releases](https://github.com/sadlowskik/Cameo/releases)
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
  moe-harness/        MoE expert-offload placement primitives
  net-strategy/       multi-node layout primitives
cli/                  `cameo` command-line tool
cameod/               `cameod` control-plane daemon: browser console + JSON API
archiso/              Arch ISO build profile (ships cameo + cameod)
containers/           Containerfile + entrypoint (the recommended delivery)
k8s/                  experimental device-plugin / Helm scaffolding
scripts/phase1/       automated Phase 1 hardware validation
docs/                 current reference, evidence snapshot, and documentation index
```

## Testers

Cameo is open source. The people who run it on real AMD hardware before v1 get
a permanent line in git: [the roll](docs/testers.md). File a
[hardware report](.github/ISSUE_TEMPLATE/hardware-report.yml) and check consent
if you want your name there.

## Documentation

- [Documentation index](docs/README.md) — the maintained reference set.
- [Productization plan](PRODUCTIZATION_PLAN.md) — the single Cameo + Knossos roadmap.
- [Release readiness](docs/release-readiness.md) — current observed evidence and blockers.
- [Generated capabilities](docs/capabilities.md) — shared manifest; also available with `cameo capabilities`.
- [Quickstart](docs/quickstart.md) · [Architecture](docs/architecture.md) · [HTTP API](docs/api-reference.md)

## Status

The fixture-tested software core includes GPU detection and tier classification,
placement, model management, the CLI, the authenticated `cameod` console/API,
durable endpoint intent, preview recommendation/setup, request-level mesh
scheduling/pairing, container packaging, and ISO/update pipelines. The current
checkout does **not** contain a retained certified-hardware record, and the public
tester roll is still empty. Certificate-bound mesh identity, distributed model
sharding, full storage/update fault qualification, and Kubernetes support are not
shipped v1 capabilities. See the [canonical plan](PRODUCTIZATION_PLAN.md).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Private security reports:
[SECURITY.md](SECURITY.md).

## License

Apache-2.0. See [`LICENSE`](LICENSE).

AMD, Radeon, and ROCm are trademarks of Advanced Micro Devices, Inc. Vulkan is a
trademark of the Khronos Group. Cameo is an independent project, not affiliated with or
endorsed by AMD or any trademark owner.
