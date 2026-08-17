#!/usr/bin/env bash
# Phase 1, step 3: benchmark each built backend with llama-bench on a reference
# model. Writes per-backend JSON to artifacts/bench-<backend>.json.
source "$(dirname "${BASH_SOURCE[0]}")/_common.sh"

[ -f "$ARTIFACTS/build.env" ] || die "run build-llama.sh first (no build.env)"
# shellcheck disable=SC1091
source "$ARTIFACTS/build.env"

# Resolve the reference model.
if [ -z "$MODEL_PATH" ] && [ -n "$MODEL_URL" ]; then
  MODEL_PATH="$ARTIFACTS/model.gguf"
  if [ ! -f "$MODEL_PATH" ]; then
    log "Downloading reference model from CAMEO_MODEL_URL..."
    # Force HTTPS (no downgrade on redirect) and fail on HTTP errors.
    curl -fL --proto '=https' --tlsv1.2 "$MODEL_URL" -o "$MODEL_PATH"
  fi
fi
[ -n "$MODEL_PATH" ] && [ -f "$MODEL_PATH" ] \
  || die "set CAMEO_MODEL_PATH (local GGUF) or CAMEO_MODEL_URL (download) first"

# Verify integrity before feeding an untrusted blob to llama.cpp's parser.
if [ -n "${MODEL_SHA256:-}" ]; then
  echo "${MODEL_SHA256}  ${MODEL_PATH}" | sha256sum -c - \
    || die "model checksum mismatch — refusing to run $MODEL_PATH"
  log "Model checksum verified."
else
  warn "No CAMEO_MODEL_SHA256 set — running an unverified model. Set it to check integrity."
fi

bench_backend() { # name build_dir [extra env assignments...]
  local name="$1" dir="$2"; shift 2
  local bin="$dir/bin/llama-bench"
  [ -x "$bin" ] || bin="$dir/llama-bench"
  [ -x "$bin" ] || { warn "$name: llama-bench not found under $dir"; return 1; }
  log "Benchmarking $name backend on $(basename "$MODEL_PATH")..."
  if env "$@" "$bin" -m "$MODEL_PATH" -o json \
       > "$ARTIFACTS/bench-$name.json" 2> "$ARTIFACTS/bench-$name.log"; then
    log "  -> $ARTIFACTS/bench-$name.json"
  else
    warn "$name benchmark failed (see bench-$name.log)"
    return 1
  fi
}

bench_backend vulkan "$vulkan_build"

if [ "${rocm_built:-no}" = "yes" ]; then
  extra=()
  # Tier 2 cards need the HSA override at runtime; pass it through if set.
  [ -n "${CAMEO_HSA_OVERRIDE:-}" ] && extra+=("HSA_OVERRIDE_GFX_VERSION=$CAMEO_HSA_OVERRIDE")
  bench_backend rocm "$rocm_build" "${extra[@]}"
fi

log "Benchmarks complete. Next: ./record-combo.sh"
