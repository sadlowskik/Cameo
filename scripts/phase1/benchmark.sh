#!/usr/bin/env bash
# Phase 1, step 3: benchmark llama.cpp with llama-bench on a reference model.
# Default: the packaged `llama-bench` on PATH (what the ISO/container ship).
# Optional: CAMEO_BUILD_LLAMA=1 + artifacts/build.env from build-llama.sh, to
# bench the source-built trees instead.
# shellcheck source-path=SCRIPTDIR
source "$(dirname "${BASH_SOURCE[0]}")/_common.sh"

USE_TREES=0
if [ "${CAMEO_BUILD_LLAMA:-}" = "1" ] && [ -f "$ARTIFACTS/build.env" ]; then
  # shellcheck disable=SC1091
  source "$ARTIFACTS/build.env"
  vulkan_build="${vulkan_build:?build.env did not define vulkan_build — re-run build-llama.sh}"
  rocm_build="${rocm_build:-}"
  USE_TREES=1
  log "Using source-built llama-bench from build.env"
fi

# Resolve the reference model.
if [ -z "$MODEL_PATH" ] && [ -n "$MODEL_URL" ]; then
  MODEL_PATH="$ARTIFACTS/model.gguf"
  if [ ! -f "$MODEL_PATH" ]; then
    log "Downloading reference model from CAMEO_MODEL_URL..."
    # Force HTTPS (no downgrade on redirect) and fail on HTTP errors.
    curl -fL --proto '=https' --tlsv1.2 "$MODEL_URL" -o "$MODEL_PATH"
  fi
fi
if [ -z "$MODEL_PATH" ] || [ ! -f "$MODEL_PATH" ]; then
  die "set CAMEO_MODEL_PATH (local GGUF) or CAMEO_MODEL_URL (download) first"
fi

# Verify integrity before feeding an untrusted blob to llama.cpp's parser.
if [ -n "${MODEL_SHA256:-}" ]; then
  echo "${MODEL_SHA256}  ${MODEL_PATH}" | sha256sum -c - \
    || die "model checksum mismatch — refusing to run $MODEL_PATH"
  log "Model checksum verified."
else
  warn "No CAMEO_MODEL_SHA256 set — running an unverified model. Set it to check integrity."
fi

bench_one() { # name binary [extra env assignments...]
  local name="$1" bin="$2"; shift 2
  if [ ! -x "$bin" ]; then
    local found
    found="$(command -v "$bin" 2>/dev/null || true)"
    bin="$found"
  fi
  [ -n "$bin" ] && [ -x "$bin" ] || { warn "$name: llama-bench not found"; return 1; }
  log "Benchmarking $name backend on $(basename "$MODEL_PATH") ($bin)..."
  if env "$@" "$bin" -m "$MODEL_PATH" -o json \
       > "$ARTIFACTS/bench-$name.json" 2> "$ARTIFACTS/bench-$name.log"; then
    log "  -> $ARTIFACTS/bench-$name.json"
  else
    warn "$name benchmark failed (see bench-$name.log)"
    return 1
  fi
}

if [ "$USE_TREES" = "1" ]; then
  vulkan_bin="$vulkan_build/bin/llama-bench"
  [ -x "$vulkan_bin" ] || vulkan_bin="$vulkan_build/llama-bench"
  bench_one vulkan "$vulkan_bin"
  if [ "${rocm_built:-no}" = "yes" ]; then
    extra=()
    [ -n "${CAMEO_HSA_OVERRIDE:-}" ] && extra+=("HSA_OVERRIDE_GFX_VERSION=$CAMEO_HSA_OVERRIDE")
    rocm_bin="$rocm_build/bin/llama-bench"
    [ -x "$rocm_bin" ] || rocm_bin="$rocm_build/llama-bench"
    bench_one rocm "$rocm_bin" "${extra[@]}"
  fi
else
  need llama-bench
  bench_one vulkan llama-bench
  if command -v rocminfo >/dev/null 2>&1; then
    extra=()
    [ -n "${CAMEO_HSA_OVERRIDE:-}" ] && extra+=("HSA_OVERRIDE_GFX_VERSION=$CAMEO_HSA_OVERRIDE")
    bench_one rocm llama-bench "${extra[@]}"
  else
    log "rocminfo absent — skipping ROCm bench (Vulkan-only)."
  fi
fi

log "Benchmarks complete. Next: ./record-combo.sh"
