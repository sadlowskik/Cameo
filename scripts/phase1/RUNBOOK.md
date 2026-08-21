# Phase 1 Runbook — Validate Cameo's core path on real AMD hardware

**Goal:** on a fresh Arch instance with an AMD GPU, run the **packaged** llama.cpp
(Vulkan always; ROCm/HIP when the card can) and record tok/s plus the working
driver/mesa/ROCm versions. That `known-good-combo.json` is the deliverable that
calibrates `core/placement` flags and `overrides.toml`. The ISO already ships
Arch's `llama-cpp` + ggml plugins — do not compile llama.cpp from `master` as
the default.

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
Installs Vulkan userspace, packaged `llama-cpp` + `ggml-cpu` + `ggml-vulkan`, and
(best-effort) ROCm + `ggml-hip`. If ROCm packages fail, the box is treated as
**Tier 3** (Vulkan-only) and the rest still works.

Quick sanity checks:
```bash
vulkaninfo --summary   # should list your AMD GPU
command -v llama-bench llama-server llama-cli
rocminfo | grep gfx    # Tier 1/2 only; prints e.g. gfx1030
```

## 2. Benchmark (default — packaged binaries)
```bash
# Provide a reference GGUF model — a local path or a URL:
export CAMEO_MODEL_PATH=/path/to/model.gguf
#   or: export CAMEO_MODEL_URL=https://.../model.gguf
# Tier 2 cards: set the runtime override you intend to ship:
#   export CAMEO_HSA_OVERRIDE=10.3.0
./benchmark.sh
```
Runs `llama-bench` from PATH, writing `artifacts/bench-vulkan.json` and, when
`rocminfo` is present, `artifacts/bench-rocm.json`. Compare tok/s against the
plan's published reference numbers (plan §7).

## 3. Optional: compile llama.cpp from source
Only if you need to bisect a flag against upstream. This is **not** what the ISO
runs.

```bash
export CAMEO_BUILD_LLAMA=1
export CAMEO_LLAMA_REF=0123456789abcdef0123456789abcdef01234567   # 40-char SHA
# export CAMEO_AMDGPU_TARGET=gfx1030
./build-llama.sh
./benchmark.sh    # uses the trees in artifacts/build.env
```

## 4. Record the known-good combination
```bash
./record-combo.sh
```
Assembles `artifacts/known-good-combo.json`: GPU model, gfx target, HSA override,
kernel / ROCm / mesa / vulkan-radeon versions, llama.cpp source (packaged vs
commit), and tok/s per backend.

## 5. Feed results back into the repo
- Copy `artifacts/known-good-combo.json` off the instance.
- Update `core/gpu-detect/data/overrides.toml` with the **confirmed** tier and
  (Tier 2) `hsa_override` for this `gfx_arch`, replacing the illustrative seed.
- Replace the illustrative fixtures in `core/gpu-detect/tests/fixtures/` with the
  real `lspci -D -nn` / `rocminfo` captures from this run:
  ```bash
  lspci -D -nn > lspci_<card>.txt
  rocminfo  > rocminfo_<card>.txt   # Tier 1/2 only
  ```

**Phase 1 is done when `known-good-combo.json` exists and the benchmarks are
sane.** Use those numbers to correct placeholder constants and spawn flags in
`core/placement` — do not compile a second llama.cpp for the product.
