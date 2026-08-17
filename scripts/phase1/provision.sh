#!/usr/bin/env bash
# Phase 1, step 1: install build tools, Vulkan userspace, and (best-effort) ROCm
# on a fresh Arch instance. Records installed versions to artifacts/provision.env.
source "$(dirname "${BASH_SOURCE[0]}")/_common.sh"

need pacman  # Arch Linux only

BUILD_PKGS="base-devel git cmake ninja curl"
VULKAN_PKGS="${CAMEO_VULKAN_PKGS:-vulkan-radeon vulkan-icd-loader vulkan-tools}"
ROCM_PKGS="${CAMEO_ROCM_PKGS:-rocm-hip-sdk rocminfo rocm-smi-lib}"

log "Installing build + Vulkan userspace: $BUILD_PKGS $VULKAN_PKGS"
# shellcheck disable=SC2086
sudo pacman -Syu --needed --noconfirm $BUILD_PKGS $VULKAN_PKGS \
  || die "pacman base/Vulkan install failed"

log "Installing ROCm (optional; failure => Tier 3 Vulkan-only): $ROCM_PKGS"
# shellcheck disable=SC2086
if ! sudo pacman -S --needed --noconfirm $ROCM_PKGS; then
  warn "ROCm packages did not install — treating this box as Tier 3 (Vulkan-only)."
fi

pkgver() { pacman -Q "$1" 2>/dev/null | awk '{print $2}'; }
{
  echo "kernel=$(uname -r)"
  echo "mesa=$(pkgver mesa)"
  echo "vulkan_radeon=$(pkgver vulkan-radeon)"
  echo "rocm=$(pkgver rocm-core)"
} > "$ARTIFACTS/provision.env"

log "Wrote $ARTIFACTS/provision.env"
log "Sanity: 'vulkaninfo --summary' and 'rocminfo' should now run (rocminfo only on Tier 1/2)."
