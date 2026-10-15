#!/usr/bin/env bash
# ==============================================================================
# Ruston Mail - macOS bundle verification
# Checks a packaged Ruston Mail.app before it ships: its signature, what each
# executable is built for and links against, and the version it reports.
# ==============================================================================

set -euo pipefail

log() { printf "\033[1;34m[verify]\033[0m %s\n" "$*"; }
die() { printf "\033[1;31m[FAIL]\033[0m %s\n" "$*" >&2; exit 1; }

usage() {
    cat << EOF
Usage: $(basename "$0") <path-to-Ruston-Mail.app> <arm64|x86_64>

Checks that:
- the signature verifies (deep and strict) and turns on the hardened runtime,
  for the app and for the ruston-cli inside it;
- ruston and ruston-cli contain only the given architecture, need no newer
  macOS than LSMinimumSystemVersion, and link only system libraries;
- the bundle and ruston-cli report the version in Cargo.toml.
EOF
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
    usage
    exit 0
fi
[[ $# -eq 2 ]] || { usage >&2; exit 2; }

APP_DIR="$1"
ARCH="$2"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
PLIST="$APP_DIR/Contents/Info.plist"
EXECUTABLES=("$APP_DIR/Contents/MacOS/ruston" "$APP_DIR/Contents/MacOS/ruston-cli")

[[ -d "$APP_DIR" ]] || die "App bundle not found: $APP_DIR"
[[ "$ARCH" == "arm64" || "$ARCH" == "x86_64" ]] || die "Unknown architecture: $ARCH"
for exe in "${EXECUTABLES[@]}"; do
    [[ -x "$exe" ]] || die "Missing executable: $exe"
done

VERSION="$(grep -m 1 '^version = ' "$REPO_ROOT/Cargo.toml" | cut -d '"' -f 2)"
MINIMUM_MACOS="$(/usr/libexec/PlistBuddy -c 'Print :LSMinimumSystemVersion' "$PLIST")"

# "a is not newer than b", for dotted versions.
not_newer() {
    [[ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | tail -n 1)" == "$2" ]]
}

log "Signature"
codesign --verify --deep --strict "$APP_DIR"
for code in "$APP_DIR" "$APP_DIR/Contents/MacOS/ruston-cli"; do
    flags="$(codesign -dv "$code" 2>&1 | sed -n 's/^CodeDirectory.*flags=\([^ ]*\).*/\1/p')"
    [[ "$flags" == *runtime* ]] || die "Hardened runtime is off for $code (flags: $flags)"
done

for exe in "${EXECUTABLES[@]}"; do
    name="$(basename "$exe")"
    log "$name"

    archs="$(lipo -archs "$exe")"
    [[ "$archs" == "$ARCH" ]] || die "$name is built for '$archs', expected '$ARCH'"

    # LC_BUILD_VERSION names it minos; older targets use LC_VERSION_MIN_MACOSX.
    minos="$(otool -l "$exe" | awk '
        /LC_BUILD_VERSION|LC_VERSION_MIN_MACOSX/ { found = 1 }
        found && ($1 == "minos" || $1 == "version") { print $2; exit }')"
    [[ -n "$minos" ]] || die "$name has no minimum macOS version"
    not_newer "$minos" "$MINIMUM_MACOS" ||
        die "$name needs macOS $minos, newer than LSMinimumSystemVersion $MINIMUM_MACOS"

    while IFS= read -r library; do
        case "$library" in
            /System/Library/* | /usr/lib/*) ;;
            *) die "$name links a library outside the system: $library" ;;
        esac
    done < <(otool -L "$exe" | tail -n +2 | awk '{print $1}')
done

log "Version"
bundle_version="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$PLIST")"
[[ "$bundle_version" == "$VERSION" ]] ||
    die "Info.plist says $bundle_version, Cargo.toml says $VERSION"
cli_version="$("$APP_DIR/Contents/MacOS/ruston-cli" --version)"
[[ "$cli_version" == "ruston-cli $VERSION" ]] ||
    die "ruston-cli reports '$cli_version', expected 'ruston-cli $VERSION'"

log "$APP_DIR is ready: $ARCH, macOS $MINIMUM_MACOS or later, version $VERSION"
