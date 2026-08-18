# archiso — the Cameo ISO

The Arch profile that builds the branded, bootable **Cameo** ISO. **Built on
Linux only** (`mkarchiso` needs Arch + root) — you cannot build it on Windows.
Build it on your Cameo/Arch box or an Arch live-USB.

## One-command build

```bash
# full edition (ROCm + Vulkan) — for Tier 1/2 cards
sudo ./scripts/build-iso.sh

# lite edition (Vulkan only, much smaller) — ideal for old / Tier-3 cards,
# e.g. an iGPU laptop that has no usable ROCm path
sudo CAMEO_EDITION=lite ./scripts/build-iso.sh
```

`scripts/build-iso.sh` builds from a *copy* of this profile (source stays clean):
it pulls the baseline boot files from the stock `releng` profile, compiles the
`cameo` CLI **and the `cameod` console daemon** (`cargo build --release`) and
stages both into the image, optionally strips the heavy ROCm/PyTorch packages for
the lite edition, then runs `mkarchiso`. The ISO lands in `archiso/out/`.

Write it to a USB stick and boot:

```bash
sudo dd if=archiso/out/cameo-*.iso of=/dev/sdX bs=4M status=progress oflag=sync
```

## What's branded
- **Identity** — `os-release` (the box *is* "Cameo Linux", coral accent), `hostname`.
- **Login** — `/etc/issue` shows the Cameo wordmark at the prompt (console-safe ASCII);
  `/etc/motd` greets with the tagline and the key commands.
- **First boot** — `cameo-firstboot.service` runs `cameo gpu-status`, so the very
  first thing you see is your GPU's support tier, in colour — followed by the
  web-console URL. It also reports whether the model cache is on RAM or a disk.
- **Install to disk** — the boot menu carries an **"Install Cameo to disk"** entry
  (every bootloader: systemd-boot, syslinux, GRUB). It boots the same kernel with
  `cameo.install` on the command line; `cameo-installer.service` sees that marker
  and launches the guided installer (`cameo-install-guided` → `cameo-install`) on
  tty1. A normal live boot lacks the marker and is unaffected. You can also just
  run `cameo-install` from any live shell.
- **Persistent model cache** — on the live medium the cache is a RAM overlay, so
  a pulled model is lost on reboot (and a large one can exhaust memory).
  `cameo-persist-cache /dev/sdXN --format` prepares a disk (labelled `CAMEO_DATA`)
  and mounts it at `/var/lib/cameo/models`, where both the daemon and the CLI look;
  `cameo-storage-init.service` remounts that disk on every later boot, so the
  setup is a one-time step. Never formats without `--format`, and refuses the
  live-USB and root disks.
- **The CLI** — `cameo` is on `PATH` in the image, wordmark and all.
- **The console** — `cameod.service` starts the browser control plane on boot.
  `cameo-console-init` generates a random key and binds all interfaces each boot,
  so out of the box it's a **key-protected home console** you open from your own
  machine's browser (the URL + key are printed on first boot). Override in
  `/etc/cameo/cameod.env` — e.g. `CAMEO_CONSOLE_HOST=127.0.0.1` to force
  loopback-only, or pin a fixed key. A non-loopback bind without a key is refused
  on purpose, so the GPU is never published unauthenticated.

## What's in the image
`packages.x86_64`: kernel + amdgpu + Vulkan userspace (every tier), ROCm (Tier 1/2,
dropped in lite), llama.cpp/PyTorch build deps. The `cameo` CLI and the `cameod`
console daemon are compiled from this repo and staged into the airootfs by the
build script (not listed as packages).

## Boot-layer tuning (the distro's unfair advantage)
- `airootfs/etc/modprobe.d/cameo-amdgpu.conf` — amdgpu module options (GTT aperture
  for offload; power features **off by default**, enable per validated card).
- `airootfs/etc/sysctl.d/30-cameo.conf` — documents the hugepage / swappiness knobs
  for pinned offload buffers. Sets nothing: both values it used to ship were no-ops
  on the live image, and a no-op reads as "already tuned".

## Still TODO (Phase 5)
- A boot-menu splash/theme (currently inherits the releng bootloader chrome).
- Pin ROCm/kernel/mesa **per tier** from `scripts/phase1`'s `known-good-combo.json`.
- The TUI installer step (detect → show tier → confirm backend → install to disk).
- **Resizable BAR** is a firmware/BIOS setting — document it as a recommended toggle.

⚠️ Every value in the tuning files is a **starting point** to confirm on real
hardware; the whole profile builds today but is only *validated* once you've booted
it and run `scripts/phase1`.
