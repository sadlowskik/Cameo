#!/usr/bin/env bash
# Fetch the starter GGUF Cameo ships on the ISO and in the container, so a
# freshly installed box can chat with no internet.
#
# The file is *not* in git (~380 MiB). This script downloads it once, checks
# the HuggingFace LFS SHA-256, and installs it as qwen2.5-0.5b.gguf — the
# name `cameo serve qwen2.5-0.5b` already resolves.
#
#   ./scripts/fetch-starter-model.sh --dest /usr/share/cameo/models
#   CAMEO_SKIP_STARTER=1 ./scripts/fetch-starter-model.sh   # no-op (CI)
#
# Qwen2.5-0.5B-Instruct is Apache-2.0 (Qwen). The GGUF is bartowski's Q4_K_M.
set -euo pipefail

NAME="qwen2.5-0.5b.gguf"
# bartowski/Qwen2.5-0.5B-Instruct-GGUF  Q4_K_M  (~379 MiB)
URL="https://huggingface.co/bartowski/Qwen2.5-0.5B-Instruct-GGUF/resolve/main/Qwen2.5-0.5B-Instruct-Q4_K_M.gguf"
# HuggingFace LFS oid == SHA-256 of the blob.
SHA256="6eb923e7d26e9cea28811e1a8e852009b21242fb157b26149d3b188f3a8c8653"

CACHE=""
DEST=""
while [ $# -gt 0 ]; do
  case "$1" in
    --cache) CACHE="${2:-}"; shift ;;
    --dest)  DEST="${2:-}"; shift ;;
    -h|--help)
      sed -n '2,16p' "$0" | sed 's/^# \{0,1\}//'
      exit 0 ;;
    *) echo "fetch-starter-model: unknown argument: $1" >&2; exit 1 ;;
  esac
  shift
done

if [ "${CAMEO_SKIP_STARTER:-}" = "1" ]; then
  echo "fetch-starter-model: CAMEO_SKIP_STARTER=1 — not fetching."
  exit 0
fi

if [ -z "$CACHE" ]; then
  HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
  CACHE="${CAMEO_STARTER_CACHE:-$HERE/build/models}"
fi
mkdir -p "$CACHE"
cached="$CACHE/$NAME"

have() {
  [ -f "$1" ] || return 1
  local got
  got="$(sha256sum "$1" | awk '{print $1}')"
  [ "$got" = "$SHA256" ]
}

if have "$cached"; then
  echo "fetch-starter-model: cache hit $cached"
else
  command -v curl >/dev/null 2>&1 || { echo "fetch-starter-model: need curl" >&2; exit 1; }
  echo "fetch-starter-model: downloading $NAME (~380 MiB)…"
  curl -fL --retry 5 --retry-delay 2 --retry-all-errors \
    -o "$cached.part" "$URL"
  got="$(sha256sum "$cached.part" | awk '{print $1}')"
  if [ "$got" != "$SHA256" ]; then
    rm -f "$cached.part"
    echo "fetch-starter-model: SHA-256 mismatch (got $got, want $SHA256)" >&2
    exit 1
  fi
  mv "$cached.part" "$cached"
  echo "fetch-starter-model: verified $SHA256"
fi

if [ -n "$DEST" ]; then
  mkdir -p "$DEST"
  # Same inode if dest is already the cache; otherwise copy into the image.
  if [ "$(realpath "$cached")" != "$(realpath "$DEST/$NAME" 2>/dev/null || true)" ]; then
    cp -a "$cached" "$DEST/$NAME"
  fi
  echo "fetch-starter-model: installed $DEST/$NAME"
fi
