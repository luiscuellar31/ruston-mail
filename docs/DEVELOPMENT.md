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
