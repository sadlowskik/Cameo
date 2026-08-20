# Cloud AMD-GPU validation (Phase 1)

The one thing never verified on real silicon: that the **serving path** actually
runs on a GPU rather than silently falling back to CPU, and that the Phase-1
placeholders (`HSA_OVERRIDE_GFX_VERSION`, llama.cpp flags, tier/VRAM constants)
are right for a real card. Detection is already corroborated on an APU; this
closes the serving half on a datacenter AMD GPU.

**Risk-order: Vulkan first, then ROCm.** The Vulkan image is CI-proven to build
and llama.cpp's Vulkan backend runs on AMD, so it reaches "a token on a real GPU"
with the least that can go wrong. ROCm is for peak throughput and its container
build is not yet CI-smoked — so building it on the box is itself a test.

## 0. Prereqs on the box

```bash
# AMD GPU visible to the kernel, and a container runtime.
ls /dev/kfd /dev/dri            # both must exist (amdgpu bound)
rocm-smi || true                # datacenter images usually have it
docker --version || podman --version
git clone https://github.com/sadlowskik/Cameo && cd Cameo
```

Note the card's arch — `rocminfo | grep -m1 gfx` (e.g. `gfx90a` MI200,
`gfx942` MI300, `gfx1100` consumer). Cameo's override DB may not have a datacenter
entry yet; that is one of the things this run confirms.

## 1. Vulkan image — first token on the GPU (low risk)

```bash
docker build -f containers/Containerfile -t cameo:vulkan .

# Run the daemon with GPU passthrough. /dev/dri is what Vulkan needs.
docker run -d --name cameo -p 9090:9090 \
  --device=/dev/dri --group-add video --group-add render \
  -v cameo-models:/var/lib/cameo/models cameo:vulkan
docker logs cameo | head        # note the generated bearer key

# Detection + serve a small model, all inside the container.
docker exec cameo cameo gpu-status          # must name the real card + a tier + VRAM
docker exec cameo cameo pull tinyllama
docker exec -d cameo cameo serve tinyllama --host 0.0.0.0 --port 8080 \
  --api-key "$KEY"                            # KEY = the console key, or set one
```

## 2. Confirm it is GPU-real, not CPU fallback

This is the whole point. In another shell, while a request runs:

```bash
# GPU utilisation + VRAM should climb when the model loads and generates.
watch -n1 rocm-smi

# Send a request and time it. GPU-fast vs painfully-slow is the tell.
curl -s http://localhost:8080/v1/chat/completions \
  -H "Authorization: Bearer $KEY" -H 'Content-Type: application/json' \
  -d '{"model":"tinyllama","messages":[{"role":"user","content":"hi"}]}'
```

- VRAM climbs on load + GPU % moves during generation → **GPU-real**. ✅
- VRAM flat, GPU idle, tokens crawl → **silent CPU fallback**. Capture the
  container logs and the exact `llama-server` command
  (`docker exec cameo cat /proc/$(pgrep -f llama-server)/cmdline | tr '\0' ' '`)
  so we can fix the backend/flag selection.

## 3. ROCm image — peak throughput (also tests the rocm build)

```bash
docker build -f containers/Containerfile --build-arg EDITION=rocm -t cameo:rocm .
docker run -d --name cameo-rocm -p 9091:9091 \
  --device=/dev/kfd --device=/dev/dri \
  --group-add video --group-add render --security-opt seccomp=unconfined \
  -v cameo-models:/var/lib/cameo/models cameo:rocm
docker exec cameo-rocm cameo run tinyllama --backend rocm
```

If the build fails, it is almost certainly a package name (`rocm-hip-runtime`,
`ggml-hip`) — record the pacman error and we fix the Containerfile. If it serves
but on CPU, the `HSA_OVERRIDE_GFX_VERSION` for this arch is the suspect: try
`cameo run … --hsa-override <ver>` (e.g. `9.0.10` for gfx90a) and see if that
lights up the GPU.

## 4. Systematic sweep (optional)

`scripts/phase1/` automates the matrix on a bare-metal box: `provision.sh`,
`build-llama.sh`, `benchmark.sh`, `record-combo.sh` — see
`scripts/phase1/RUNBOOK.md`. Use it to record tokens/sec per (card, backend,
quant) combo.

## What to bring back

For each card arch tested: the `cameo gpu-status` output, whether Vulkan and ROCm
each ran **on the GPU**, the working `HSA_OVERRIDE_GFX_VERSION` if any, and
tokens/sec. That confirms (or corrects) the Phase-1 placeholders and the override
DB for datacenter AMD parts — the last unvalidated piece of v1.
```
