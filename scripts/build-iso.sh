#!/usr/bin/env bash
# Build the Cameo ISO.
#
# LINUX + Arch only — needs `mkarchiso` (the `archiso` package) and root. This
# CANNOT run on Windows/macOS; build it on your Cameo/Arch box (or an Arch
# live-USB). It builds from a *copy* of the profile, so the source tree stays clean.
#
#   sudo ./scripts/build-iso.sh                        # full edition (ROCm included)
#   sudo CAMEO_EDITION=lite ./scripts/build-iso.sh     # Vulkan-only, small — ideal for
#                                                      # old / Tier-3 cards (e.g. an iGPU laptop)
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PROFILE="$REPO/archiso"
# The work dir must live OUTSIDE the profile. mkarchiso builds from a staged
# copy at $WORK/profile, so a work dir of archiso/work makes that copy
# `cp -r archiso archiso/work/profile` — a directory into itself, which cp
# refuses. Keep this outside archiso/ or the build cannot start.
WORK="${CAMEO_WORK:-$REPO/build}"
OUT="${CAMEO_OUT:-$REPO/archiso/out}"
EDITION="${CAMEO_EDITION:-full}"
RELENG="${CAMEO_RELENG:-/usr/share/archiso/configs/releng}"

log() { printf '\033[1;38;5;209m[cameo-iso]\033[0m %s\n' "$*"; }
die() { printf '\033[1;31m[cameo-iso] ERROR:\033[0m %s\n' "$*" >&2; exit 1; }

command -v mkarchiso >/dev/null 2>&1 || die "mkarchiso not found — install the 'archiso' package (Arch only)."
[ "$(id -u)" -eq 0 ] || die "run as root — mkarchiso needs it: sudo $0"
[ -d "$RELENG" ] || die "baseline releng profile not found at $RELENG (install 'archiso'), or set CAMEO_RELENG."

BUILD="$WORK/profile"
log "Staging a build copy of the profile at $BUILD (edition: $EDITION)"
rm -rf "$BUILD"
mkdir -p "$WORK" "$OUT"
cp -r "$PROFILE" "$BUILD"
# Don't recursively copy our own work/out into the build copy.
rm -rf "$BUILD/work" "$BUILD/out"

# 1. Pull the baseline boot files the Cameo profile intentionally doesn't vendor.
[ -f "$BUILD/pacman.conf" ] || cp "$RELENG/pacman.conf" "$BUILD/"
for d in efiboot syslinux grub; do
  [ -e "$BUILD/$d" ] || cp -r "$RELENG/$d" "$BUILD/" 2>/dev/null || true
done

# 1a. Build airootfs as releng's live root with Cameo's files overlaid.
#
# Cherry-picking individual files from releng was the wrong model. Cameo's
# airootfs holds only branding and tuning, so everything a live system needs
# and Cameo doesn't happen to vendor -- the account database, mkinitcpio
# hooks, mirrorlist, network config -- was simply absent, and each omission
# surfaced only as a broken boot. Starting from releng's working root and
# letting Cameo win on conflict makes the live environment complete by
# construction rather than by enumeration.
#
# .wants is stripped from the base first: releng enables units for packages it
# ships and Cameo may not, which would leave systemd failing on units that do
# not exist. Cameo enables what it needs explicitly, below.
if [ -d "$RELENG/airootfs" ]; then
  MERGED="$WORK/airootfs.merged"
  rm -rf "$MERGED"
  cp -a "$RELENG/airootfs" "$MERGED"
  rm -rf "$MERGED"/etc/systemd/system/*.wants
  [ -d "$BUILD/airootfs" ] && cp -a "$BUILD/airootfs/." "$MERGED/"
  rm -rf "$BUILD/airootfs"
  mv "$MERGED" "$BUILD/airootfs"
  log "Merged airootfs: releng base, Cameo overlay"
fi

# Enable the units Cameo wants. systemd only honours [Install] at
# `systemctl enable` time, which never happens for a live image, so archiso
# profiles ship the .wants symlinks instead. Created here rather than committed
# because the tree is edited on Windows, where git symlinks are a trap.
WANTS="$BUILD/airootfs/etc/systemd/system/multi-user.target.wants"
mkdir -p "$WANTS"
ln -sf /etc/systemd/system/cameo-firstboot.service "$WANTS/cameo-firstboot.service"
# networkd and resolved ship inside systemd itself, so these can never dangle.
ln -sf /usr/lib/systemd/system/systemd-networkd.service "$WANTS/systemd-networkd.service"
ln -sf /usr/lib/systemd/system/systemd-resolved.service "$WANTS/systemd-resolved.service"
# Initialises /etc/pacman.d/gnupg. Without it every pacman install on the live
# image fails on signature trust, which also kills the only workaround for a
# missing package.
ln -sf /etc/systemd/system/pacman-init.service "$WANTS/pacman-init.service"
# Wireless association. Package is in packages.x86_64, so this cannot dangle.
ln -sf /usr/lib/systemd/system/iwd.service "$WANTS/iwd.service"
log "Enabled cameo-firstboot, networkd, resolved, pacman-init, iwd"

# Clock. A laptop with a flat CMOS battery boots with a wrong time, which
# breaks TLS on model downloads and pacman signature validity before anything
# else has a chance to fail.
SYSINIT="$BUILD/airootfs/etc/systemd/system/sysinit.target.wants"
mkdir -p "$SYSINIT"
ln -sf /usr/lib/systemd/system/systemd-timesyncd.service "$SYSINIT/systemd-timesyncd.service"
ln -sf /usr/lib/systemd/system/systemd-time-wait-sync.service "$SYSINIT/systemd-time-wait-sync.service"
log "Enabled time synchronisation"

# 1b. Brand the boot menus. Those borrowed configs label every entry "Arch
# Linux install medium", so the boot screen would announce the wrong distro.
# Retitle them in the staged copy — done with sed against whatever releng
# ships rather than by vendoring copies that would rot against upstream.
# Only human-readable titles change; kernel paths, archisobasedir and
# archisolabel are lowercase and untouched.
log "Branding the boot menus"
branded=0
while IFS= read -r -d '' f; do
  sed -i \
    -e 's/Arch Linux install medium/Cameo Linux/g' \
    -e 's/Arch Linux/Cameo Linux/g' \
    "$f"
  branded=$((branded + 1))
done < <(find "$BUILD/syslinux" "$BUILD/efiboot" "$BUILD/grub" \
           -type f \( -name '*.cfg' -o -name '*.conf' \) -print0 2>/dev/null)
log "Rebranded $branded boot config file(s)"

# 1c. Render the Cameo mark into the syslinux menu background. releng's
# archiso_head.cfg points MENU BACKGROUND at splash.png, so overwriting that
# file is all the wiring needed. A missing rasteriser is not fatal: the stock
# splash still boots, and a cosmetic asset must never fail a build.
SPLASH_SVG="$REPO/docs/brand/cameo-splash.svg"
if [ -f "$SPLASH_SVG" ] && [ -d "$BUILD/syslinux" ]; then
  if command -v rsvg-convert >/dev/null 2>&1; then
    rsvg-convert -w 640 -h 480 -o "$BUILD/syslinux/splash.png" "$SPLASH_SVG"
    log "Rendered the Cameo boot splash (rsvg-convert)"
  elif command -v magick >/dev/null 2>&1; then
    magick -background none "$SPLASH_SVG" -resize 640x480! "$BUILD/syslinux/splash.png"
    log "Rendered the Cameo boot splash (ImageMagick)"
  else
    log "No SVG rasteriser found - keeping the stock splash"
  fi
fi

# 2. Build the cameo CLI (native, for the ISO's arch) and stage it into the image.
log "Building the cameo CLI (release)..."
( cd "$REPO" && cargo build --release -p cameo-cli )
install -Dm755 "$REPO/target/release/cameo" "$BUILD/airootfs/usr/local/bin/cameo"

# 3. Lite edition: drop the heavy ROCm / PyTorch packages (Vulkan-only, much smaller).
if [ "$EDITION" = "lite" ]; then
  # ggml-hip matches neither ^rocm nor ^python-pytorch, so it needs its own
  # alternative or the "Vulkan-only" edition pulls the whole ROCm stack in as
  # a dependency of the compute backend.
  grep -viE '^(rocm|python-pytorch|ggml-hip)' "$PROFILE/packages.x86_64" > "$BUILD/packages.x86_64"
  log "Lite edition: ROCm/PyTorch excluded — Vulkan baseline only."
fi

# 4. Build the ISO.
log "Running mkarchiso (this takes a while and downloads packages)..."
mkarchiso -v -w "$WORK/tmp" -o "$OUT" "$BUILD"

log "Done. ISO(s) in: $OUT"
ls -lh "$OUT" || true
log "Write it to USB with:  sudo dd if=$OUT/cameo-*.iso of=/dev/sdX bs=4M status=progress oflag=sync"
