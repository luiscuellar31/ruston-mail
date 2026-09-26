# Development

All three packages use Rust 2024 and require Rust 1.96 or newer. Their shared
version, edition, and minimum Rust version are set in the root
[`Cargo.toml`](../Cargo.toml) under `[workspace.package]`. Development happens
on macOS, and CI checks both macOS and Linux. For ownership and code paths,
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
