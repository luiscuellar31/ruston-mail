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

## Acknowledgements

Ruston is built on top of [`proton-core`](https://github.com/filippofinke/protonmail-rs/tree/main/crates/proton-core), part of the [`protonmail-rs`](https://github.com/filippofinke/protonmail-rs) project.

This project is not affiliated with or endorsed by Proton AG.
