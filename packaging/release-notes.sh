#!/usr/bin/env bash
# ==============================================================================
# Ruston Mail - release notes
# Writes the draft notes for a tagged release from its Conventional Commits
# since the previous tag: new features, fixes and performance work under the
# areas in release-note-areas.tsv, and everything users do not see in a short
# Maintenance list before the download instructions.
#
# The release workflow runs it with Ubuntu's bash 5, and a Mac with the bash
# 3.2 it ships, so it keeps to what bash 3.2 has: no associative arrays, no
# namerefs and no case changes in expansions. Under set -u, bash 3.2 calls an
# empty array unbound in "${array[@]}", so such expansions wait for a check
# that the array has elements. Without namerefs, a helper hands back several
# values by setting variables its caller declares local.
# ==============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
AREAS_FILE="${SCRIPT_DIR}/release-note-areas.tsv"

# git log ends a commit's fields with the unit separator and the commit with
# the record separator: a subject or a body can hold any other character.
FIELD_SEP=$'\x1f'
RECORD_SEP=$'\x1e'
# The version bump is the release itself, not a change in it.
VERSION_BUMP_PATTERN='^[[:space:]]*[Bb]ump version'

# The options.
TAG=""
VERSION=""
REPO=""
RANGE=""
OUTPUT=""
# The tag before TAG, when the range is the commits since it.
PREV_TAG=""

# The areas, in the order of the notes, each with its scopes between spaces,
# and the scopes of work users do not see. Other, the last area, takes the
# changes whose scope no area lists.
AREA_NAMES=()
AREA_SCOPES=()
INTERNAL_SCOPES=" "
OTHER_AREA=0

# What the commits bring. Per area, the lists of new features, fixes and
# performance changes, one item a line, and how many those lists hold.
FEAT_ITEMS=()
FIX_ITEMS=()
PERF_ITEMS=()
AREA_TOTALS=()
# Across areas, and for the lists that go by no area.
FEAT_COUNT=0
FIX_COUNT=0
PERF_COUNT=0
BREAKING_ITEMS=""
MAINTENANCE_ITEMS=""
MAINTENANCE_COUNT=0
# The scopes filed under Other, between spaces.
UNLISTED_SCOPES=" "

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

warn() {
  echo "[release-notes] $*" >&2
}

die() {
  warn "$@"
  exit 1
}

# Turns a commit description into a sentence: single spaces and a capital
# first letter.
sentence() {
  local -a words
  read -r -a words <<<"$1"
  local text="${words[*]}"
  local first
  first="$(printf '%s' "${text:0:1}" | tr '[:lower:]' '[:upper:]')"
  printf '%s' "${first}${text:1}"
}

# Prints "1 fix" or "3 fixes".
count_phrase() {
  if (($1 == 1)); then
    printf '1 %s' "$2"
  else
    printf '%s %s' "$1" "$3"
  fi
}

# Joins words as "A", "A and B" or "A, B and C".
join_phrases() {
  local result="" index
  for ((index = 1; index <= $#; index++)); do
    if ((index == 1)); then
      result="${!index}"
    elif ((index == $#)); then
      result+=" and ${!index}"
    else
      result+=", ${!index}"
    fi
  done
  printf '%s' "$result"
}

# --- Arguments ----------------------------------------------------------------

parse_arguments() {
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
}

# Checks the tag and fills in what the options left out: the version, from
# the tag, and the range, the commits since the previous tag or, for the
# first one, every commit up to it.
resolve_release() {
  git rev-parse --verify "${TAG}^{commit}" >/dev/null 2>&1 ||
    die "Tag not found (or not resolvable as commit): ${TAG}"

  if [[ -z "$VERSION" ]]; then
    VERSION="${TAG#v}"
  fi

  if [[ -z "$RANGE" ]]; then
    PREV_TAG="$(git describe --tags --abbrev=0 "${TAG}^" 2>/dev/null || true)"
    if [[ -n "$PREV_TAG" ]]; then
      RANGE="${PREV_TAG}..${TAG}"
    else
      RANGE="${TAG}"
    fi
  fi
}

# --- Areas --------------------------------------------------------------------

# Sets the caller's area to an area index, to "internal", or to "" for a
# scope that release-note-areas.tsv does not list.
find_area() {
  local scope="$1" index
  area=""
  [[ -n "$scope" ]] || return 0
  if [[ "$INTERNAL_SCOPES" == *" ${scope} "* ]]; then
    area="internal"
    return 0
  fi
  for index in "${!AREA_SCOPES[@]}"; do
    if [[ "${AREA_SCOPES[$index]}" == *" ${scope} "* ]]; then
      area="$index"
      return 0
    fi
  done
}

# Reads release-note-areas.tsv, where each line holds an area's name, a tab
# and its scopes, then adds Other and gives every area empty lists.
load_areas() {
  local line line_number=0 area_name scope area index
  local -a area_scopes

  [[ -r "$AREAS_FILE" ]] || die "Cannot read ${AREAS_FILE}"
  while IFS= read -r line || [[ -n "$line" ]]; do
    line_number=$((line_number + 1))
    [[ -z "$line" || "$line" == \#* ]] && continue
    [[ "$line" == *$'\t'* ]] ||
      die "${AREAS_FILE}:${line_number}: expected an area, a tab and its scopes"
    area_name="${line%%$'\t'*}"
    read -r -a area_scopes <<<"${line#*$'\t'}"
    [[ -n "$area_name" && ${#area_scopes[@]} -gt 0 ]] ||
      die "${AREAS_FILE}:${line_number}: an area needs a name and scopes"
    for scope in "${area_scopes[@]}"; do
      find_area "$scope"
      [[ -z "$area" ]] ||
        die "${AREAS_FILE}:${line_number}: scope '${scope}' is listed twice"
    done
    if [[ "$area_name" == "internal" ]]; then
      INTERNAL_SCOPES+="${area_scopes[*]} "
    else
      AREA_NAMES+=("$area_name")
      AREA_SCOPES+=(" ${area_scopes[*]} ")
    fi
  done <"$AREAS_FILE"
  ((${#AREA_NAMES[@]} > 0)) || die "${AREAS_FILE} lists no areas"

  OTHER_AREA=${#AREA_NAMES[@]}
  AREA_NAMES+=("Other")
  for index in "${!AREA_NAMES[@]}"; do
    FEAT_ITEMS[index]=""
    FIX_ITEMS[index]=""
    PERF_ITEMS[index]=""
    AREA_TOTALS[index]=0
  done
}

# --- Commits ------------------------------------------------------------------

# Files every commit of the range, oldest first, then names the scopes whose
# changes went to Other.
collect_commits() {
  local commits record sha subject body scope

  commits="$(
    git log --no-merges --reverse \
      --pretty="tformat:%H${FIELD_SEP}%s${FIELD_SEP}%b${RECORD_SEP}" \
      "$RANGE"
  )"
  if [[ -n "$commits" ]]; then
    while IFS= read -r -d "$RECORD_SEP" record; do
      split_commit_record "$record" || continue
      file_commit "$sha" "$subject" "$body"
    done < <(printf '%s' "$commits")
  fi

  for scope in $UNLISTED_SCOPES; do
    warn "scope '${scope}' is not in release-note-areas.tsv; its changes are listed under Other"
  done
}

# Sets the caller's sha, subject and body from a git log record, and fails
# for a record without all three.
split_commit_record() {
  local record="${1#$'\n'}" rest
  [[ "$record" == *"$FIELD_SEP"* ]] || return 1
  sha="${record%%"$FIELD_SEP"*}"
  rest="${record#*"$FIELD_SEP"}"
  [[ "$rest" == *"$FIELD_SEP"* ]] || return 1
  subject="${rest%%"$FIELD_SEP"*}"
  body="${rest#*"$FIELD_SEP"}"

  sha="${sha//$'\r'/}"
  subject="${subject//$'\r'/}"
}

# Sets the caller's type, scope, bang and desc from a Conventional Commits
# subject, "type(scope)!: description". A subject in another form has no
# type, and all of it is the description.
parse_subject() {
  local subject="$1" prefix
  type=""
  scope=""
  bang=""
  desc="$subject"
  [[ "$subject" == *:* ]] || return 0
  prefix="${subject%%:*}"
  if [[ "$prefix" =~ ^([A-Za-z]+)(\((.*)\))?(!)?$ ]]; then
    type="$(printf '%s' "${BASH_REMATCH[1]}" | tr '[:upper:]' '[:lower:]')"
    # A commit with several scopes is filed under the first.
    scope="${BASH_REMATCH[3]%%,*}"
    scope="${scope// /}"
    bang="${BASH_REMATCH[4]}"
    desc="${subject#*:}"
  fi
}

# Prints the first line of a commit body's BREAKING CHANGE footer, or nothing
# when the body has none.
breaking_note_of() {
  local line note=""
  while IFS= read -r line; do
    # git puts a space before the body in this log format.
    line="${line#"${line%%[![:space:]]*}"}"
    if [[ "$line" =~ ^BREAKING[[:space:]-]CHANGE:[[:space:]]*(.+)$ ]]; then
      note="${BASH_REMATCH[1]}"
      break
    fi
  done <<<"$1"
  # A note that starts with code, such as ruston_core::Error, keeps its case;
  # one that starts with a plain word gets a capital.
  if [[ "${note%% *}" =~ ^[a-z]+$ ]]; then
    note="$(sentence "$note")"
  fi
  printf '%s' "$note"
}

# Files one commit: a new feature, fix or performance change under its area,
# the version bump nowhere, and anything else under Maintenance. A breaking
# change is also listed on its own.
file_commit() {
  local sha="$1" subject="$2" body="$3"
  local short_sha="${sha:0:8}" type scope bang desc item note area

  parse_subject "$subject"
  item="$(sentence "$desc") (${short_sha})"

  if [[ -n "$bang" || "$body" == *"BREAKING CHANGE:"* ||
    "$body" == *"BREAKING-CHANGE:"* ]]; then
    note="$(breaking_note_of "$body")"
    if [[ -n "$note" ]]; then
      BREAKING_ITEMS+="- ${note} (${short_sha})"$'\n'
    else
      BREAKING_ITEMS+="- ${item}"$'\n'
    fi
  fi

  case "$type" in
  feat | fix | perf)
    find_area "$scope"
    if [[ "$area" != "internal" ]]; then
      add_change "$type" "$scope" "$area" "$item"
      return 0
    fi
    ;;
  "")
    warn "${short_sha} has no Conventional Commits type; listed under Maintenance"
    ;;
  esac

  if [[ "$desc" =~ $VERSION_BUMP_PATTERN ]]; then
    return 0
  fi
  MAINTENANCE_ITEMS+="- ${item}"$'\n'
  MAINTENANCE_COUNT=$((MAINTENANCE_COUNT + 1))
}

# Lists a change users notice under its area, or under Other when
# release-note-areas.tsv does not list its scope, which is then noted.
add_change() {
  local type="$1" scope="$2" area="$3" item="$4"

  if [[ -z "$area" ]]; then
    area=$OTHER_AREA
    if [[ "$UNLISTED_SCOPES" != *" ${scope:-(none)} "* ]]; then
      UNLISTED_SCOPES+="${scope:-(none)} "
    fi
  fi
  case "$type" in
  feat)
    FEAT_ITEMS[area]+="- ${item}"$'\n'
    FEAT_COUNT=$((FEAT_COUNT + 1))
    ;;
  fix)
    FIX_ITEMS[area]+="- ${item}"$'\n'
    FIX_COUNT=$((FIX_COUNT + 1))
    ;;
  perf)
    PERF_ITEMS[area]+="- ${item}"$'\n'
    PERF_COUNT=$((PERF_COUNT + 1))
    ;;
  esac
  AREA_TOTALS[area]=$((AREA_TOTALS[area] + 1))
}

# --- Notes --------------------------------------------------------------------

print_notes() {
  print_overview
  print_summary
  print_breaking_changes
  print_changes
  print_maintenance
  print_download_instructions
  print_comparison
}

print_overview() {
  cat <<EOF
<!-- RELEASE OVERVIEW: write the release name and a short introduction before publishing. Everything below is generated from the commits. -->
# Release name (${TAG})

_Add a short introduction for this release._

<!-- END RELEASE OVERVIEW -->

EOF
}

# Prints how many changes of each kind users notice, and in which areas
# most of them are.
print_summary() {
  local -a summary=() busiest=()
  local line

  if ((FEAT_COUNT > 0)); then
    summary+=("$(count_phrase "$FEAT_COUNT" "new feature" "new features")")
  fi
  if ((FIX_COUNT > 0)); then
    summary+=("$(count_phrase "$FIX_COUNT" "fix" "fixes")")
  fi
  if ((PERF_COUNT > 0)); then
    summary+=("$(count_phrase "$PERF_COUNT" "performance improvement" "performance improvements")")
  fi
  ((${#summary[@]} > 0)) || return 0

  pick_busiest_areas
  line="$(join_phrases "${summary[@]}")"
  if ((${#busiest[@]} > 0)); then
    line+=", mostly in $(join_phrases "${busiest[@]}")"
  fi
  printf '%s.\n\n' "$line"
}

# Adds to the caller's busiest, in bold, up to three areas with the most
# changes, Other aside.
pick_busiest_areas() {
  local chosen=" " best index

  for _ in 1 2 3; do
    best=""
    for ((index = 0; index < OTHER_AREA; index++)); do
      [[ "$chosen" != *" ${index} "* ]] || continue
      ((AREA_TOTALS[index] > 0)) || continue
      if [[ -z "$best" ]] || ((AREA_TOTALS[index] > AREA_TOTALS[best])); then
        best=$index
      fi
    done
    [[ -n "$best" ]] || break
    chosen+="${best} "
    busiest+=("**${AREA_NAMES[$best]}**")
  done
}

print_breaking_changes() {
  if [[ -n "$BREAKING_ITEMS" ]]; then
    printf '## Breaking changes\n\n%s\n' "$BREAKING_ITEMS"
  fi
}

print_changes() {
  if ((FEAT_COUNT > 0)); then
    print_by_area "What's new" feat
  fi
  if ((FIX_COUNT > 0)); then
    print_by_area "Fixes" fix
  fi
  if ((PERF_COUNT > 0)); then
    print_by_area "Performance" perf
  fi
}

# Prints the changes of one type, feat, fix or perf, under a title, with one
# heading per area.
print_by_area() {
  local title="$1" type="$2" index items
  printf '## %s\n\n' "$title"
  for index in "${!AREA_NAMES[@]}"; do
    case "$type" in
    feat) items="${FEAT_ITEMS[$index]}" ;;
    fix) items="${FIX_ITEMS[$index]}" ;;
    perf) items="${PERF_ITEMS[$index]}" ;;
    esac
    [[ -n "$items" ]] || continue
    printf '### %s\n\n%s\n' "${AREA_NAMES[$index]}" "$items"
  done
}

print_maintenance() {
  ((MAINTENANCE_COUNT > 0)) || return 0
  cat <<EOF
<details>
<summary><strong>Maintenance</strong>: $(count_phrase "$MAINTENANCE_COUNT" "behind-the-scenes change" "behind-the-scenes changes") to code, tests, builds, releases and documentation</summary>

${MAINTENANCE_ITEMS}
</details>

EOF
}

print_download_instructions() {
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

EOF
}

print_comparison() {
  if [[ -n "$PREV_TAG" && -n "$REPO" ]]; then
    echo "**Full comparison:** [\`${PREV_TAG}...${TAG}\`](https://github.com/${REPO}/compare/${PREV_TAG}...${TAG})"
  elif [[ -n "$PREV_TAG" ]]; then
    echo "**Commit range:** \`${PREV_TAG}..${TAG}\`"
  else
    echo "**Commit range:** \`${TAG}\` (first release: every commit up to this tag)"
  fi
}

main() {
  cd "$REPO_ROOT"
  parse_arguments "$@"
  resolve_release
  load_areas
  collect_commits
  print_notes >"${OUTPUT:-/dev/stdout}"
}

main "$@"
