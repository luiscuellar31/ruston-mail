# Ruston Mail

Ruston Mail is a lightweight, unofficial Proton Mail desktop client written in
Rust. It is a clean, minimal alternative to keeping Proton Mail open in a
browser tab.

The app uses a focused three-pane interface without an embedded web view. The
project is still early: reading, organizing, and sending mail work, but some
familiar mail features do not.

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

Remote images are never fetched. Links show their real destination before they
open unless you turn that prompt off. Read more in
[Privacy and local data](docs/PRIVACY.md).

## Current limits

Saved drafts, security-key sign-in, and folder or label management are not
supported yet in the desktop client. It can move a message to Trash, but it
never permanently deletes mail. See the
[user guide](docs/USAGE.md) for more detail.

## Run from source

You need Rust 1.96 or newer. There are no packaged releases yet.

```sh
git clone https://github.com/luiscuellar31/ruston-mail.git
cd ruston-mail
cargo run
```

The workspace includes the native desktop client (`ruston-mail`), the shared
SDK (`crates/ruston-core`), and a separate command-line client
(`crates/ruston-cli`). The [architecture map](docs/ARCHITECTURE.md) shows where
each kind of change belongs. To inspect CLI commands:

```sh
cargo run -p ruston-cli -- --help
```

To look around without an account or network connection, start the fictional
mailbox:

```sh
RUSTON_DEMO=1 cargo run
```

## Documentation

- [Using Ruston Mail](docs/USAGE.md)
- [Privacy and local data](docs/PRIVACY.md)
- [Development](docs/DEVELOPMENT.md)
- [Architecture and code map](docs/ARCHITECTURE.md)
- [Contributing](CONTRIBUTING.md)

## Credits

Authentication and Proton's mail cryptography are powered by
[`ruston-core`](crates/ruston-core), originally adapted from
[`protonmail-rs`](https://github.com/filippofinke/protonmail-rs) by
Filippo Finke.

Ruston Mail is available under the [MIT License](LICENSE).
