#!/usr/bin/env bash
# Shared configuration and helpers for Cameo Phase 1 validation scripts.
#
# Every value here is a PLACEHOLDER to confirm on real hardware. Override any of
# them via the CAMEO_* environment variables (the plan's "auto but overridable"
# rule applies to this runbook too).
# This file is a sourced library: the variables below are consumed by the
# scripts that source it, which standalone shellcheck cannot see.
# shellcheck disable=SC2034
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ARTIFACTS="${CAMEO_ARTIFACTS:-$HERE/artifacts}"
mkdir -p "$ARTIFACTS"

# llama.cpp source + pin. Pin LLAMA_REF to a specific tag/commit once validated.
LLAMA_REPO="${CAMEO_LLAMA_REPO:-https://github.com/ggml-org/llama.cpp}"
LLAMA_REF="${CAMEO_LLAMA_REF:-master}"
LLAMA_SRC="${CAMEO_LLAMA_SRC:-$HERE/llama.cpp}"

# AMD build target for the ROCm/HIP backend (e.g. gfx1030). Auto-detected from
# rocminfo when left empty.
AMDGPU_TARGET="${CAMEO_AMDGPU_TARGET:-}"

# Reference model (GGUF) for benchmarking: a local path or a download URL.
MODEL_PATH="${CAMEO_MODEL_PATH:-}"
MODEL_URL="${CAMEO_MODEL_URL:-}"
# Optional SHA-256 of the downloaded model. Set it to verify integrity — without
# it, a redirected/tampered download is run unverified.
MODEL_SHA256="${CAMEO_MODEL_SHA256:-}"

log()  { printf '\033[1;36m[phase1]\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m[phase1] WARN:\033[0m %s\n' "$*" >&2; }
die()  { printf '\033[1;31m[phase1] ERROR:\033[0m %s\n' "$*" >&2; exit 1; }

need() { command -v "$1" >/dev/null 2>&1 || die "required command not found: $1"; }
