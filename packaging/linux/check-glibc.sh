#!/usr/bin/env bash
# ==============================================================================
# Ruston Mail - glibc floor check
# Fails if any ELF file under a path needs a newer glibc than the given
# version, so a build keeps running on the oldest system it is meant for.
# ==============================================================================

set -euo pipefail

log() { printf "\033[1;34m[glibc]\033[0m %s\n" "$*"; }
die() { printf "\033[1;31m[glibc]\033[0m %s\n" "$*" >&2; exit 1; }

usage() {
    cat << EOF
Usage: $(basename "$0") <ELF-file-or-directory> <max-glibc>

Checks every ELF file below the path and rejects any that needs a GLIBC symbol
version newer than <max-glibc>, such as 2.35 for Ubuntu 22.04.
EOF
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
    usage
    exit 0
fi
[[ $# -eq 2 ]] || { usage >&2; exit 2; }

ROOT_PATH="$1"
MAX_GLIBC="$2"
[[ -e "$ROOT_PATH" ]] || die "Path does not exist: $ROOT_PATH"
[[ "$MAX_GLIBC" =~ ^[0-9]+\.[0-9]+$ ]] || die "Invalid maximum glibc version: $MAX_GLIBC"
command -v readelf > /dev/null 2>&1 || die "Missing required tool: readelf"

# "a is newer than b", for dotted versions.
newer() {
    [[ "$1" != "$2" && "$(printf '%s\n%s\n' "$1" "$2" | sort -V | tail -n 1)" == "$1" ]]
}

checked=0
failed=0
newest_seen=""
while IFS= read -r -d '' file; do
    readelf -h "$file" > /dev/null 2>&1 || continue
    checked=$((checked + 1))
    needed="$(readelf --version-info --wide "$file" 2> /dev/null |
        grep -Eo 'GLIBC_[0-9]+(\.[0-9]+)+' | sed 's/^GLIBC_//' | sort -Vu | tail -n 1 || true)"
    [[ -n "$needed" ]] || continue
    if [[ -z "$newest_seen" ]] || newer "$needed" "$newest_seen"; then
        newest_seen="$needed"
    fi
    if newer "$needed" "$MAX_GLIBC"; then
        printf '[glibc] %s needs GLIBC_%s (allowed: GLIBC_%s)\n' "$file" "$needed" "$MAX_GLIBC" >&2
        failed=1
    fi
done < <(find -L "$ROOT_PATH" -type f -print0)

[[ $checked -gt 0 ]] || die "No ELF files found under: $ROOT_PATH"
[[ $failed -eq 0 ]] || die "glibc floor check failed"
log "Checked $checked ELF files; newest glibc needed: ${newest_seen:-none} (allowed: $MAX_GLIBC)"
