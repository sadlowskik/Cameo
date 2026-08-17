# Phase 1 Runbook — Validate Cameo's core path on real AMD hardware

**Goal:** on a fresh Arch instance with an AMD GPU, get llama.cpp's **Vulkan** and
(if the card supports it) **ROCm** backends building and benchmarked, then record
the exact working combination. That `known-good-combo.json` is the deliverable
that unblocks Phase 2 — the plan forbids Phase 2+ without it (plan §1, §9).

This runbook is meant to be **copy-paste** on a rented cloud AMD instance so the
paid GPU time is short and productive.

## 0. Prerequisites
- A cloud instance (or bare-metal box) with an AMD GPU, running **Arch Linux**
  (or an Arch container/chroot with `/dev/dri` and, for ROCm, `/dev/kfd` exposed).
- `sudo` and network access.
- This repo checked out on the instance.

> Single-GPU note: nothing here needs a display; it's all headless compute. If you
> rent a datacenter card, ROCm behavior won't perfectly mirror a consumer card —
> that's fine for validating the *plumbing*; consumer-card specifics get their own
> matrix entry later.

## 1. Provision
```bash
cd scripts/phase1
chmod +x *.sh
./provision.sh
```
Installs build tools, Vulkan userspace, and (best-effort) ROCm. If ROCm packages
fail, the box is treated as **Tier 3** (Vulkan-only) and the rest still works.

Quick sanity checks:
```bash
vulkaninfo --summary   # should list your AMD GPU
rocminfo | grep gfx    # Tier 1/2 only; prints e.g. gfx1030
```

## 2. Build the backends
```bash
# Optionally pin llama.cpp and/or set the target explicitly:
#   export CAMEO_LLAMA_REF=b3xxxx
#   export CAMEO_AMDGPU_TARGET=gfx1030
./build-llama.sh
```
Always builds the Vulkan backend; builds the ROCm backend when `rocminfo` is present.
Writes `artifacts/build.env`.

## 3. Benchmark
```bash
# Provide a reference GGUF model — a local path or a URL:
export CAMEO_MODEL_PATH=/path/to/model.gguf
#   or: export CAMEO_MODEL_URL=https://.../model.gguf
# Tier 2 cards: set the runtime override you intend to ship:
#   export CAMEO_HSA_OVERRIDE=10.3.0
./benchmark.sh
```
Runs `llama-bench` per built backend, writing `artifacts/bench-<backend>.json`.
Compare tok/s against the plan's published reference numbers (plan §7) to confirm
Cameo's wrapper will perform comparably to a raw manual setup.

## 4. Record the known-good combination
```bash
./record-combo.sh
```
Assembles `artifacts/known-good-combo.json`: GPU model, gfx target, HSA override,
kernel / ROCm / mesa / vulkan-radeon versions, llama.cpp commit, and tok/s per
backend.

## 5. Feed results back into the repo
- Copy `artifacts/known-good-combo.json` off the instance.
- Update `core/gpu-detect/data/overrides.toml` with the **confirmed** tier and
  (Tier 2) `hsa_override` for this `gfx_arch`, replacing the illustrative seed.
- Replace the illustrative fixtures in `core/gpu-detect/tests/fixtures/` with the
  real `lspci -nn` / `rocminfo` captures from this run:
  ```bash
  lspci -nn > lspci_<card>.txt
  rocminfo  > rocminfo_<card>.txt   # Tier 1/2 only
  ```

**Phase 1 is done when `known-good-combo.json` exists and the benchmarks are
sane.** Only then does Phase 2 backend implementation begin.
