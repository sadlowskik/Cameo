# Cameo quickstart

Flash the image. Boot the box. Chat. After that it does not need the internet.
Network is only for extra models and attaching a node to a fleet.

## 1. Download the image

From [GitHub Releases](https://github.com/sadlowskik/Cameo/releases), take a file
named `cameo-*.iso` (or `.iso.part*` if split). Ignore “Source code” zip/tar —
those are not flashable. If the newest tag has no ISO, use the last tag that
lists one. Today the last flashable lite image is
[v0.1.0-beta.2](https://github.com/sadlowskik/Cameo/releases/tag/v0.1.0-beta.2).

- **Universal** (`cameo-*.iso`) — every AMD card. Unsure? This one.
- **Lite** (`cameo-lite-*.iso`) — Vulkan only, known old card (RX 580, APU).

Verify against `SHA256SUMS` before flashing.

## 2. Flash a USB

**Windows (Rufus):** choose the ISO, then **DD Image mode** (not ISO mode).
ISO mode often produces a stick that will not boot.

**Windows / macOS:** [Etcher](https://etcher.balena.io/) also works.

**Linux / macOS:**

```bash
sudo dd if=cameo-*.iso of=/dev/sdX bs=4M status=progress oflag=sync
```

## 3. Boot and install

1. **Disable Secure Boot** (or the stick is ignored and Windows boots). Details:
   [secure-boot.md](secure-boot.md).
2. Boot **Install Cameo to disk** (the default). Pick a disk, an admin username
   (lowercase, e.g. `admin`), a password, and a timezone. Type the disk’s name
   to confirm.
3. Reboot **with the USB still in**. Unplug it only when the screen goes dark.

You do **not** need Wi-Fi for this.

## 4. Open the console and chat

After login the box prints the console URL and key (`cameo-hello` reprints them).
Plug Ethernet if the box has a port so another device on the LAN can open:

`http://cameo.local:9090` or `http://<the-box>:9090`

Enter the key. Press **Start qwen2.5-0.5b and chat**. That starter is a smoke
test (~0.5B). Extra GGUFs: drop them in `/var/lib/cameo/models`.

Live USB (without installing): the console key **changes every reboot**.
Install to disk to keep it.

## Fleet: that’s when a node gets a network

A single box stays air-gapped after you flash. To attach this machine as a
**node** in a fleet, give it a network:

- Ethernet: plug in (DHCP is automatic).
- Wi-Fi, on that node only:

```bash
iwctl
  station list
  station wlan0 scan
  station wlan0 get-networks
  station wlan0 connect 'YourSSID'
```

Then point it at the hub (`CAMEO_HUB_URL` / `cameo fleet`). Extra models from
the internet are `cameo pull` — also later, not required to chat.

## Developers

Container path: host owns the GPU driver. Not the appliance.

```bash
podman build -f containers/Containerfile -t cameo:vulkan .
podman run --rm -p 9090:9090 -v cameo-models:/var/lib/cameo/models \
  --device=/dev/kfd --device=/dev/dri --group-add video --group-add render \
  cameo:vulkan
```

Build the ISO yourself on an Arch host (`mkarchiso`, root):

```bash
sudo ./scripts/build-iso.sh
sudo CAMEO_EDITION=lite ./scripts/build-iso.sh
```
