#!/usr/bin/env bash
# Prints the version in [workspace.package] of the root Cargo.toml, the one
# every crate and every release artifact takes. Only that table is read: the
# first `version = ` of the file need not be its own.
set -euo pipefail

manifest="$(dirname "${BASH_SOURCE[0]}")/../Cargo.toml"
version="$(sed -n '/^\[workspace\.package\]/,/^\[/ s/^version = "\([^"]*\)"$/\1/p' "$manifest" | head -n 1)"
if [[ -z "$version" ]]; then
    echo "No version in [workspace.package] of $manifest" >&2
    exit 1
fi
printf '%s\n' "$version"
