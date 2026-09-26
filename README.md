# Ruston Mail

Ruston Mail is an unofficial desktop client for Proton Mail, written in Rust.
Its three-pane interface lets you read, organize, and send mail without an
embedded browser. The project is still in early development, so some familiar
mail features are missing.

> Ruston Mail is not affiliated with or endorsed by Proton AG.

## What works

- Sign in to Proton, including prompts for TOTP, a mailbox password, or human
  verification.
- Browse system folders, custom folders, and labels with unread counts.
- Filter loaded conversations as you type, or search the whole mailbox through
  Proton.
- Read plain-text and HTML mail, including threads and quoted replies.
- Write new messages, reply, reply to everyone, and forward.
- Attach local files to outgoing messages.
- Archive, star, mark read or unread, move to spam or trash, undo moves, and
  apply custom labels.
- Download received attachments.

## Desktop limitations

Saved drafts, security-key sign-in, and folder or label management are not
supported yet in the desktop client. It can move a message to Trash, but it
never permanently deletes mail. See the
[user guide](docs/USAGE.md) for more detail.

## Run from source

You need Rust 1.96 or newer. To run the desktop client:

```sh
git clone https://github.com/luiscuellar31/ruston-mail.git
cd ruston-mail
cargo run
```

To explore a fictional mailbox without signing in, run:

```sh
RUSTON_DEMO=1 cargo run
```

On PowerShell:

```powershell
$env:RUSTON_DEMO = "1"
cargo run
Remove-Item Env:RUSTON_DEMO
```

The demo does not contact Proton. A first build may still need to download Rust
dependencies.

## In this repository

- [`ruston-mail`](src/) is the desktop client.
- [`ruston-cli`](crates/ruston-cli/) provides terminal commands.
- [`ruston-core`](crates/ruston-core/) handles shared Proton authentication and
  mail operations for both clients.

See the [architecture map](docs/ARCHITECTURE.md) to find the code for a feature.
To inspect the CLI commands, run:

```sh
cargo run -p ruston-cli -- --help
```

## Privacy at a glance

The desktop keeps mailbox content in memory and does not save a persistent
mail cache. The CLI's optional local search index stores decrypted message
bodies in a SQLite database that Ruston Mail does not encrypt.

In the desktop reader, remote images are never fetched, and links show their
destination before opening unless you disable that prompt. See
[Privacy and local data](docs/PRIVACY.md) for details.

## Documentation

- [Using Ruston Mail](docs/USAGE.md)
- [Privacy and local data](docs/PRIVACY.md)
- [Development](docs/DEVELOPMENT.md)
- [Architecture and code map](docs/ARCHITECTURE.md)
- [Contributing](CONTRIBUTING.md)

## License and attribution

`ruston-core` is adapted from Filippo Finke's
[`protonmail-rs`](https://github.com/filippofinke/protonmail-rs).

Ruston Mail is available under the [MIT License](LICENSE).
