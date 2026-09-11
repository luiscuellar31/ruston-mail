# Ruston

Ruston is a native, cross-platform desktop client for Proton Mail, written in Rust.

## Status

Early development.

## Development

Rust 1.96 or newer is required.

```sh
cargo run
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

### Demo mode

```sh
RUSTON_DEMO=1 cargo run
```

Runs a local, fictional mailbox without connecting to Proton or reading any saved session. Demo conversations are invented, kept only in memory, and never persisted. The Spam folder always fails to load so the error state can be checked. On Windows PowerShell, use `$env:RUSTON_DEMO=1; cargo run`.

## Acknowledgements

Ruston is built on top of [`proton-core`](https://github.com/filippofinke/protonmail-rs/tree/main/crates/proton-core), part of the [`protonmail-rs`](https://github.com/filippofinke/protonmail-rs) project.

This project is not affiliated with or endorsed by Proton AG.
