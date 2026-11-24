#!/usr/bin/env bash
# ==============================================================================
# Ruston Mail - README screenshots
# Takes the main window's screenshots in assets/screenshots again: the demo's
# inbox with a conversation open, dark and light, as macOS draws the window.
# Run it on a Mac with a Retina display and leave the window in front until it
# closes. See "README screenshots" in docs/DEVELOPMENT.md.
# ==============================================================================

set -euo pipefail

if [[ "$(uname -s)" != Darwin ]]; then
    echo "The README shows Ruston Mail on macOS; take its screenshots there." >&2
    exit 1
fi

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TARGET_DIR="$REPO_ROOT/target/readme-screenshots"
FINISH_SECONDS=120

# A folder of its own, so packaging never picks up this build, and without the
# release profile's fat LTO, which takes minutes and a screenshot does not need.
(cd "$REPO_ROOT" && CARGO_TARGET_DIR="$TARGET_DIR" \
    CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 \
    cargo build --locked --release -p ruston-mail --features readme-screenshots)

# A home of its own keeps your settings, the demo's included, as they are.
home="$(mktemp -d)"
trap 'rm -rf "$home"' EXIT
shots="$home/shots"
mkdir -p "$shots"

HOME="$home" RUSTON_DEMO=1 RUSTON_README_SCREENSHOTS="$shots" \
    "$TARGET_DIR/release/ruston" &
pid=$!

# A covered window or a locked screen draws nothing, and would wait forever.
for ((i = 0; i < FINISH_SECONDS; i++)); do
    kill -0 "$pid" 2>/dev/null || break
    sleep 1
done
if kill -0 "$pid" 2>/dev/null; then
    kill "$pid" 2>/dev/null || true
    echo "The window did not finish in $FINISH_SECONDS s; keep the screen unlocked and the window in front." >&2
    exit 1
fi
# The files say whether it worked; the app has already said why if not.
wait "$pid" || true

for name in main-window-dark main-window-light; do
    if [[ ! -s "$shots/$name.png" ]]; then
        echo "$name.png was not taken." >&2
        exit 1
    fi
done
mkdir -p "$REPO_ROOT/assets/screenshots"
cp "$shots/main-window-dark.png" "$shots/main-window-light.png" "$REPO_ROOT/assets/screenshots/"
echo "Updated assets/screenshots/main-window-dark.png and main-window-light.png"
