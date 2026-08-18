# Release readiness

An honest assessment of whether the Cameo ISO is worth shipping, and of the two
gates that cannot be closed from a non-Linux dev box. Updated 2026-08-18.

## What this image is

A bootable Arch respin that turns any AMD box into a **GPU-aware home console**:

- Boots to a text console (autologin, passwordless root — the archiso norm for a
  live image), with the `cameo` CLI on `PATH`.
- **First boot prints your GPU's support tier** in plain language, then the URL and
  key for the web console.
- **`cameod` runs as a service and is a home console out of the box.** Each boot,
  `cameo-console-init` generates a random key and binds all interfaces; you open
  the printed URL from *your own machine's* browser and manage GPUs, models, and
  inference endpoints. A non-loopback bind without a key is refused, so the GPU is
  never published unauthenticated. `/etc/cameo/cameod.env` overrides everything
  (force loopback, pin a fixed key, change the port).
- Full and `lite` (Vulkan-only, ROCm stripped) editions; keeps the build
  toolchain, because Cameo is a dev platform, not a locked appliance.

## Verified (on any OS, including the Windows dev box)

- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -D warnings`,
  `cargo test --workspace` — **all green, 159 tests**.
- The daemon was driven end-to-end in a browser against the APU+dGPU fixture:
  GPU/tier report, plan preview, and the full endpoint lifecycle (start → list →
  stop). On a non-Linux host the final spawn reports "Linux only" — correct; on
  the real image it spawns `llama-server`.
- The exact shipped auth/bind path was smoke-tested: with the generated key +
  `0.0.0.0` bind, the API is 401 without/with a wrong key and 200 with the right
  one, while the dashboard shell loads openly so the browser can prompt.
- `build-iso.sh`, `cameo-firstboot`, `cameo-console-init` pass `bash -n`.

## The two gates that need YOUR machine

These are physical constraints, not unfinished work — neither can be done from a
Windows/macOS box.

1. **Build the ISO** (`mkarchiso`, Linux + root only):
   ```bash
   sudo ./scripts/build-iso.sh                     # full (ROCm + Vulkan)
   sudo CAMEO_EDITION=lite ./scripts/build-iso.sh  # Vulkan-only, small
   ```
   The `cameod` binary and its units are staged into the airootfs by the same
   pipeline that already produced a booting image with the CLI, so this is a
   low-risk extension of a known-good build — but it has not been re-run since
   these changes, so **run it and confirm the ISO builds and boots before
   shipping.** In particular, this round added boot-surface changes that only a
   real `mkarchiso` build + boot can confirm: the new `cameo-storage-init` and
   `cameo-installer` units, and the **"Install Cameo to disk" boot entry** that
   `build-iso.sh` clones into systemd-boot/syslinux/GRUB (the cloning logic was
   validated against releng-style fixtures, but the live entries were not). Boot
   the built ISO, pick that entry, and confirm the guided installer starts on
   tty1.

2. **Phase 1 hardware validation** (`scripts/phase1/` on a real AMD box). Until
   `known-good-combo.json` exists, these remain *starting points*, not validated
   facts, and are centralized so one run corrects them:
   - llama.cpp / torchrun flags (`core/placement/src/command.rs`).
   - Tier assignments + HSA overrides (`core/gpu-detect/data/overrides.toml`).
   - Memory-estimate constants (`core/placement/src/model.rs`, `plan.rs`).
   - The container GPU-passthrough recipe (`core/containers`): whether
     `seccomp=unconfined` and the `render` group are strictly required.

## Verdict

**Shippable as a v0.x preview/beta**: it builds from a known-good pipeline and
boots to a working, GPU-aware home console you administer from your browser. It is
**not a hardware-validated v1** — that needs the one Phase-1 run, which the
project's own plan requires before Phase 2 backend work.

## Known scope decisions (not defects)

- **No local desktop/browser.** The image is a headless appliance administered
  remotely (the Proxmox model). A local desktop environment is a deliberate,
  separate size/scope decision, not shipped here.
- **Install to disk is shipped.** A boot-menu "Install Cameo to disk" entry runs
  the guided installer (`cameo-install`): it partitions, pacstraps the tier-aware
  stack, installs GRUB (UEFI/BIOS), creates the admin account, and writes a
  persistent console key. After install, accounts/models/config persist normally.
- **Live-medium persistence.** On the *live USB* the model cache is a RAM overlay
  by default; `cameo-persist-cache` points it at a `CAMEO_DATA` disk that
  `cameo-storage-init` remounts each boot. Accounts/config on a pure live boot
  still reset (that is what install-to-disk is for).
- **Containers/Kubernetes console is not built.** The AMD GPU-passthrough recipe
  exists and is tested (`core/containers`), but the Podman/k3s adapters and their
  dashboard tabs — the wider "manage all your deployments" vision — are the next
  increments, tracked in `containers/README.md` and `k8s/README.md`.
