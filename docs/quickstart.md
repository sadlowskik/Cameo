# Cameo quickstart

Run and serve an LLM on the hardware you have, three ways from one build. Pick the
delivery that fits; the daemon, console, and CLI are identical across all three.

## Container (recommended — runs anywhere)

The container is the hero artifact: it runs on AMD, NVIDIA/Intel (Vulkan), or CPU,
and the host owns the GPU driver.

```bash
# build the universal Vulkan image (or --build-arg EDITION=rocm for AMD accel)
podman build -f containers/Containerfile -t cameo:vulkan .

# run it; models persist in the named volume, AMD GPUs pass through as shown
podman run --rm -p 9090:9090 -v cameo-models:/var/lib/cameo/models \
  --device=/dev/kfd --device=/dev/dri --group-add video --group-add render \
  cameo:vulkan
```

The entrypoint prints the console URL and a generated key. Then:

```bash
# from your machine — pull a small model and serve it
curl -X POST http://<host>:9090/api/servers \
  -H "Authorization: Bearer $KEY" -d '{"model":"tinyllama","params":1.1}'

# chat through the one OpenAI door
curl http://<host>:9090/v1/chat/completions -H "Authorization: Bearer $KEY" \
  -d '{"model":"tinyllama","messages":[{"role":"user","content":"hi"}]}'
```

…or just open `http://<host>:9090` and drive it from the console (GPUs, endpoints,
the chat playground, model cache).

## ISO appliance (a box that *is* the console)

Build on an Arch host (`mkarchiso`, root):

```bash
sudo ./scripts/build-iso.sh                        # full (ROCm + Vulkan)
sudo CAMEO_EDITION=lite ./scripts/build-iso.sh     # Vulkan-only, small
sudo dd if=archiso/out/cameo-*.iso of=/dev/sdX bs=4M status=progress oflag=sync
```

Boot it. First boot prints your GPU's tier plus the console URL and key — open it
from your own machine's browser (the box is a headless, key-protected home
console). See [`archiso/README.md`](../archiso/README.md).

## Bare install (persist to disk)

From the live medium, `cameo-install` installs Cameo to disk so accounts, models,
and config survive a reboot.

## The CLI (scriptable, `--json` everywhere)

```bash
cameo gpu-status                 # detected GPUs, tiers, backend
cameo pull tinyllama             # fetch a model into the cache
cameo serve tinyllama --params 1.1   # OpenAI endpoint on :8080
cameo model ls|du|rm|gc          # manage the model cache
cameo fleet status --node a:9090 --node b:9090   # front several boxes
```

Every command takes `--dry-run` (prints the exact plan + command, no GPU needed)
and `--json`. Point a harness at Cameo via [`harness-integration.md`](harness-integration.md);
update via [`updating.md`](updating.md); the HTTP surface is
[`api-reference.md`](api-reference.md).
