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
`RUSTDOCFLAGS` through the workflow's `env` field. The Windows job uses the
`windows-2025` x86_64 runner and the native MSVC toolchain. Its cache is separate
from the Linux and macOS caches. All platforms run the same checks; a failure
on one platform does not cancel the other jobs.

Linux also validates the desktop entry with `desktop-file-validate`. Windows
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
passwords, tokens, or personal message contents. Desktop notifications and
unread icon badges are not implemented on Windows yet.

## Desktop identity and icons

Native windows use `com.luiscuellar.ruston-mail`, matching the existing macOS
bundle identifier. Both the mailbox and separate composer use the existing
Ruston logo. On Windows, the process sets this AppUserModelID before opening
windows, and the build embeds a multi-resolution icon and version metadata in
`ruston.exe`. Native Windows builds require the Windows SDK resource compiler
alongside the MSVC toolchain described above; failure to embed resources fails
the build. The version comes from Cargo rather than a second version setting.

Linux's launcher filename and icon name match the Wayland app ID. On X11,
`StartupWMClass=ruston` matches the native window class supplied by winit for
the `ruston` executable. The desktop identity does not change session profiles
or preference storage paths.

Generated PNGs and the Windows ICO are committed, so ordinary builds do not
need image tools. To regenerate them from the existing 1024-pixel logo on
macOS, use Python 3 and the system `sips` tool:

```sh
python3 packaging/generate-desktop-icons.py
```

The source logo is [`assets/macos/ruston-mail-1024.png`](../assets/macos/ruston-mail-1024.png).
Regenerating these assets does not regenerate the existing macOS ICNS; use
[`packaging/macos/generate-icon.swift`](../packaging/macos/generate-icon.swift)
when changing that artwork.

## Desktop installation (Linux)

From the repository root, build and install the desktop client in the standard
system locations. These commands require administrator access and replace a
previous installation at the same paths:

```sh
cargo build --locked --release --bin ruston
sudo install -Dm755 target/release/ruston /usr/local/bin/ruston
sudo install -Dm644 packaging/linux/com.luiscuellar.ruston-mail.desktop \
  /usr/local/share/applications/com.luiscuellar.ruston-mail.desktop
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

## Packaging (macOS)

Build a signed `Ruston Mail.app` bundle and draggable `.dmg` disk image:

```sh
./packaging/macos/package.sh
```

Options:
- `--skip-build`: Package the existing `target/release/ruston` binary without rebuilding.
- `CODESIGN_IDENTITY="Developer ID Application: ..."`: Sign with a commercial certificate; defaults to ad-hoc signing (`-s -`).

Artifacts are written to `dist/`:
- `dist/Ruston Mail.app`
- `dist/ruston-mail-<version>-<arch>.dmg`
- `dist/ruston-mail-<version>-<arch>.dmg.sha256`

## HTTP diagnostics

To inspect Proton request results without logging message bodies or secrets:

```sh
RUSTON_DEBUG_HTTP=1 cargo run
```

See [Privacy and local data](PRIVACY.md#http-diagnostics) for exactly what this
prints.
