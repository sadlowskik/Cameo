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
WORK="${CAMEO_WORK:-$REPO/archiso/work}"
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

# 2. Build the cameo CLI (native, for the ISO's arch) and stage it into the image.
log "Building the cameo CLI (release)..."
( cd "$REPO" && cargo build --release -p cameo-cli )
install -Dm755 "$REPO/target/release/cameo" "$BUILD/airootfs/usr/local/bin/cameo"

# 3. Lite edition: drop the heavy ROCm / PyTorch packages (Vulkan-only, much smaller).
if [ "$EDITION" = "lite" ]; then
  grep -viE '^(rocm|python-pytorch)' "$PROFILE/packages.x86_64" > "$BUILD/packages.x86_64"
  log "Lite edition: ROCm/PyTorch excluded — Vulkan baseline only."
fi

# 4. Build the ISO.
log "Running mkarchiso (this takes a while and downloads packages)..."
mkarchiso -v -w "$WORK/tmp" -o "$OUT" "$BUILD"

log "Done. ISO(s) in: $OUT"
ls -lh "$OUT" || true
log "Write it to USB with:  sudo dd if=$OUT/cameo-*.iso of=/dev/sdX bs=4M status=progress oflag=sync"
