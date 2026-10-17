#!/usr/bin/env bash
# ==============================================================================
# Ruston Mail - release notes
# Writes the draft notes for a tagged release from its Conventional Commits:
# feat commits become the highlights, and every commit since the previous tag
# lands in the full changelog, grouped by type.
# ==============================================================================

set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  ./packaging/release-notes.sh --tag <tag> [options]

Options:
  --tag <tag>          Tag to generate notes for (e.g. vX.Y.Z). Required.
  --version <version>  Version used in artifact names (default: tag without leading v).
  --repo <owner/repo>  GitHub repository for compare links and attestations (optional).
  --range <A..B>       Explicit git range. If omitted, uses previous tag..tag.
  --output <file>      Output markdown path. If omitted, prints to stdout.
EOF
}

die() {
  echo "[release-notes] $*" >&2
  exit 1
}

append_item() {
  local var_name="$1"
  local text="$2"
  printf -v "$var_name" '%s- %s\n' "${!var_name}" "$text"
}

print_section() {
  local title="$1"
  local items="$2"
  [[ -n "$items" ]] || return 0
  printf '## %s\n\n%s\n' "$title" "$items"
}

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "$REPO_ROOT"

TAG=""
VERSION=""
REPO=""
RANGE=""
OUTPUT=""

while (($#)); do
  case "$1" in
  --tag)
    (($# >= 2)) || die "Missing value for --tag"
    TAG="$2"
    shift 2
    ;;
  --version)
    (($# >= 2)) || die "Missing value for --version"
    VERSION="$2"
    shift 2
    ;;
  --repo)
    (($# >= 2)) || die "Missing value for --repo"
    REPO="$2"
    shift 2
    ;;
  --range)
    (($# >= 2)) || die "Missing value for --range"
    RANGE="$2"
    shift 2
    ;;
  --output)
    (($# >= 2)) || die "Missing value for --output"
    OUTPUT="$2"
    shift 2
    ;;
  -h | --help)
    usage
    exit 0
    ;;
  *)
    die "Unknown argument: $1"
    ;;
  esac
done

[[ -n "$TAG" ]] || {
  usage
  die "--tag is required"
}

git rev-parse --verify "${TAG}^{commit}" >/dev/null 2>&1 ||
  die "Tag not found (or not resolvable as commit): ${TAG}"

if [[ -z "$VERSION" ]]; then
  VERSION="${TAG#v}"
fi

PREV_TAG=""
if [[ -z "$RANGE" ]]; then
  PREV_TAG="$(git describe --tags --abbrev=0 "${TAG}^" 2>/dev/null || true)"
  if [[ -n "$PREV_TAG" ]]; then
    RANGE="${PREV_TAG}..${TAG}"
  else
    RANGE="${TAG}"
  fi
fi

commits_raw="$(
  git log --no-merges --reverse \
    --pretty=tformat:'%H%x1f%s%x1f%b%x1e' \
    "$RANGE"
)"

breaking_items=""
feat_items=""
fix_items=""
perf_items=""
refactor_items=""
docs_items=""
maintenance_items=""
other_items=""

if [[ -n "$commits_raw" ]]; then
  while IFS= read -r -d $'\x1e' record; do
    [[ -n "$record" ]] || continue
    record="${record#$'\n'}"
    if [[ "$record" != *$'\x1f'* ]]; then
      continue
    fi
    sha="${record%%$'\x1f'*}"
    rest="${record#*$'\x1f'}"
    if [[ "$rest" != *$'\x1f'* ]]; then
      continue
    fi
    subject="${rest%%$'\x1f'*}"
    body="${rest#*$'\x1f'}"

    sha="${sha//$'\r'/}"
    subject="${subject//$'\r'/}"
    short_sha="${sha:0:8}"

    type=""
    scope=""
    bang=""
    desc="$subject"
    if [[ "$subject" == *:* ]]; then
      prefix="${subject%%:*}"
      raw_desc="${subject#*:}"
      raw_desc="${raw_desc#"${raw_desc%%[![:space:]]*}"}"
      if [[ "$prefix" =~ ^([A-Za-z]+)(\(.*\))?(!)?$ ]]; then
        type="$(printf '%s' "${BASH_REMATCH[1]}" | tr '[:upper:]' '[:lower:]')"
        scope="${BASH_REMATCH[2]}"
        bang="${BASH_REMATCH[3]}"
        desc="$raw_desc"
      fi
    fi

    if [[ -n "$scope" ]]; then
      scope_name="${scope#(}"
      scope_name="${scope_name%)}"
      desc="${scope_name}: ${desc}"
    fi

    entry="${desc} (${short_sha})"

    is_breaking=0
    breaking_note=""
    if [[ -n "$bang" ]]; then
      is_breaking=1
    fi
    if [[ "$body" == *"BREAKING CHANGE:"* || "$body" == *"BREAKING-CHANGE:"* ]]; then
      is_breaking=1
      while IFS= read -r line; do
        if [[ "$line" =~ ^BREAKING[[:space:]-]CHANGE:[[:space:]]*(.+)$ ]]; then
          breaking_note="${BASH_REMATCH[1]}"
          break
        fi
      done <<<"$body"
    fi
    if ((is_breaking)); then
      if [[ -n "$breaking_note" ]]; then
        append_item breaking_items "${breaking_note} (${short_sha})"
      else
        append_item breaking_items "$entry"
      fi
    fi

    case "$type" in
    feat)
      append_item feat_items "$entry"
      ;;
    fix)
      append_item fix_items "$entry"
      ;;
    perf)
      append_item perf_items "$entry"
      ;;
    refactor)
      append_item refactor_items "$entry"
      ;;
    docs)
      append_item docs_items "$entry"
      ;;
    build | ci | chore | style | test | revert)
      append_item maintenance_items "$entry"
      ;;
    "")
      append_item other_items "${subject} (${short_sha})"
      ;;
    *)
      append_item other_items "$entry"
      ;;
    esac
  done < <(printf '%s' "$commits_raw")
fi

{
  cat <<EOF
<!-- RELEASE OVERVIEW: edit this block before publishing. Highlights come from feat commits. -->
# Release name (${TAG})

_Add a short introduction for this release._

<!-- END RELEASE OVERVIEW -->

EOF

  print_section "Highlights" "$feat_items"

  cat <<EOF
<details>
<summary><strong>Download and install</strong></summary>

## Verify downloads

Verify the download first. \`SHA256SUMS.txt\` is attached to this release:

\`\`\`bash
shasum -a 256 -c SHA256SUMS.txt --ignore-missing  # macOS
sha256sum -c SHA256SUMS.txt --ignore-missing      # Linux
\`\`\`

On Windows, compare the hash PowerShell prints with the file's line in \`SHA256SUMS.txt\`:

\`\`\`powershell
Get-FileHash .\\ruston-mail-${VERSION}-x86_64-setup-UNSIGNED.exe
\`\`\`
EOF

  if [[ -n "$REPO" ]]; then
    cat <<EOF

Each file also carries a build provenance attestation from this repository's
release workflow, which the GitHub CLI can check:

\`\`\`bash
gh attestation verify ruston-mail-${VERSION}-x86_64.AppImage --repo ${REPO}
\`\`\`
EOF
  fi

  cat <<EOF

### macOS

Download \`ruston-mail-${VERSION}-arm64-UNSIGNED.dmg\` on Apple Silicon or \`ruston-mail-${VERSION}-x86_64-UNSIGNED.dmg\` on Intel, open it, and drag Ruston Mail onto Applications. macOS 11 or later is required.

Ruston Mail is not signed with an Apple Developer ID, so macOS blocks the first launch and reports that it cannot check the app for malicious software. To allow it, open **System Settings → Privacy & Security**, scroll down to the Security section, click **Open Anyway** next to the message about Ruston Mail, and confirm. On macOS 12 and 13 the same setting lives in **System Preferences → Security & Privacy → General**. The DMG ships these steps as \`README.txt\`.

Because the build is signed ad-hoc, macOS treats every new version as a different app: after an update it asks again before Ruston Mail can read the session saved in the keychain.

The command-line client, \`ruston-cli\`, is inside the app. To put it on your \`PATH\`:

\`\`\`bash
sudo ln -sf "/Applications/Ruston Mail.app/Contents/MacOS/ruston-cli" /usr/local/bin/ruston-cli
\`\`\`

### Linux

The AppImage runs on distributions with glibc 2.35 or newer, such as Ubuntu 22.04, Debian 12 and Fedora 36 or later:

\`\`\`bash
chmod +x ruston-mail-${VERSION}-x86_64.AppImage
./ruston-mail-${VERSION}-x86_64.AppImage
\`\`\`

The command-line client is inside it. Run it with \`--cli\`, or through a link named \`ruston-cli\`:

\`\`\`bash
./ruston-mail-${VERSION}-x86_64.AppImage --cli --help
ln -sf "\$PWD/ruston-mail-${VERSION}-x86_64.AppImage" ~/.local/bin/ruston-cli
\`\`\`

Replace \`x86_64\` with \`aarch64\` on ARM64 machines. Without FUSE, add \`--appimage-extract-and-run\` after the AppImage's name.

### Windows

Run \`ruston-mail-${VERSION}-x86_64-setup-UNSIGNED.exe\`, or \`ruston-mail-${VERSION}-arm64-setup-UNSIGNED.exe\` on ARM64 PCs. It installs for your user without administrator rights, adds Ruston Mail to the Start menu, and can offer it for email links and put \`ruston-cli\` on your \`PATH\`. Windows 10 or later is required. To run Ruston Mail without installing it, extract \`ruston-mail-${VERSION}-x86_64-windows-UNSIGNED.zip\` (or the \`arm64\` one) and start \`ruston.exe\`; its \`README.txt\` covers email links and removal.

The files are not signed, so SmartScreen may say that it protected your PC: choose **More info**, then **Run anyway**. Windows with Smart App Control turned on does not run unsigned apps.

The command-line client, \`ruston-cli.exe\`, sits beside \`ruston.exe\` in the installation folder, \`%LOCALAPPDATA%\\Programs\\Ruston Mail\`.

</details>

<details>
<summary><strong>Full changelog</strong></summary>

EOF

  print_section "Breaking changes" "$breaking_items"
  print_section "Fixes" "$fix_items"
  print_section "Performance" "$perf_items"
  print_section "Refactors" "$refactor_items"
  print_section "Documentation" "$docs_items"
  print_section "Maintenance" "$maintenance_items"
  print_section "Other changes" "$other_items"

  echo "</details>"
  echo
  if [[ -n "$PREV_TAG" && -n "$REPO" ]]; then
    echo "**Full comparison:** [\`${PREV_TAG}...${TAG}\`](https://github.com/${REPO}/compare/${PREV_TAG}...${TAG})"
  elif [[ -n "$PREV_TAG" ]]; then
    echo "**Commit range:** \`${PREV_TAG}..${TAG}\`"
  else
    echo "**Commit range:** \`${TAG}\` (first release: every commit up to this tag)"
  fi
} >"${OUTPUT:-/dev/stdout}"
