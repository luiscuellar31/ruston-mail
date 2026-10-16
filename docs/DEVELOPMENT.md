# Development

All three packages use Rust 2024 and require Rust 1.96 or newer. Their shared
version, edition, and minimum Rust version are set in the root
[`Cargo.toml`](../Cargo.toml) under `[workspace.package]`. Development happens
on macOS, and CI checks macOS, Linux, and Windows. For ownership and code paths,
see the [architecture map](ARCHITECTURE.md); for contribution rules, see
[Contributing](../CONTRIBUTING.md).

## Run the app

```sh
cargo run
```

`cargo run` selects the desktop package (`ruston-mail`) and builds its core
dependency. Run the CLI explicitly:

```sh
cargo run -p ruston-cli -- --help
```

## Demo mailbox

Demo mode uses fictional mail and never contacts Proton or saves a session:

```sh
RUSTON_DEMO=1 cargo run
```

On PowerShell:

```powershell
$env:RUSTON_DEMO = "1"
cargo run
Remove-Item Env:RUSTON_DEMO
```

Sent demo messages stay inside the process and disappear when it exits.
Demo preferences are kept in `settings-demo.json`, apart from your own.
**Exit demo** closes the fictional mailbox, but a process started in demo mode
refuses to sign in. Close it and start Ruston Mail without `RUSTON_DEMO` to use
an account.
The CLI has its own fictional demo data:

```sh
cargo run -p ruston-cli -- --demo messages list
```

## Checks

Run the main workspace checks used by CI before sending a code change:

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --locked --workspace --no-deps --all-features
cargo test --locked --workspace --all-features
```

CI uses Bash on all runners, including Git Bash on Windows, and sets
`RUSTDOCFLAGS` through the workflow's `env` field. Linux jobs run on
`ubuntu-24.04` and `ubuntu-24.04-arm`, named rather than `ubuntu-latest` so a
new Ubuntu reaches CI only through a workflow change. The Windows job uses the
`windows-2025` x86_64 runner and the native MSVC toolchain. Its cache is separate
from the Linux and macOS caches. All platforms run the same checks; a failure
on one platform does not cancel the other jobs.

Linux also validates the desktop entry with `desktop-file-validate` and its
AppStream metadata with `appstreamcli validate`. Windows
builds `ruston.exe` and checks its version information against the desktop
package; this catches missing resources without requiring a graphical session.
A Windows-only test also sets and reads back the process AppUserModelID through
the native APIs.

In PowerShell, run the same checks with the documentation environment variable
set separately:

```powershell
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
$previousRustdocFlags = $env:RUSTDOCFLAGS
try {
    $env:RUSTDOCFLAGS = "-D warnings"
    cargo doc --locked --workspace --no-deps --all-features
} finally {
    if ($null -eq $previousRustdocFlags) {
        Remove-Item Env:RUSTDOCFLAGS -ErrorAction SilentlyContinue
    } else {
        $env:RUSTDOCFLAGS = $previousRustdocFlags
    }
}
cargo test --locked --workspace --all-features
```

## Windows native validation

For mailbox layout changes, also check a 480×420 window, the smallest the app
allows, and an 820×480 one at interface scales
of 100%, 150%, 175%, and 200% in both themes. At high scales, use Folders/Mail/
Reading to navigate the compact view, scroll to the sidebar's final label,
verify Settings/sign-out stay accessible, open a conversation, and write a
draft with Cc/Bcc and attachments. Grow and shrink the window while preserving
the open message, draft, and list position. Verify that unsaved Settings changes still require
a decision when leaving. Run these checks on macOS and Linux as well; automated
egui geometry/input tests cover clipping and navigation, while native smoke
checks cover compositor and platform behavior.

CI checks compilation through Clippy and the test builds, documentation, and
automated tests. Tests use mock servers and in-memory secret stores; they do
not establish that Credential Manager, native dialogs, or window behavior work
in an interactive Windows session. Live account tests remain opt-in and need
explicit test credentials. Run the following smoke checks on Windows x86_64
before claiming native integration has been verified.

Install Rust with the `x86_64-pc-windows-msvc` toolchain and Visual Studio Build
Tools with the Desktop development with C++ workload and a Windows SDK. Build
both frontends from PowerShell:

```powershell
rustc -vV
cargo build --locked --workspace
```

### Fictional mailbox and native windows

Open the desktop demo without a Proton account. The demo uses the desktop's
normal preferences, so use a separate Windows user if existing preferences
must be preserved. Restore any previous environment value when it closes:

```powershell
$previousRustonDemo = $env:RUSTON_DEMO
try {
    $env:RUSTON_DEMO = "1"
    .\target\debug\ruston.exe
} finally {
    if ($null -eq $previousRustonDemo) {
        Remove-Item Env:RUSTON_DEMO -ErrorAction SilentlyContinue
    } else {
        $env:RUSTON_DEMO = $previousRustonDemo
    }
}
```

- Minimize, restore, maximize, and resize the main window. Verify that Windows
  decorations work and the panels remain usable at the default interface scale.
- Switch between light and dark appearance, resize the panes, close the app,
  and reopen it. Verify that applied appearance and settled sizes are restored.
- Check the UI at Windows display scaling of 100% and 150%. If two monitors are
  available, move the window between different display scales and check text,
  pointer targets, and the file dialog position.
- Set the composer placement to a separate window. Verify focus switching and
  that closing an edited composer offers the existing discard confirmation.
- Check that the main and composer windows display the Ruston logo in Alt+Tab
  and group under Ruston Mail in the taskbar. In Explorer, verify the executable
  icon and its Properties > Details: product and file description **Ruston
  Mail**, the Cargo package version, and original filename `ruston.exe`.
- Attach a readable file whose path contains spaces and non-ASCII characters.
  Verify the native picker opens, the chosen filename appears, cancellation
  leaves the composer unchanged, and removing the attachment works. A demo
  send must stay within the fictional mailbox.
- Open the Source code button in Settings and confirm the default browser
  receives the repository URL.

### Credential Manager and attachment downloads

These checks contact Proton. Use a test account and an unused CLI profile such
as `windows-smoke`; ensure `RUSTON_DEMO` is unset before running live commands.
Enter credentials through the interactive prompts rather than command-line
arguments:

```powershell
.\target\debug\ruston-cli.exe --profile windows-smoke login
.\target\debug\ruston-cli.exe --profile windows-smoke whoami
```

In Control Panel's Credential Manager, inspect **Windows Credentials** and
confirm Ruston entries for the test profile exist under the `ruston-mail`
service. Do not reveal or copy secret values. Close and reopen PowerShell,
then run `whoami` again to check session persistence. Finally:

```powershell
.\target\debug\ruston-cli.exe --profile windows-smoke logout
.\target\debug\ruston-cli.exe --profile windows-smoke whoami
```

The final command must fail because the profile has no saved session, and its
credentials must be removed from Credential Manager. Entries for other
profiles must remain intact.

For desktop sign-in and downloads, use a separate Windows user and the test
account. The desktop always uses the `ruston` profile, shared with the CLI's
default profile. Check that reopening the desktop resumes the session, and
that signing out also prevents the default CLI from resuming it. Download a
fictional test attachment twice: both copies must reach the system Downloads
folder with distinct names and unchanged contents. Use **Show in Explorer**
to verify that Explorer selects the saved file, including a filename with
spaces and non-ASCII characters.

Record the commit, Windows version, Rust host/version, display scaling, and
pass/fail result for each check in the PR or release notes. Mark unavailable
checks as **not run**. Record failures with reproduction steps; do not include
passwords, tokens, or personal message contents. Unread icon badges are not
implemented on Windows yet; notification checks are described below.

## macOS menus, window chrome, and safe closing

Use the fictional desktop mailbox for these checks; no account or outgoing
mail is needed. Record OS/build, display scaling, application zoom, and package.

- At 100%, 125%, and 175% interface zoom, verify that sign-in, mailbox, and
  Settings content stays below the traffic lights. Resize, enter/leave
  fullscreen, and move between displays with different scaling. Native metrics
  are sampled each logic pass and divided by application zoom; fullscreen
  removes the inset. A missing handle retains the previous/fallback measurement.
- Check **Settings…** in the application menu and **File > New Message** with
  their keyboard equivalents, from both windows and with the mailbox minimized.
  Preserve About, Services, Hide, and Quit. New Message must keep an existing
  draft; dirty Settings must still ask before navigating. Mail commands must be
  disabled while signed out or a confirmation/native file dialog is open.
- In both composer placements, write a draft, close the main window, repeat the
  close request, cancel, and confirm discard. Test menu Quit and Cmd+Q too.
  Escape/backdrop dismissal must preserve the draft. Repeat with unapplied
  Settings changes, choosing each Settings decision before resolving the draft.
- While a demo send is pending, close requests must be cancelled; a failed send
  must preserve its message. An untouched composer may close without a prompt.
  Confirm file dialogs block closing until they complete. Repeat window close
  checks on Linux/Windows; they share the same shell guard.

Automated tests exercise zoom/fullscreen geometry, queued command routing,
repeated close requests (including logic-only minimized input), preference
decisions, and an in-flight send racing a discard. They do not prove AppKit menu
presentation or real display geometry. Mark untested native cases as **not run**.
Direct OS termination, including Dock Quit/system shutdown, is not intercepted:
winit owns the application delegate. Draft persistence remains unimplemented.

## Native notifications and background refresh validation

Automated tests cover scheduling, backoff, stale responses, silent baselines,
unread-count-independent detection, bounded headers, activation cancellation,
and the real freedesktop D-Bus signature against an in-memory service. They do
not prove presentation, authorization, focus behavior, or Do Not Disturb on an
interactive desktop. Native checks require an authorized test account and
harmless messages; the demo deliberately sends no notifications.

- On Linux, test a notification-capable desktop in Wayland and X11 where
  available. Verify Ruston's name/icon, literal `<`, `>`, `&` in the subject,
  and a graceful unavailable status when there is no notification service.
- On Windows, verify the per-user `Ruston Mail.lnk` shortcut carries
  `com.luiscuellar.ruston-mail` and toasts appear as Ruston. Minimize the mailbox
  and verify clicking a transient toast restores/focuses it while running.
  An existing foreign shortcut must be preserved. This implementation uses an
  in-memory activation handler; reopening the app through notification history
  after exit is not implemented. Test current Windows 10/11 and system blocking.
- On macOS, build the `.app` bundle and verify allow/deny authorization, a later
  change through System Settings, and delivery without focus. A bare `cargo run`
  executable must report unavailable without invoking UserNotifications.
- Establish a silent first snapshot; then receive a new message while reading
  another elsewhere, leaving the unread count unchanged. It should still
  notify. Mark an old message unread and verify it does not notify. Check a
  reply to an existing conversation too, and test while viewing Sent/Settings.
- Verify polling about every minute with focus and every three minutes without
  it/minimized. Simulate network failures to check increasing waits, no overlap,
  and recovery. Returning soon after a background success must not duplicate it.
- Disable the option, sign out, and switch accounts while a request is pending.
  Its later result must not notify. Closing the app stops polling. Test Do Not
  Disturb and confirm delivered history follows system preferences.

Record OS/build, desktop/compositor, session type, package/commit, permission
state, and observations. Mark untested combinations as **not run**. Reference
the [freedesktop protocol](https://specifications.freedesktop.org/notification/latest/protocol.html),
[Windows desktop toast requirements](https://learn.microsoft.com/en-us/windows/win32/shell/quickstart-sending-desktop-toast),
and [Apple authorization guidance](https://developer.apple.com/documentation/usernotifications/asking-permission-to-use-notifications)
when changing adapters.

## Native file dialogs and file manager validation

Use the fictional mailbox to check **Attach files** in both composer placements
(reading pane and separate window). Verify the picker stays associated with
Ruston, cancelling leaves the draft unchanged, and repeated clicks do not open
multiple dialogs. On macOS, verify an attached sheet on the owning window; on
Windows, check ownership and focus after closing the dialog. On Linux, check
the portal under Wayland and X11 where available. eframe exposes only the root
native window, so a Linux picker opened from the separate composer is currently
associated with the mailbox window. A late result after closing/replacing the
composer must not add files to another draft.

Downloads require an explicitly authorized test account because the desktop
demo has no attachment bytes. Use harmless test files and verify:

- **Save** still writes distinct numbered copies in Downloads.
- **Save as…** opens the native selector, suggests a safe filename, and saves
  verified bytes to the chosen path. Test spaces, Unicode, `%`, `#`, and quotes.
- Cancelling **Save as…** starts no download. Selecting an existing file leaves
  its contents unchanged and reports that another filename is needed, even if
  the native selector offered replacement. A failed download creates no file.
- Where the desktop permits it, sign out while choosing a destination and
  verify the old dialog result cannot start a download in the next session.
- Reveal selects the exact saved file in Finder/Explorer. On Linux with a
  FileManager1 service it selects the file; without that interface, or when its
  request fails or times out, it opens the containing folder instead.

Record OS, desktop/compositor, session type, composer placement, commit, and
observations. Mark untested combinations as **not run**. Automated state, file
creation, and in-memory D-Bus tests cover cancellation/stale responses,
exclusive writes, URI encoding, method signatures, and service errors; they do
not prove real sheet, portal, or file-manager behavior.

## Desktop identity and icons

Native windows use `com.luiscuellar.ruston-mail`, matching the existing macOS
bundle identifier. Both the mailbox and separate composer use the existing
Ruston logo. On Windows, the process sets this AppUserModelID before opening
windows, and the build embeds a multi-resolution icon and version metadata in
`ruston.exe`. Native Windows builds require the Windows SDK resource compiler
alongside the MSVC toolchain described above; failure to embed resources fails
the build. The version comes from Cargo rather than a second version setting.

Linux's launcher filename and icon name match the Wayland app ID. On X11, the
window's class is the same ID: egui-winit names windows after the app ID, and
winit uses that name for `WM_CLASS`, so `StartupWMClass` names it too. The
desktop identity does not change session profiles or preference storage paths.

The artwork is drawn by
[`packaging/macos/generate-icon.swift`](../packaging/macos/generate-icon.swift),
which writes the source logo,
[`assets/macos/ruston-mail-1024.png`](../assets/macos/ruston-mail-1024.png), in
sRGB. The PNGs, ICNS, and ICO derived from it are committed, so ordinary builds
do not need image tools. After changing the artwork, regenerate them on macOS
with Swift, Python 3, and the system `sips` tool:

```sh
swift packaging/macos/generate-icon.swift
python3 packaging/generate-desktop-icons.py
cp assets/icons/hicolor/128x128/apps/com.luiscuellar.ruston-mail.png assets/ui/ruston-mail-128.png
rm assets/macos/ruston-mail.icns   # the next package.sh run rebuilds it
```

## Desktop installation (Linux)

From the repository root, build and install the desktop client in the standard
system locations. These commands require administrator access and replace a
previous installation at the same paths. Release builds take a few minutes:
the workspace enables link-time optimization, a single code generation unit,
and symbol stripping, which make the binaries about a third smaller. Panic
messages still name the source line, but backtraces lose function names.

```sh
cargo build --locked --release --bin ruston
sudo install -Dm755 target/release/ruston /usr/local/bin/ruston
sudo install -Dm644 packaging/linux/com.luiscuellar.ruston-mail.desktop \
  /usr/local/share/applications/com.luiscuellar.ruston-mail.desktop
sudo install -Dm644 packaging/linux/com.luiscuellar.ruston-mail.appdata.xml \
  /usr/local/share/metainfo/com.luiscuellar.ruston-mail.appdata.xml
for size in 16 24 32 48 64 128 256 512; do
  sudo install -Dm644 \
    "assets/icons/hicolor/${size}x${size}/apps/com.luiscuellar.ruston-mail.png" \
    "/usr/local/share/icons/hicolor/${size}x${size}/apps/com.luiscuellar.ruston-mail.png"
done
```

The launcher expects `ruston` on the graphical session's `PATH`. Standard
desktop sessions include `/usr/local/bin`, and `/usr/local/share` is a standard
XDG data location. If your desktop overrides those paths, adjust its `PATH` and
`XDG_DATA_DIRS` accordingly. Packagers can install the same desktop file and
icon tree under `/usr/share` with the executable under `/usr/bin`.

If available, run `desktop-file-validate` on the installed desktop file and
`sudo gtk-update-icon-cache --force --ignore-theme-index /usr/local/share/icons/hicolor`.
Desktop caches may require a new login before the launcher appears.

Validate in both a Wayland and an X11 session when available: launch Ruston
Mail from the application menu, verify its logo in the menu and Alt+Tab, and
check that the mailbox and separate composer group together in the taskbar or
dock. Repeat with light and dark desktop themes and more than one display
scale. For an account-free run, close the app and run `RUSTON_DEMO=1 ruston`.
Record the desktop environment, session type, scale, commit, and results;
mark unavailable sessions as **not run**. Automated entry and icon checks do
not establish how a particular desktop shell displays them.

## Typography and local font coverage

The existing bundled face remains the default. To compare, enable **Use system
interface font** under Settings → Interface and Apply; disabling it restores
the bundled typography. Sizes and monospace primary stay unchanged. System
language fallbacks are active in both modes, depending on installed fonts.

Automated checks preserve the bundled chain, verify rendered atlas glyphs for
available language fonts, reject missing/malformed/oversized files, check
legacy preference defaults, and exercise reader skeleton alignment with both
font modes, themes, small windows, application zoom, and native scaling.
The glyph test uses rendered allocations because egui 0.36's `has_glyph` can
report a false negative when a real glyph uses the replacement glyph's face.

Native visual checks should compare long Spanish subjects, accented names,
Cyrillic and CJK names, email addresses, Reply controls, login, Settings, and
monospace body passages. Check truncation, wrapping, and clipped ascenders at
100% and 175% zoom. On Windows test Segoe UI and installed language packs;
on Linux test Fontconfig, missing `fc-match`, and minimal/font-poor installs.
Missing fonts must preserve usability through the bundled chain. Coverage does
not establish complete RTL/bidirectional support in the current layout engine.

## Email-link handlers and activation

The desktop accepts `mailto:` URI arguments; it does not automatically become
the default handler. Choose the handler explicitly using the platform's UI.
For direct testing without a real account:

```sh
RUSTON_DEMO=1 cargo run -- 'mailto:team@example.org?subject=Hello&body=Hola%0D%0Amundo'
```

Keep that instance open and repeat with another link. Only one demo window
should exist; the second link waits until the current draft is sent or closed.
The demo and real desktop use separate activation channels. On PowerShell,
set `$env:RUSTON_DEMO = '1'` before `cargo run -- 'mailto:...'`.

- Linux: install the updated `.desktop` file and run
  `update-desktop-database` for its installation directory. It declares
  `x-scheme-handler/mailto` and passes the URL with `Exec=ruston %u`. Select
  Ruston as the email app in desktop settings, or explicitly run
  `xdg-mime default com.luiscuellar.ruston-mail.desktop x-scheme-handler/mailto`.
- macOS: use the packaged `.app`, which already declares `mailto` in its
  `Info.plist`. Select Ruston in Mail > Settings > General > Default email
  reader. Test both cold and warm reception with
  `open -a '/path/to/Ruston Mail.app' 'mailto:team@example.org?subject=Hello'`.
- Windows: keep `ruston.exe` in its installed location, then run
  `./packaging/windows/register-mailto.ps1 -ExecutablePath 'C:\path with spaces\ruston.exe'`
  from PowerShell. The script registers current-user capabilities and a quoted
  command; it neither changes the default app nor modifies `UserChoice`.
  Select Ruston for `MAILTO` in Settings > Apps > Default apps. Test a link
  using `Start-Process 'mailto:team@example.org?subject=Hello'`.

To undo default selection, choose the previous email app in the same settings.
For manual Windows uninstall, remove only Ruston's registered-application
value, `HKCU\Software\Classes\com.luiscuellar.ruston-mail.mailto`, and
`HKCU\Software\Ruston Mail\Capabilities`; do not edit `UserChoice`.

Automated tests cover URI decoding/injection, login/draft/modal guards, real
loopback forwarding, lock takeover, overflow, private Unix permissions, idle
and slow peers. Windows CI additionally exercises the registration script with an
executable path containing spaces and verifies that the default association
is unchanged. Native desktop smoke checks still require each platform:

1. Open a link with Ruston stopped, then with it running/minimized. Confirm one
   instance and the correct recipients, Unicode subject, and multi-line body.
2. Without signing in, open a link, sign in with an authorized test account,
   and check that no mail is sent. Avoid a real account for other smoke checks.
3. Receive a link while writing a draft, sending, editing Settings, choosing a
   file, or deciding whether to close. Confirm the draft/dialog remains intact
   and the request opens only after the owning UI is finished, or is dismissed.
4. Test reading-pane and separate-window composition, at 100% and high zoom.
   On Wayland, record whether the compositor permits foreground focus; Ruston
   cannot override its focus policy.

## Packaging (macOS)

Build a signed `Ruston Mail.app` bundle and draggable `.dmg` disk image for the
machine's architecture:

```sh
./packaging/macos/package.sh
./packaging/macos/verify-bundle.sh "dist/Ruston Mail.app" "$(uname -m)"
```

The script builds `ruston` and `ruston-cli` with `--locked`, so the release uses
the dependency versions in `Cargo.lock`, and for the oldest macOS that
`assets/macos/Info.plist` declares (`LSMinimumSystemVersion`). `ruston-cli` goes
in `Contents/MacOS` beside the app and is signed before the bundle. Both ad-hoc
and Developer ID signatures turn on the hardened runtime (`--options runtime`),
so neither executable loads injected libraries. `verify-bundle.sh` checks the
signature, each executable's architecture, minimum macOS and linked libraries,
and the version both report.

Options:
- `--skip-build`: Package the existing `target/release` binaries without rebuilding.
- `--target <TRIPLE>`: Build for another architecture, such as `x86_64-apple-darwin`
  on Apple Silicon, after `rustup target add <TRIPLE>`.
- `CODESIGN_IDENTITY="Developer ID Application: ..."`: Sign with a commercial certificate; defaults to ad-hoc signing (`-s -`).

Artifacts are written to `dist/`:
- `dist/Ruston Mail.app`
- `dist/ruston-mail-<version>-<arch>-UNSIGNED.dmg` (`<arch>` is `arm64` or
  `x86_64`; the `-UNSIGNED` suffix marks an ad-hoc signature)
- `dist/ruston-mail-<version>-<arch>-UNSIGNED.dmg.sha256`

`./packaging/smoke-test.sh "dist/Ruston Mail.app/Contents/MacOS/ruston"` starts
the app with an empty home directory and checks that it is still running after
10 seconds.

## Packaging (Linux AppImage)

On Linux, build `dist/ruston-mail-<version>-<arch>.AppImage` (`<arch>` is
`x86_64` or `aarch64`) with `ruston-cli` inside it:

```sh
APPIMAGE_RUNTIME=/path/to/runtime-x86_64 ./packaging/linux/appimage.sh
```

The script needs `linuxdeploy` and `appimagetool` on the `PATH` (or in
`LINUXDEPLOY` and `APPIMAGETOOL`) and an AppImage runtime file from
[type2-runtime](https://github.com/AppImage/type2-runtime/releases); without one,
`appimagetool` would download whichever runtime is newest. The release workflow
downloads pinned versions of all three and checks their digests.

linuxdeploy assembles an AppDir with both binaries, the desktop entry, the
hicolor icons, and four libraries winit loads at run time that not every system
installs: `libxkbcommon`, `libxkbcommon-x11`, `libXcursor`, and `libXi`, without
which the app stops on X11. GL, `libX11`, `libxcb`, and Wayland libraries stay
the system's, as AppImage's excludelist requires. The AppStream metadata goes to
`usr/share/metainfo`, and the 256 px icon becomes `.DirIcon`, the AppImage's
thumbnail. Before packing, the script checks that the binaries find the bundled
libraries and validates the desktop entry. On request it also runs:
- [`check-glibc.sh`](../packaging/linux/check-glibc.sh), with
  `RUSTON_MAX_GLIBC_VERSION`: no ELF file may need a newer glibc;
- the AppImage catalog's `appdir-lint.sh`, with `APPDIR_LINT`.

With `APPIMAGE_UPDATE_INFORMATION`, the AppImage carries update information and
a `.zsync` file is written beside it.

The AppImage starts the desktop app. Its [`AppRun`](../packaging/linux/AppRun)
starts `ruston-cli` instead when the first argument is `--cli` or when the
AppImage runs through a link named `ruston-cli`. Release builds run on Ubuntu
22.04, the oldest LTS in standard support and the system the AppImage catalog
tests on, so the AppImage needs glibc 2.35 or newer.

## Releases

The [release workflow](../.github/workflows/release-artifacts.yml) builds every
release artifact on each push to `main`, without publishing anything, so a
packaging break shows up before a release:

| Artifact | Built on | Checks |
|---|---|---|
| `ruston-mail-<version>-arm64-UNSIGNED.dmg` | macOS 15 runner | `verify-bundle.sh`, then started once |
| `ruston-mail-<version>-x86_64-UNSIGNED.dmg` | macOS 15 Intel runner | `verify-bundle.sh`, then started once |
| `ruston-mail-<version>-x86_64.AppImage` and its `.zsync` | Ubuntu 22.04 runner | `appdir-lint.sh`, glibc 2.35 floor, started once under Xvfb |
| `ruston-mail-<version>-aarch64.AppImage` and its `.zsync` | Ubuntu 22.04 ARM runner | as for x86_64 |

The builds use pinned, digest-checked tools and no cache, so each release comes
from the lockfile alone.

To publish a release:

1. Set the version in `[workspace.package]` of `Cargo.toml`, run
   `cargo update --workspace` for `Cargo.lock`, commit, and push to `main`.
2. Tag that commit and push the tag:

   ```sh
   git tag -a v<version> -m "Ruston Mail <version>"
   git push origin v<version>
   ```

3. The workflow checks that the tag names the workspace version and is on
   `main`, builds everything again, and creates a draft release with the files,
   `SHA256SUMS.txt`, build provenance attestations, and notes written by
   [`release-notes.sh`](../packaging/release-notes.sh) from the commits since the
   previous tag. Re-running the workflow for the tag updates the draft.
4. Edit the introduction at the top of the draft, check the files, and publish
   it. Once a release is published, the workflow refuses to replace its files.

`./packaging/release-notes.sh --tag <tag> --version <version>` prints the same
notes locally.

## HTTP diagnostics

To inspect Proton request results without logging message bodies or secrets:

```sh
RUSTON_DEBUG_HTTP=1 cargo run
```

See [Privacy and local data](PRIVACY.md#http-diagnostics) for exactly what this
prints.
