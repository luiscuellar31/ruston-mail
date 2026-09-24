# Development

Ruston Mail uses Rust 2024 and requires Rust 1.96 or newer. Development happens
on macOS, and CI checks both macOS and Linux.

## Run the app

```sh
cargo run
```

The first build needs network access because Cargo fetches `proton-core` from a
pinned Git revision.

## Demo mailbox

Demo mode uses fictional mail and never contacts Proton or saves a session:

```sh
RUSTON_DEMO=1 cargo run
```

On PowerShell:

```powershell
$env:RUSTON_DEMO=1; cargo run
```

Sent demo messages stay inside the process and disappear when it exits.

## Checks

Run the same checks used by CI before sending a change:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```

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

## Source layout

The code follows three main layers:

- `src/mail/` owns mail models and the Proton and demo backends.
- `src/app/` owns state, decisions, and side effects without depending on UI
  widgets.
- `src/ui/` draws the app with `eframe` and `egui`.

`src/runtime.rs` runs asynchronous work away from the UI thread.
`src/settings.rs` persists user preferences, and `src/downloads.rs` saves
attachments safely.

Files usually match across layers. For example, `app/auth.rs` decides the login
flow and `ui/login.rs` draws it. Tests stay beside the code they cover.

## Proton dependency

The project uses a fork of
[`proton-core`](https://github.com/filippofinke/protonmail-rs/tree/main/crates/proton-core)
with fixes needed by Ruston Mail's sign-in flow. `Cargo.toml` pins an exact Git
revision, so dependency changes remain deliberate and reproducible.

## HTTP diagnostics

To inspect Proton request results without logging message bodies or secrets:

```sh
RUSTON_DEBUG_HTTP=1 cargo run
```

See [Privacy and local data](PRIVACY.md#http-diagnostics) for exactly what this
prints.
