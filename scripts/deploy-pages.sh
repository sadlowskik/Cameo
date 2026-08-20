#!/usr/bin/env bash
# Deploy site/ to Cloudflare Pages - the resilient path.
#
# No origin, no tunnel, no home LAN in the request path: the front page is served
# from Cloudflare's edge and cannot be taken down by the homelab, the router, or a
# DHCP hiccup. This is the recommended host for cameoconstruct.xyz's public page;
# keep the CT 103 container for the backend apps (tags, admin, umami) that need it.
#
# Usage (from the Cameo repo root):
#     ./scripts/deploy-pages.sh
#
# First run opens a browser to authorize wrangler and creates the project. After
# that it just publishes a new version to the same project.
#
#     CAMEO_PAGES_PROJECT   Pages project name   (default: cameo)
set -euo pipefail

PROJECT="${CAMEO_PAGES_PROJECT:-cameoconstruct}"
SRC="$(cd "$(dirname "${BASH_SOURCE[0]}")/../site" && pwd)"

[ -f "$SRC/index.html" ] || { printf 'no site/index.html at %s\n' "$SRC" >&2; exit 1; }

printf '\033[1;38;5;209m[pages]\033[0m Deploying %s -> Cloudflare Pages project "%s"\n' "$SRC" "$PROJECT"
# Prefer a globally-installed wrangler; fall back to npx (which fetches it on demand).
if command -v wrangler >/dev/null 2>&1; then WR=(wrangler); else WR=(npx --yes wrangler); fi
# --branch main makes this a PRODUCTION deployment regardless of the repo's current
# git branch; without it, deploys from feature branches only create previews.
"${WR[@]}" pages deploy "$SRC" --project-name "$PROJECT" --branch main --commit-dirty=true

printf '\033[1;38;5;209m[pages]\033[0m Done. Add the custom domain in the Pages project:\n'
printf '  Cloudflare dashboard -> Pages -> %s -> Custom domains -> add cameoconstruct.xyz\n' "$PROJECT"
printf '  Then remove the apex tunnel/DNS route that points the root domain at CT 103.\n'
