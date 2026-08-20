#!/usr/bin/env bash
# Push the Cameo landing (site/) to the cameoconstruct.xyz container on CT 103.
#
# The portfolio stack's `homepage` service (container name: portfolio) is plain
# nginx:alpine serving its bind-mounted docroot:
#     ./site  ->  /usr/share/nginx/html:ro
# So anything that lands in <REMOTE_DIR>/site on the box is served LIVE - no image
# rebuild, no compose restart. This script copies site/ there over SSH.
#
# Usage (from the Cameo repo root, in Git Bash):
#     ./scripts/deploy-cameoconstruct.sh --dry-run     # list what would be pushed
#     ./scripts/deploy-cameoconstruct.sh               # push it
#
# Configure the target with env vars (defaults target CT 103 as documented in the
# Homeserver ARCHITECTURE.md - the public zone at 192.168.4.105):
#     CAMEO_SITE_HOST     ssh target            (default: root@192.168.4.105)
#     CAMEO_SITE_REMOTE   dir holding the       (default: /root/portfolio)
#                         portfolio compose + site/   <-- VERIFY THIS PATH ON THE BOX
set -euo pipefail

HOST="${CAMEO_SITE_HOST:-root@192.168.4.105}"
REMOTE_DIR="${CAMEO_SITE_REMOTE:-/root/portfolio}"
SRC="$(cd "$(dirname "${BASH_SOURCE[0]}")/../site" && pwd)"

log(){ printf '\033[1;38;5;209m[deploy]\033[0m %s\n' "$*"; }
die(){ printf '\033[1;31m[deploy] ERROR:\033[0m %s\n' "$*" >&2; exit 1; }

[ -f "$SRC/index.html" ] || die "no site/index.html found at $SRC"

if [ "${1:-}" = "--dry-run" ]; then
  log "Would push these files to ${HOST}:${REMOTE_DIR}/site/"
  (cd "$SRC" && find . -type f | sed 's|^\./|  |')
  log "(dry run - nothing sent). Remote docroot: ${REMOTE_DIR}/site"
  exit 0
fi

log "Pushing $SRC  ->  ${HOST}:${REMOTE_DIR}/site/"
# tar-over-ssh: portable (no rsync needed), and additive - it overwrites the files
# it carries (index.html, legal.html, assets/) and leaves anything else in place.
tar -C "$SRC" -czf - . \
  | ssh "$HOST" "mkdir -p '${REMOTE_DIR}/site' && tar -C '${REMOTE_DIR}/site' -xzf - && echo unpacked"

# Static content is served straight from the bind mount, so a reload is optional;
# do it anyway so any future config change also takes effect. Never fatal.
ssh "$HOST" "docker exec portfolio nginx -s reload >/dev/null 2>&1 || true"

log "Done. Live at https://cameoconstruct.xyz/  (hard-refresh to skip cache)."
