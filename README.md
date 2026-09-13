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
- Archive, star, mark read or unread, move to spam or trash, undo moves, and
  apply custom labels.
- Download received attachments.

Remote images are never fetched. Links show their real destination before they
open unless you turn that prompt off. Read more in
[Privacy and local data](docs/PRIVACY.md).

## Current limits

Attaching files, saved drafts, security-key sign-in, and folder or label
management are not supported yet. Ruston Mail can move a message to Trash, but
it never permanently deletes mail. See the
[user guide](docs/USAGE.md) for more detail.

## Run from source

You need Rust 1.96 or newer. There are no packaged releases yet.

```sh
git clone https://github.com/luiscuellar31/ruston-mail.git
cd ruston-mail
cargo run
```

The first build downloads a pinned fork of `proton-core` from GitHub.

To look around without an account or network connection, start the fictional
mailbox:

```sh
RUSTON_DEMO=1 cargo run
```

## Documentation

- [Using Ruston Mail](docs/USAGE.md)
- [Privacy and local data](docs/PRIVACY.md)
- [Development](docs/DEVELOPMENT.md)

## Credits

Authentication and Proton's mail cryptography come from
[`proton-core`](https://github.com/filippofinke/protonmail-rs/tree/main/crates/proton-core),
part of [`protonmail-rs`](https://github.com/filippofinke/protonmail-rs) by
Filippo Finke.

Ruston Mail is available under the [MIT License](LICENSE).
