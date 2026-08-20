# Cameo quickstart

Cameo is an appliance. Download the ISO, flash a USB, plug it into the AMD
machine, set an account, play. After that the box does not need the internet.
Network is for extra models, other Cameo nodes, and opening the console from
outside the house.

## 1. Download

From [GitHub Releases](https://github.com/sadlowskik/Cameo/releases):

- **Universal** — every AMD card. Ships ROCm, boots on Vulkan, uses ROCm only
  when the card can train. Take this if you are unsure.
- **Lite** — Vulkan-only. For RX 580s, APUs, and anything with no ROCm path.

Verify against `SHA256SUMS` before flashing.

## 2. Flash a USB

Windows: [Rufus](https://rufus.ie/) or [Etcher](https://etcher.balena.io/).
Linux / macOS: Etcher, or:

```bash
sudo dd if=cameo-*.iso of=/dev/sdX bs=4M status=progress oflag=sync
```

## 3. Plug it in, set an account

Boot the AMD machine from the USB. **Install Cameo to disk** is the default
menu entry. The installer is offline: it copies the live image, it downloads
nothing. It shows the detected GPU and tier, then asks for a disk, an admin
username, and a password. Type the disk name to confirm. Reboot, remove the USB.

The same flow from a live shell:

```bash
cameo-install-guided          # interactive
cameo-install                 # dry run — prints the plan, changes nothing
cameo-install --commit        # do it (still confirms the disk name)
```

## 4. Play, offline

First boot prints the GPU tier, the console URL, and a key. From a laptop or
phone on the same LAN:

1. Open `http://<the-box>:9090`
2. Enter the key once (the dashboard remembers it)
3. A starter model (`qwen2.5-0.5b`, ~380 MiB) is already in
   `/var/lib/cameo/models`. Start it from the console, or:

```bash
cameo gpu-status
cameo serve qwen2.5-0.5b
```

Serving never phones home. Extra weights: drop a `.gguf` in that directory
(a USB stick is enough) or `cameo pull` when you have a network.

## When you want a network

| Need | What to use |
|---|---|
| More models from the internet | `cameo pull tinyllama` (or any alias / HuggingFace spec) |
| Several Cameo boxes as a fleet | `cameo fleet status --node a:9090 --node b:9090` |
| Console from outside the house | The same `:9090`, forwarded or tunneled however you already expose a LAN service |

One machine, one card, one house: stay air-gapped after the ISO download.

## Developers

The CLI, daemon, and console are identical in the container. The host owns the
GPU driver. This is not the appliance path.

```bash
podman build -f containers/Containerfile -t cameo:vulkan .
podman run --rm -p 9090:9090 -v cameo-models:/var/lib/cameo/models \
  --device=/dev/kfd --device=/dev/dri --group-add video --group-add render \
  cameo:vulkan
```

Build the ISO yourself on an Arch host (`mkarchiso`, root):

```bash
sudo ./scripts/build-iso.sh                        # universal
sudo CAMEO_EDITION=lite ./scripts/build-iso.sh     # lite
```

Every CLI command takes `--dry-run` and `--json`. Point a harness at Cameo via
[`harness-integration.md`](harness-integration.md); update via
[`updating.md`](updating.md); the HTTP surface is
[`api-reference.md`](api-reference.md).
