#!/usr/bin/env bash
# ==============================================================================
# Ruston Mail - startup smoke test
# Starts the desktop app with a throwaway home directory, so no saved session
# or setting is read, and fails if it exits before the given number of seconds.
# The app then shows its sign-in screen, as on a new installation.
# ==============================================================================

set -euo pipefail

usage() {
    cat << EOF
Usage: $(basename "$0") [--seconds N] <command> [args...]

Runs <command> with HOME and the XDG directories pointed at an empty temporary
directory. Succeeds if it is still running after N seconds (default: 10), then
stops it; otherwise prints its output and fails.
EOF
}

ALIVE_SECONDS=10
while [[ $# -gt 0 ]]; do
    case "$1" in
        --seconds)
            ALIVE_SECONDS="$2"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            break
            ;;
    esac
done
[[ $# -gt 0 ]] || { usage >&2; exit 2; }
[[ "$ALIVE_SECONDS" =~ ^[0-9]+$ ]] || { echo "Invalid --seconds: $ALIVE_SECONDS" >&2; exit 2; }

home="$(mktemp -d)"
trap 'rm -rf "$home"' EXIT
mkdir -p "$home/.config" "$home/.local/share" "$home/.cache"

HOME="$home" \
    XDG_CONFIG_HOME="$home/.config" \
    XDG_DATA_HOME="$home/.local/share" \
    XDG_CACHE_HOME="$home/.cache" \
    "$@" > "$home/output.log" 2>&1 &
pid=$!

for ((i = 0; i < ALIVE_SECONDS * 2; i++)); do
    if ! kill -0 "$pid" 2>/dev/null; then
        status=0
        wait "$pid" || status=$?
        echo "[smoke] $1 exited after about $((i / 2)) s with status $status:" >&2
        cat "$home/output.log" >&2
        exit 1
    fi
    sleep 0.5
done

kill "$pid" 2>/dev/null || true
wait "$pid" 2>/dev/null || true
echo "[smoke] $1 was still running after $ALIVE_SECONDS s"
