#!/usr/bin/env bash
# ==============================================================================
# Ruston Mail - Linux AppImage packaging script
# Builds ruston and ruston-cli, assembles an AppDir with linuxdeploy, checks it
# the way the AppImage catalog (appimage.github.io) does, and packs it into an
# AppImage with appimagetool.
# ==============================================================================

set -euo pipefail

log() { printf "\033[1;34m[INFO]\033[0m %s\n" "$*"; }
die() { printf "\033[1;31m[FAIL]\033[0m %s\n" "$*" >&2; exit 1; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
DIST_DIR="${DIST_DIR:-$REPO_ROOT/dist}"
TARGET_DIR="$REPO_ROOT/target/release"

APP_ID="com.luiscuellar.ruston-mail"
LINUXDEPLOY="${LINUXDEPLOY:-linuxdeploy}"
APPIMAGETOOL="${APPIMAGETOOL:-appimagetool}"

usage() {
    cat << EOF
Usage: $(basename "$0") [--skip-build]

Builds dist/ruston-mail-<version>-<arch>.AppImage for this machine's
architecture, with ruston-cli inside it.

Options:
  --skip-build          Package the existing target/release binaries

Environment variables:
  APPIMAGE_RUNTIME      AppImage runtime to embed (required; without it
                        appimagetool downloads whichever runtime is newest)
  LINUXDEPLOY           linuxdeploy to run (default: linuxdeploy on PATH)
  APPIMAGETOOL          appimagetool to run (default: appimagetool on PATH)
  APPIMAGE_UPDATE_INFORMATION
                        Update information to embed; a .zsync file is then
                        written beside the AppImage (needs zsyncmake)
  APPDIR_LINT           appdir-lint.sh to check the AppDir with, its
                        excludelist beside it (optional)
  RUSTON_MAX_GLIBC_VERSION
                        Fail if any ELF file needs a newer glibc (optional)
  DIST_DIR              Output directory (default: dist/)
EOF
}

SKIP_BUILD=0
while [[ $# -gt 0 ]]; do
    case "$1" in
        --skip-build)
            SKIP_BUILD=1
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            die "Unknown option: $1"
            ;;
    esac
done

command -v "$LINUXDEPLOY" > /dev/null 2>&1 || die "Missing linuxdeploy: $LINUXDEPLOY"
command -v "$APPIMAGETOOL" > /dev/null 2>&1 || die "Missing appimagetool: $APPIMAGETOOL"
[[ -f "${APPIMAGE_RUNTIME:-}" ]] || die "Set APPIMAGE_RUNTIME to a downloaded AppImage runtime"
for cmd in desktop-file-validate ldconfig readelf; do
    command -v "$cmd" > /dev/null 2>&1 || die "Missing required command: $cmd"
done

VERSION="$(grep -m 1 '^version = ' "$REPO_ROOT/Cargo.toml" | cut -d '"' -f 2)"
ARCH="$(uname -m)"
case "$ARCH" in
    x86_64) LDCONFIG_ARCH="x86-64" ;;
    aarch64) LDCONFIG_ARCH="AArch64" ;;
    *) die "Unsupported architecture: $ARCH" ;;
esac
OUTPUT="ruston-mail-${VERSION}-${ARCH}.AppImage"
APPDIR="$DIST_DIR/ruston-mail.AppDir"

log "Packaging Ruston Mail v$VERSION ($ARCH)"

# 1. Build release binaries if requested
if [[ "$SKIP_BUILD" -eq 0 ]]; then
    log "Building release binaries (ruston, ruston-cli)..."
    # --locked builds the audited dependency versions from Cargo.lock.
    (cd "$REPO_ROOT" && cargo build --locked --release -p ruston-mail -p ruston-cli --bins)
fi
for exe in ruston ruston-cli; do
    [[ -x "$TARGET_DIR/$exe" ]] || die "Binary not found at: $TARGET_DIR/$exe"
done

# The path of a system library for this architecture.
library_path() {
    local path
    path="$(ldconfig -p | awk -v name="$1" -v arch="$LDCONFIG_ARCH" \
        '$1 == name && index($0, arch) { print $NF; exit }')"
    [[ -n "$path" ]] || die "System library not found: $1"
    printf '%s\n' "$path"
}

# 2. Assemble the AppDir
log "Assembling AppDir at: $APPDIR"
mkdir -p "$DIST_DIR"
rm -rf "$APPDIR"

icon_args=()
for size in 16 24 32 48 64 128 256 512; do
    icon_args+=(--icon-file "$REPO_ROOT/assets/icons/hicolor/${size}x${size}/apps/$APP_ID.png")
done

# winit loads libxkbcommon and, on X11, libxkbcommon-x11, libXcursor and libXi
# at run time, and stops without them; not every system installs them, and
# AppImages bundle what is not on the excludelist. The executables get an
# $ORIGIN-relative RUNPATH, so those loads find the bundled copies. GL, libX11,
# libxcb and Wayland stay the system's.
"$LINUXDEPLOY" --appdir "$APPDIR" \
    --executable "$TARGET_DIR/ruston" \
    --executable "$TARGET_DIR/ruston-cli" \
    --desktop-file "$REPO_ROOT/packaging/linux/$APP_ID.desktop" \
    "${icon_args[@]}" \
    --library "$(library_path libxkbcommon.so.0)" \
    --library "$(library_path libxkbcommon-x11.so.0)" \
    --library "$(library_path libXcursor.so.1)" \
    --library "$(library_path libXi.so.6)" \
    --custom-apprun "$SCRIPT_DIR/AppRun"

install -Dm644 "$REPO_ROOT/packaging/linux/$APP_ID.appdata.xml" \
    "$APPDIR/usr/share/metainfo/$APP_ID.appdata.xml"
install -Dm644 "$REPO_ROOT/LICENSE" "$APPDIR/usr/share/doc/ruston-mail/LICENSE"
# The AppImage's thumbnail. appimagetool would only add it while packing,
# after the checks below, and from the smaller icon linuxdeploy put at the root.
ln -sf "usr/share/icons/hicolor/256x256/apps/$APP_ID.png" "$APPDIR/.DirIcon"

# 3. Check the AppDir before it becomes read-only
for library in libxkbcommon.so.0 libxkbcommon-x11.so.0 libXcursor.so.1 libXi.so.6; do
    [[ -f "$APPDIR/usr/lib/$library" ]] || die "$library was not bundled"
done
search_path="$(readelf -d "$APPDIR/usr/bin/ruston" | grep -E '\((RUNPATH|RPATH)\)' || true)"
[[ "$search_path" == *'$ORIGIN/../lib'* ]] ||
    die "ruston cannot find the bundled libraries (${search_path:-no RUNPATH})"
desktop-file-validate "$APPDIR/$APP_ID.desktop"
if [[ -n "${RUSTON_MAX_GLIBC_VERSION:-}" ]]; then
    "$SCRIPT_DIR/check-glibc.sh" "$APPDIR" "$RUSTON_MAX_GLIBC_VERSION"
fi
if [[ -n "${APPDIR_LINT:-}" ]]; then
    log "Running appdir-lint.sh"
    bash "$APPDIR_LINT" "$APPDIR"
fi

# 4. Pack the AppImage
log "Creating AppImage: $OUTPUT"
tool_args=(--runtime-file "$APPIMAGE_RUNTIME")
if [[ -n "${APPIMAGE_UPDATE_INFORMATION:-}" ]]; then
    tool_args+=(--updateinformation "$APPIMAGE_UPDATE_INFORMATION")
fi
(
    cd "$DIST_DIR"
    rm -f "$OUTPUT" "$OUTPUT.zsync" "$OUTPUT.sha256"
    ARCH="$ARCH" VERSION="$VERSION" "$APPIMAGETOOL" "${tool_args[@]}" "$APPDIR" "$OUTPUT"
    [[ -s "$OUTPUT" ]] || die "appimagetool did not produce: $OUTPUT"
    chmod 755 "$OUTPUT"

    if [[ -n "${APPIMAGE_UPDATE_INFORMATION:-}" ]]; then
        # Extraction mode would start the app instead of answering; reading
        # the embedded metadata needs no FUSE.
        embedded="$(env -u APPIMAGE_EXTRACT_AND_RUN "./$OUTPUT" --appimage-updateinformation)"
        [[ "$embedded" == "$APPIMAGE_UPDATE_INFORMATION" ]] ||
            die "Unexpected update information in the AppImage: $embedded"
        [[ -s "$OUTPUT.zsync" ]] || die "Missing or empty zsync file: $OUTPUT.zsync"
    fi
    sha256sum "$OUTPUT" | awk '{print $1}' > "$OUTPUT.sha256"
)

log "Done: $DIST_DIR/$OUTPUT"
