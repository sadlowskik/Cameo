#!/usr/bin/env bash
# Phase 1, step 1: install build tools, Vulkan userspace, and (best-effort) ROCm
# on a fresh Arch instance. Records installed versions to artifacts/provision.env.
# shellcheck source-path=SCRIPTDIR
source "$(dirname "${BASH_SOURCE[0]}")/_common.sh"

need pacman  # Arch Linux only

# Packaged inference engine — this is what the ISO/container ship. Source-building
# llama.cpp is opt-in (CAMEO_BUILD_LLAMA=1); cmake/ninja stay available for that.
BUILD_PKGS="base-devel git cmake ninja curl"
VULKAN_PKGS="${CAMEO_VULKAN_PKGS:-vulkan-radeon vulkan-icd-loader vulkan-tools}"
LLAMA_PKGS="${CAMEO_LLAMA_PKGS:-llama-cpp ggml-cpu ggml-vulkan}"
ROCM_PKGS="${CAMEO_ROCM_PKGS:-rocm-hip-sdk rocminfo rocm-smi-lib ggml-hip}"

log "Installing Vulkan userspace + packaged llama.cpp: $VULKAN_PKGS $LLAMA_PKGS"
# shellcheck disable=SC2086
sudo pacman -Syu --needed --noconfirm $BUILD_PKGS $VULKAN_PKGS $LLAMA_PKGS \
  || die "pacman base/Vulkan/llama-cpp install failed"

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
