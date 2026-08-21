# Cameo quickstart

Download the ISO, flash a USB, plug it into the AMD machine, set an account,
open the console, chat. After that the box does not need the internet. Network
is for extra models, other Cameo nodes, and opening the console from outside
the house.

## 1. Download

From [GitHub Releases](https://github.com/sadlowskik/Cameo/releases):

- **Universal** (`cameo-*.iso`) — every AMD card. Take this if you are unsure.
  Vulkan always; ROCm only when the card can train. Larger download.
- **Lite** (`cameo-lite-*.iso`) — Vulkan only. For a known old card (RX 580,
  APU) or a small USB stick.

Verify against `SHA256SUMS` before flashing.

## 2. Flash a USB

**Windows (Rufus):** choose the ISO, then **DD Image mode** (not ISO mode).
ISO mode often produces a stick that will not boot.

**Windows / macOS:** [Etcher](https://etcher.balena.io/) also works.

**Linux / macOS:**

```bash
sudo dd if=cameo-*.iso of=/dev/sdX bs=4M status=progress oflag=sync
```

## 3. Firmware, then boot

1. **Disable Secure Boot** (or add the USB as a trusted key). Cameo’s bootloader
   is not Microsoft-signed yet; with Secure Boot on, many PCs ignore the stick
   and boot Windows as if nothing happened. Details: [secure-boot.md](secure-boot.md).
2. Boot from the USB. **Install Cameo to disk** is the default menu entry.
3. You will see the detected GPU and tier. Pick a disk, an admin username
   (lowercase, e.g. `admin`), a password, and a timezone. Type the disk’s name
   to confirm. Nothing is erased until that last step.
4. Reboot **with the USB still in**. Unplug it only when the screen goes dark
   (or at the firmware logo). Pulling it while the installer is still running
   kills the live OS.

Same flow from a live shell:

```bash
cameo-install-guided          # interactive
cameo-install                 # dry run — prints the plan, changes nothing
cameo-install --commit        # do it (still confirms the disk name)
```

## 4. Open the console and chat

After login the box prints the console URL and key (`cameo-hello` reprints them).

1. **Ethernet:** plug in. DHCP is automatic.
2. **Wi-Fi only:**

   ```bash
   iwctl
     station list
     station wlan0 scan          # use the name `station list` printed
     station wlan0 get-networks
     station wlan0 connect 'YourSSID'
   cameo-hello
   ```

3. From a phone or laptop on the same LAN, open
   `http://<the-box>:9090` or `http://cameo.local:9090`.
4. Enter the console key (the page has a field; it remembers it).
5. Press **Start qwen2.5-0.5b and chat**, then type. The starter is a
   **smoke test** (~0.5B). It proves the card works. Pull a larger GGUF when
   you have a network (`cameo pull --list`).

Serving never phones home. Extra weights: drop a `.gguf` in
`/var/lib/cameo/models` or `cameo pull`.

Live USB (without installing): the console key **changes every reboot**.
Install to disk to keep it.

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

The container prints a bearer key on start. Open `http://127.0.0.1:9090`.

Build the ISO yourself on an Arch host (`mkarchiso`, root):

```bash
sudo ./scripts/build-iso.sh                        # universal
sudo CAMEO_EDITION=lite ./scripts/build-iso.sh     # lite
```

Every CLI command takes `--dry-run` and `--json`. Point a harness at Cameo via
[`harness-integration.md`](harness-integration.md); update via
[`updating.md`](updating.md); the HTTP surface is
[`api-reference.md`](api-reference.md).
