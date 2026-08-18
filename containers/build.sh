#!/usr/bin/env bash
# Build the Cameo container image(s). Vulkan is the universal default; pass
# `rocm` to also build the AMD-accelerated variant.
#   containers/build.sh          # cameo:vulkan
#   containers/build.sh rocm     # cameo:vulkan + cameo:rocm
#
# Honours $CONTAINER_ENGINE (podman or docker); auto-detects otherwise.
set -euo pipefail
cd "$(dirname "$0")/.."

engine="${CONTAINER_ENGINE:-}"
if [ -z "$engine" ]; then
  if command -v podman >/dev/null 2>&1; then engine=podman; else engine=docker; fi
fi

"$engine" build -f containers/Containerfile -t cameo:vulkan .
echo "built cameo:vulkan"

if [ "${1:-}" = "rocm" ]; then
  "$engine" build -f containers/Containerfile --build-arg EDITION=rocm -t cameo:rocm .
  echo "built cameo:rocm"
fi
