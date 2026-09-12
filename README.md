# Ruston Mail

A desktop client for Proton Mail, written in Rust with a native interface
instead of a bundled browser. It is unofficial and not affiliated with Proton.

This is early software. It reads mail; it does not write it yet. What it does
do, it tries to do without surprises: it never loads remote images, never
opens a link without telling you where it goes, and never deletes anything.

## What it does today

You sign in with your Proton account, including a two-factor code and a
separate mailbox password if you use one. The session is remembered, so the
next launch goes straight to the mailbox.

The mailbox shows the system folders with their unread counts, loads more
conversations as you ask for them, and lets you filter what is already loaded
by sender, subject or preview. The three panes can be dragged to whatever
widths suit you.

Opening a conversation marks it read and shows its messages oldest first, with
the newest one expanded. HTML mail is drawn with real text rather than a web
view: headings, lists, quotes, preformatted blocks. Quoted replies start folded
so a long thread stays readable. Images are not downloaded — you get their
description in place, which keeps the sender from learning you opened the mail.
Plain text bodies can be selected and copied straight away; formatted ones have
a button that swaps them for selectable text.

You can archive, star, mark read or unread, and move mail to spam or trash.
After a move, a bar offers to put it back where it came from; that offer only
appears when the mail came from a real folder, since Starred and Sent are not
places anything can be returned to. Nothing is ever permanently deleted — a
move is a move.

### Keyboard

| Key | What it does |
| --- | --- |
| `j` / `↓` | Open the next conversation |
| `k` / `↑` | Open the previous one |
| `Enter` | Open the selected conversation, or retry it after a failure |
| `Esc` | Back out: the link prompt, then the search, then the reader |
| `Cmd`/`Ctrl` + `R` | Refresh the folder |
| `Cmd`/`Ctrl` + `F` | Jump to the search field |

Shortcuts stay out of the way while you are typing: a key that a focused field
takes never reaches the mailbox.

## What it cannot do yet

- Write mail. There is no compose, reply or forward.
- Open attachments. It tells you a message carries files and how many, and
  stops there.
- Show custom folders and labels. Only Proton's system folders appear.
- Search the server. Search reads the conversations already loaded, and says
  so under the field, so load more if you are looking for something older.

## Where your mail lives

Session tokens go to the operating system's keychain, never to disk in the
clear. Session metadata — which account, which profile — sits in the platform
config directory.

Mail is not stored anywhere. `proton-core` can keep a local database of message
metadata, but that belongs to its sync feature, which Ruston Mail does not use:
it asks Proton for what the open folder needs and holds it in memory. Close the
app and nothing is left behind but the session.

On macOS you may be asked to unlock the keychain every time you start a build
you compiled yourself. That is macOS, not Ruston Mail: a binary from `cargo`
has no stable code signature, so the keychain treats each rebuild as a new
application. A signed, installed app is asked once.

## Building it

Rust 1.96 or newer.

```sh
cargo run
```

Before sending a change, these should all be quiet:

```sh
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

`proton-core` is pulled from a fork with a fix for Proton's captcha flow, so
the first build will fetch it from GitHub.

### Demo mode

```sh
RUSTON_DEMO=1 cargo run
```

Opens a made-up mailbox: no network, no saved session, no account. Useful for
seeing the interface, and for working on it without touching real mail. The
fictional conversations live in memory and reset on exit. On Windows
PowerShell, use `$env:RUSTON_DEMO=1; cargo run`.

### HTTP diagnostics

```sh
RUSTON_DEBUG_HTTP=1 cargo run
```

Prints Proton request paths and response status codes to stderr, which is
enough to see which request a server rejected. Credentials, tokens, headers
and response bodies are never printed.

## Acknowledgements

Ruston Mail stands on [`proton-core`](https://github.com/filippofinke/protonmail-rs/tree/main/crates/proton-core),
part of [`protonmail-rs`](https://github.com/filippofinke/protonmail-rs) by
Filippo Finke. The hard parts — Proton's authentication and cryptography — are
that project's work.

This project is not affiliated with or endorsed by Proton AG.
