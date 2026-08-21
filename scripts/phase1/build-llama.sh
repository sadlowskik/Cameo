#!/usr/bin/env bash
# Phase 1, step 2: build llama.cpp's Vulkan backend (always) and ROCm/HIP backend
# (when rocminfo is present). Records the exact build inputs to artifacts/build.env.
# shellcheck source-path=SCRIPTDIR
source "$(dirname "${BASH_SOURCE[0]}")/_common.sh"

[ "${CAMEO_BUILD_LLAMA:-}" = "1" ] \
  || die "packaged llama-cpp is the product path. Set CAMEO_BUILD_LLAMA=1 to compile from source (requires CAMEO_LLAMA_REF as a 40-char SHA)."

need git
need cmake
need ninja

[ -n "$LLAMA_REF" ] || die "set CAMEO_LLAMA_REF to a 40-character commit SHA."
if ! [[ "$LLAMA_REF" =~ ^[0-9a-fA-F]{40}$ ]]; then
  die "CAMEO_LLAMA_REF='$LLAMA_REF' is not a 40-char SHA — refusing to compile unverified upstream."
fi

if [ ! -d "$LLAMA_SRC/.git" ]; then
  log "Cloning llama.cpp into $LLAMA_SRC"
  git clone "$LLAMA_REPO" "$LLAMA_SRC"
fi
git -C "$LLAMA_SRC" fetch --all --tags --quiet || true
git -C "$LLAMA_SRC" checkout "$LLAMA_REF"
LLAMA_COMMIT="$(git -C "$LLAMA_SRC" rev-parse HEAD)"
log "llama.cpp at $LLAMA_REF ($LLAMA_COMMIT)"

# --- Vulkan build: the universal baseline, must always succeed ---
log "Building Vulkan backend..."
cmake -S "$LLAMA_SRC" -B "$LLAMA_SRC/build-vulkan" -G Ninja \
  -DCMAKE_BUILD_TYPE=Release -DGGML_VULKAN=ON >/dev/null
cmake --build "$LLAMA_SRC/build-vulkan" -j

# --- ROCm/HIP build: optional, Tier 1/2 only ---
ROCM_BUILT=no
if command -v rocminfo >/dev/null 2>&1; then
  if [ -z "$AMDGPU_TARGET" ]; then
    AMDGPU_TARGET="$(rocminfo 2>/dev/null | grep -oE 'gfx[0-9a-f]+' | head -1 || true)"
  fi
  if [ -n "$AMDGPU_TARGET" ]; then
    log "Building ROCm backend for target $AMDGPU_TARGET..."
    if cmake -S "$LLAMA_SRC" -B "$LLAMA_SRC/build-rocm" -G Ninja \
         -DCMAKE_BUILD_TYPE=Release -DGGML_HIP=ON \
         -DAMDGPU_TARGETS="$AMDGPU_TARGET" \
         -DCMAKE_C_COMPILER=hipcc -DCMAKE_CXX_COMPILER=hipcc >/dev/null \
       && cmake --build "$LLAMA_SRC/build-rocm" -j; then
      ROCM_BUILT=yes
    else
      warn "ROCm build failed for $AMDGPU_TARGET — recording Vulkan-only."
    fi
  else
    warn "Could not determine AMDGPU target from rocminfo — skipping ROCm build."
  fi
else
  warn "rocminfo absent — skipping ROCm build (Tier 3 path)."
fi

{
  echo "llama_repo=$LLAMA_REPO"
  echo "llama_ref=$LLAMA_REF"
  echo "llama_commit=$LLAMA_COMMIT"
  echo "amdgpu_target=$AMDGPU_TARGET"
  echo "vulkan_build=$LLAMA_SRC/build-vulkan"
  echo "rocm_built=$ROCM_BUILT"
  echo "rocm_build=$LLAMA_SRC/build-rocm"
} > "$ARTIFACTS/build.env"

log "Wrote $ARTIFACTS/build.env (rocm_built=$ROCM_BUILT)"
