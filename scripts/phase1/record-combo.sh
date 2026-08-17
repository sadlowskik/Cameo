#!/usr/bin/env bash
# Phase 1, step 4: assemble the first "known-good combination" record from the
# provision/build/benchmark artifacts. This JSON is the deliverable of Phase 1 —
# it seeds the compatibility matrix and core/gpu-detect/data/overrides.toml.
source "$(dirname "${BASH_SOURCE[0]}")/_common.sh"

# shellcheck disable=SC1091
[ -f "$ARTIFACTS/provision.env" ] && source "$ARTIFACTS/provision.env" || true
# shellcheck disable=SC1091
[ -f "$ARTIFACTS/build.env" ] && source "$ARTIFACTS/build.env" || true

# Best-effort: pull the max avg_ts from a llama-bench JSON, else null.
extract_ts() {
  local f="$1"
  [ -f "$f" ] || { echo "null"; return; }
  local v
  v="$(grep -oE '"avg_ts":[[:space:]]*[0-9.]+' "$f" \
       | grep -oE '[0-9.]+' | sort -nr | head -1)"
  echo "${v:-null}"
}

VULKAN_TS="$(extract_ts "$ARTIFACTS/bench-vulkan.json")"
ROCM_TS="$(extract_ts "$ARTIFACTS/bench-rocm.json")"

if [ "${rocm_built:-no}" = "yes" ]; then ROCM_BUILT_JSON=true; else ROCM_BUILT_JSON=false; fi

GPU_MODEL="$(lspci -nn 2>/dev/null \
  | grep -iE 'vga compatible|display controller|3d controller' \
  | grep -i '\[1002:' | head -1 \
  | sed -E 's/.*\[AMD\/ATI\][[:space:]]*(.*)[[:space:]]*\[1002:.*/\1/' || true)"

OUT="$ARTIFACTS/known-good-combo.json"
cat > "$OUT" <<JSON
{
  "recorded_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "gpu_model": "${GPU_MODEL:-unknown}",
  "amdgpu_target": "${amdgpu_target:-}",
  "hsa_override": "${CAMEO_HSA_OVERRIDE:-}",
  "kernel": "${kernel:-$(uname -r)}",
  "rocm": "${rocm:-}",
  "mesa": "${mesa:-}",
  "vulkan_radeon": "${vulkan_radeon:-}",
  "llama_cpp": {
    "repo": "${llama_repo:-}",
    "ref": "${llama_ref:-}",
    "commit": "${llama_commit:-}"
  },
  "backends": {
    "vulkan": { "built": true, "tok_s": ${VULKAN_TS:-null} },
    "rocm": { "built": ${ROCM_BUILT_JSON}, "tok_s": ${ROCM_TS:-null} }
  }
}
JSON

log "Wrote $OUT"
echo "----------------------------------------"
cat "$OUT"
echo "----------------------------------------"
log "Copy this file back to the repo and use it to seed the compatibility matrix"
log "and core/gpu-detect/data/overrides.toml. Phase 1 is complete once this exists."
