# Ruston Mail

Ruston Mail is a native, cross-platform desktop client for Proton Mail, written in Rust.

## Status

Early development. Currently working:

- Proton sign-in, including TOTP and separate mailbox passwords, with saved-session resume
- Mailbox with system folders, unread counts, pagination, and refresh
- Local filtering of loaded conversations by participant, subject, and preview
- Conversation reader with HTML bodies shown as native text: headings, lists, quotes that fold away, and image descriptions in place of images, which are never loaded
- Links open only after confirming where they go
- Message text can be selected and copied
- Read, star, archive, trash, and spam actions

Custom folders are not shown yet.

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

Runs a local, fictional mailbox without connecting to Proton or reading any saved session. Demo conversations and mailbox actions stay in memory and reset when Ruston Mail exits. On Windows PowerShell, use `$env:RUSTON_DEMO=1; cargo run`.

### HTTP diagnostics

```sh
RUSTON_DEBUG_HTTP=1 cargo run
```

Prints Proton request paths and response status codes to stderr, which helps identify rejected requests. Credentials, tokens, headers, and response bodies are never printed.

## Acknowledgements

Ruston Mail is built on top of [`proton-core`](https://github.com/filippofinke/protonmail-rs/tree/main/crates/proton-core), part of the [`protonmail-rs`](https://github.com/filippofinke/protonmail-rs) project.

This project is not affiliated with or endorsed by Proton AG.
