#!/bin/sh
# Cameo container entrypoint.
#
# Default (no args, or only flags): run cameod with a reachable but authenticated
# console, mirroring the ISO's cameo-console-init. Any other argv is exec'd as-is,
# so the image doubles as the CLI:
#   podman run --rm cameo:vulkan cameo pull tinyllama
set -eu

# An explicit command (not a flag) runs verbatim instead of the daemon.
if [ "$#" -gt 0 ]; then
    case "$1" in
    -*) : ;;          # a flag: fall through to the daemon, passing it along
    *) exec "$@" ;;   # a command: cameo, sh, ...
    esac
fi

# Make the console reachable from the host, but never unauthenticated: generate a
# key when none was supplied and the bind is not loopback.
if [ -z "${CAMEO_CONSOLE_KEY:-}" ] && [ "${CAMEO_CONSOLE_HOST:-0.0.0.0}" != "127.0.0.1" ]; then
    key=$(head -c 48 /dev/urandom | base64 | tr -dc 'A-Za-z0-9' | cut -c1-32)
    if [ -n "$key" ]; then
        CAMEO_CONSOLE_HOST=${CAMEO_CONSOLE_HOST:-0.0.0.0}
        CAMEO_CONSOLE_KEY=$key
        export CAMEO_CONSOLE_HOST CAMEO_CONSOLE_KEY
        echo "cameo: console on ${CAMEO_CONSOLE_HOST}:9090 with generated bearer key: $key"
    else
        export CAMEO_CONSOLE_HOST=127.0.0.1
        echo "cameo: no entropy for a key; console bound loopback-only" >&2
    fi
fi

exec /usr/local/bin/cameod "$@"
