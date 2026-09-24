#!/usr/bin/env bash
# ==============================================================================
# Ruston Mail - macOS Application & DMG Packaging Script
# Builds Ruston Mail.app, signs it ad-hoc (or with Developer ID), and creates a
# drag-and-drop installer disk image (.dmg).
# ==============================================================================

set -euo pipefail

# Helper logging
log()  { printf "\033[1;34m[INFO]\033[0m %s\n" "$*"; }
warn() { printf "\033[1;33m[WARN]\033[0m %s\n" "$*" >&2; }
die()  { printf "\033[1;31m[FAIL]\033[0m %s\n" "$*" >&2; exit 1; }

# Paths and coordinates
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
ASSETS_DIR="$REPO_ROOT/assets/macos"
DIST_DIR="$REPO_ROOT/dist"

APP_NAME="Ruston Mail"
EXECUTABLE="ruston"
IDENTIFIER="com.luiscuellar.ruston-mail"

# Usage instructions
usage() {
    cat << EOF
Usage: $(basename "$0") [OPTIONS]

Packages Ruston Mail as a native macOS application bundle and draggable DMG.

Options:
  --skip-build          Skip cargo build --release and package existing binary
  --target <TRIPLE>     Specify target architecture triple (e.g. aarch64-apple-darwin)
  --binary <PATH>       Path to custom pre-built executable
  --dist <PATH>         Custom output directory (default: dist/)
  -h, --help            Show this help message

Environment variables:
  CODESIGN_IDENTITY     Signing identity (default: "-" for ad-hoc signing)
EOF
    exit 0
}

SKIP_BUILD=0
TARGET_TRIPLE=""
CUSTOM_BINARY=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --skip-build)
            SKIP_BUILD=1
            shift
            ;;
        --target)
            TARGET_TRIPLE="$2"
            shift 2
            ;;
        --binary)
            CUSTOM_BINARY="$2"
            shift 2
            ;;
        --dist)
            DIST_DIR="$2"
            shift 2
            ;;
        -h|--help)
            usage
            ;;
        *)
            die "Unknown option: $1"
            ;;
    esac
done

# Check essential host tools
for cmd in codesign hdiutil sips; do
    command -v "$cmd" >/dev/null 2>&1 || die "Missing required command: $cmd"
done

# Read version from Cargo.toml
VERSION="$(grep -m 1 '^version = ' "$REPO_ROOT/Cargo.toml" | cut -d '"' -f 2)"
BUILD="${VERSION%%-*}"
ARCH="$(uname -m)"
[[ -n "$TARGET_TRIPLE" ]] && ARCH="${TARGET_TRIPLE%%-*}"

log "Packaging $APP_NAME v$VERSION ($ARCH)"

# 1. Build release binary if requested
BINARY_PATH="$CUSTOM_BINARY"
if [[ -z "$BINARY_PATH" ]]; then
    if [[ -n "$TARGET_TRIPLE" ]]; then
        BINARY_PATH="$REPO_ROOT/target/$TARGET_TRIPLE/release/$EXECUTABLE"
    else
        BINARY_PATH="$REPO_ROOT/target/release/$EXECUTABLE"
    fi

    if [[ "$SKIP_BUILD" -eq 0 ]]; then
        log "Building release binary ($EXECUTABLE)..."
        BUILD_ARGS=(--release --bin "$EXECUTABLE")
        if [[ -n "$TARGET_TRIPLE" ]]; then
            BUILD_ARGS+=(--target "$TARGET_TRIPLE")
        fi
        (cd "$REPO_ROOT" && cargo build "${BUILD_ARGS[@]}")
    fi
fi

[[ -f "$BINARY_PATH" ]] || die "Binary not found at: $BINARY_PATH"

mkdir -p "$DIST_DIR"

# 2. Ensure icon exists
ICNS_PATH="$ASSETS_DIR/ruston-mail.icns"
if [[ ! -f "$ICNS_PATH" ]]; then
    PNG_SOURCE="$ASSETS_DIR/ruston-mail-1024.png"
    [[ -f "$PNG_SOURCE" ]] || PNG_SOURCE="$ASSETS_DIR/icon-1024.png"

    if [[ -f "$PNG_SOURCE" ]]; then
        log "Generating .icns from $(basename "$PNG_SOURCE")..."
        iconset_temp="$(mktemp -d)"
        iconset="$iconset_temp/ruston-mail.iconset"
        mkdir -p "$iconset"
        for size in 16 32 128 256 512; do
            sips -z "$size" "$size" "$PNG_SOURCE" --out "$iconset/icon_${size}x${size}.png" >/dev/null
            double=$((size * 2))
            sips -z "$double" "$double" "$PNG_SOURCE" --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
        done
        iconutil -c icns "$iconset" -o "$ICNS_PATH"
        rm -rf "$iconset_temp"
    else
        die "Neither 'ruston-mail.icns' nor 'ruston-mail-1024.png' was found in $ASSETS_DIR"
    fi
fi

# 3. Assemble .app bundle
APP_DIR="$DIST_DIR/$APP_NAME.app"
log "Assembling bundle at: $APP_DIR"
rm -rf "$APP_DIR"
mkdir -p "$APP_DIR/Contents/MacOS" "$APP_DIR/Contents/Resources"

# Copy binary
cp "$BINARY_PATH" "$APP_DIR/Contents/MacOS/$EXECUTABLE"
chmod 755 "$APP_DIR/Contents/MacOS/$EXECUTABLE"

# Copy icon
cp "$ICNS_PATH" "$APP_DIR/Contents/Resources/ruston-mail.icns"

# Configure Info.plist
PLIST_TEMPLATE="$ASSETS_DIR/Info.plist"
[[ -f "$PLIST_TEMPLATE" ]] || die "Info.plist template missing at: $PLIST_TEMPLATE"
sed \
    -e "s/__VERSION__/$VERSION/g" \
    -e "s/__BUILD__/$BUILD/g" \
    -e "s/__IDENTIFIER__/$IDENTIFIER/g" \
    -e "s/__EXECUTABLE__/$EXECUTABLE/g" \
    "$PLIST_TEMPLATE" > "$APP_DIR/Contents/Info.plist"

# 4. Code signing (Ad-hoc by default, or with Developer ID if configured)
ENTITLEMENTS="$ASSETS_DIR/entitlements.plist"
SIGN_ARGS=(--force)

if [[ -n "${CODESIGN_IDENTITY:-}" ]]; then
    log "Signing bundle with Developer ID: $CODESIGN_IDENTITY"
    SIGN_ARGS+=(--timestamp --options runtime --sign "$CODESIGN_IDENTITY")
else
    log "Signing bundle with ad-hoc identity (resolves Keychain re-prompting)"
    SIGN_ARGS+=(-s - --timestamp=none)
fi

if [[ -f "$ENTITLEMENTS" ]]; then
    SIGN_ARGS+=(--entitlements "$ENTITLEMENTS")
fi

codesign "${SIGN_ARGS[@]}" "$APP_DIR"
log "Verifying code signature..."
codesign --verify --deep --strict --verbose=2 "$APP_DIR"

# 5. Create drag-and-drop DMG
DMG_NAME="ruston-mail-${VERSION}-${ARCH}.dmg"
DMG_PATH="$DIST_DIR/$DMG_NAME"
log "Creating disk image ($DMG_NAME)..."

STAGING="$(mktemp -d -t ruston-mail-dmg)"
trap 'rm -rf "$STAGING"' EXIT

cp -R "$APP_DIR" "$STAGING/"
ln -s /Applications "$STAGING/Applications"

if [[ -f "$ASSETS_DIR/dmg-readme.txt" ]]; then
    cp "$ASSETS_DIR/dmg-readme.txt" "$STAGING/README.txt"
fi

rm -f "$DMG_PATH"

hdiutil create -ov -fs HFS+ -volname "$APP_NAME" \
    -srcfolder "$STAGING" \
    -format UDZO \
    -imagekey zlib-level=9 \
    "$DMG_PATH" >/dev/null

log "Verifying DMG integrity..."
hdiutil verify "$DMG_PATH" >/dev/null

# 6. Generate SHA-256 checksum
SHA_FILE="${DMG_PATH}.sha256"
shasum -a 256 "$DMG_PATH" | awk '{print $1}' > "$SHA_FILE"
SHA_VAL="$(cat "$SHA_FILE")"

log "========================================================"
log "Successfully packaged $APP_NAME!"
log "  Bundle:   $APP_DIR"
log "  DMG:      $DMG_PATH"
log "  SHA-256:  $SHA_VAL"
log "========================================================"
