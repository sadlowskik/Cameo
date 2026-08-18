#!/bin/sh
# Cameo container entrypoint.
#
# Default (no args, or only flags): run `cameod`, first making the console
# reachable from the host *without* leaving it unauthenticated — the same
# fail-closed stance as the ISO's cameo-console-init. A container with no
# CAMEO_CONSOLE_KEY gets a fresh generated one, printed once to the log.
#
# Any other argument vector is exec'd verbatim, so the image doubles as the CLI:
#   podman run --rm cameo:vulkan cameo pull tinyllama
set -eu

run_daemon() {
  if [ -z "${CAMEO_CONSOLE_KEY:-}" ] && [ "${CAMEO_CONSOLE_HOST:-0.0.0.0}" != "127.0.0.1" ]; then
    key="$(head -c 48 /dev/urandom | base64 | tr -dc 'A-Za-z0-9' | head -c 32)"
    if [ -n "$key" ]; then
      CAMEO_CONSOLE_HOST="${CAMEO_CONSOLE_HOST:-0.0.0.0}"
      CAMEO_CONSOLE_KEY="$key"
      export CAMEO_CONSOLE_HOST CAMEO_CONSOLE_KEY
      echo "cameo: console on ${CAMEO_CONSOLE_HOST}:9090 — generated bearer key: $key"
      echo "cameo: send it as 'Authorization: Bearer <key>', or set CAMEO_CONSOLE_KEY yourself."
    else
      export CAMEO_CONSOLE_HOST=127.0.0.1
      echo "cameo: no entropy for a key; console bound loopback-only." >&2
    fi
  fi
  exec /usr/local/bin/cameod "$@"
}

# No args, or the first token is a flag → run the daemon. Otherwise exec the
# given command (so `... cameo pull x` and `... sh -c ...` work).
if [ "$#" -eq 0 ] || [ "${1#-}" != "$1" ]; then
  run_daemon "$@"
fi
exec "$@"
