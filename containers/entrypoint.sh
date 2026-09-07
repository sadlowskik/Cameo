#!/bin/sh
# Cameo container entrypoint.
#
# Default (no args, or only flags): run cameod with a loopback-only console.
# The built-in listener is HTTP-only, so a bearer key must not cross the LAN.
# Any other argv is exec'd as-is,
# so the image doubles as the CLI:
#   podman run --rm cameo:vulkan cameo pull tinyllama
set -eu

# Publish the baked-in starter GGUF into the (often empty) volume so
# `cameo serve qwen2.5-0.5b` works offline on first start.
if [ -x /usr/local/bin/cameo-seed-models ]; then
    /usr/local/bin/cameo-seed-models
fi

# An explicit command (not a flag) runs verbatim instead of the daemon.
if [ "$#" -gt 0 ]; then
    case "$1" in
    -*) : ;;          # a flag: fall through to the daemon, passing it along
    *) exec "$@" ;;   # a command: cameo, sh, ...
    esac
fi

# A non-loopback bind is an explicit operator opt-in. Keep authentication on it,
# while defaulting the image to loopback for SSH/VPN or reverse-proxy access.
export CAMEO_CONSOLE_HOST=${CAMEO_CONSOLE_HOST:-127.0.0.1}
if [ -z "${CAMEO_CONSOLE_KEY:-}" ] && [ "$CAMEO_CONSOLE_HOST" != "127.0.0.1" ]; then
    key=$(head -c 48 /dev/urandom | base64 | tr -dc 'A-Za-z0-9' | cut -c1-32)
    if [ -n "$key" ]; then
        CAMEO_CONSOLE_KEY=$key
        export CAMEO_CONSOLE_HOST CAMEO_CONSOLE_KEY
        key_file="${CAMEO_MODELS_DIR:-/var/lib/cameo/models}/.console-key"
        umask 077
        printf '%s\n' "$key" > "$key_file"
        echo "cameo: console on ${CAMEO_CONSOLE_HOST}:9090 with a generated bearer key"
        echo "cameo: read it with: podman exec <container> cat $key_file"
    else
        export CAMEO_CONSOLE_HOST=127.0.0.1
        echo "cameo: no entropy for a key; console bound loopback-only" >&2
    fi
fi

exec /usr/local/bin/cameod "$@"
