# Ruston Mail

A desktop client for Proton Mail, written in Rust with a native interface
instead of a bundled browser. It is unofficial and not affiliated with Proton.

This is early software. It reads mail; it does not write it yet. What it does
do, it tries to do without surprises: it never loads remote images, never
deletes anything, and tells you where a link goes before opening it — that
last one you can turn off, the other two are not up for negotiation.

## What it does today

You sign in with your Proton account, including a two-factor code and a
separate mailbox password if you use one. The session is remembered, so the
next launch goes straight to the mailbox.

The mailbox shows Proton's own folders, the folders you made and, under a
heading of their own, your labels, each with its unread count, and loads more
conversations as you ask for them. Typing in the search field narrows what is
already loaded, by sender, subject or preview; pressing Enter hands the same
words to Proton, which searches every folder and the whole history and answers
in one batch. Emptying the field brings the folder back. The three panes can be
dragged to whatever widths suit you, and they stay that way next time.

Your labels also sit above an open conversation: press one to give the thread
that label, press it again to take it away. Either way the mail stays in
whatever folder it is already in, which is what makes a label different from
a move. If you keep more labels than fit on one line, the conversation shows
the ones it already carries and a button that reaches the rest.

Opening a conversation marks it read, unless you would rather it did not, and
shows its messages oldest first, with the newest one expanded. HTML mail is
drawn with real text rather than a web view: headings, lists, quotes,
preformatted blocks. Quoted replies start folded
so a long thread stays readable. Images are not downloaded — you get their
description in place, which keeps the sender from learning you opened the mail.
Plain and formatted message text can be selected and copied directly, including
across styled spans, without giving up headings, emphasis or links.

You can archive, star, mark read or unread, and move mail to spam or trash.
After a move, a bar offers to put it back where it came from; that offer only
appears when the mail came from a real folder, since Starred and Sent are not
places anything can be returned to. Nothing is ever permanently deleted — a
move is a move.

A message that carries files lists them with their sizes, and each one can be
saved to your downloads folder. Inline parts are left out: those belong to the
body, which is why they are not offered as files.

### Settings

Four choices have behaviour behind them: whether opening mail marks it read,
whether every message in a conversation opens or only the newest, whether
quoted passages start unfolded, and whether a link is confirmed before it
opens. Each ships set the way the app behaved before it could be configured, so
an upgrade changes nothing on its own, and each reaches the conversation
already open rather than waiting for the next one. The panel also lists the
keys below, since a shortcut no one can find is a shortcut no one uses.

Everything else worth keeping — the window size, the pane widths, the folder
you left off in — is remembered on its own, with nothing to set.

### Keyboard

| Key | What it does |
| --- | --- |
| `j` / `↓` | Open the next conversation |
| `k` / `↑` | Open the previous one |
| `Enter` | Open the selected conversation, or retry a failed one. In the search field, search all mail |
| `Esc` | Back out one layer: settings, link prompt, search, reader |
| `Cmd`/`Ctrl` + `R` | Refresh the folder |
| `Cmd`/`Ctrl` + `F` | Jump to the search field |

Shortcuts stay out of the way while you are typing: a key that a focused field
takes never reaches the mailbox.

## What it cannot do yet

- Write mail. There is no compose, reply or forward.
- Open an attachment where it sits. It saves the file; opening it is your
  file manager's job.
- Page through search results. Proton answers a search in one batch, so a very
  common word may not reach as far back as you expect.
- Make, rename or delete a folder or a label. It shows and uses the ones your
  account already has.

## Where your mail lives

Session tokens go to the operating system's keychain, never to disk in the
clear. Session metadata — which account, which profile — sits in the platform
config directory, next to a `settings.json` holding the preferences above. A
damaged or hand-edited settings file costs you your window size, nothing more:
it falls back to the defaults rather than refusing to start.

Saved attachments go to your downloads folder under the name the sender chose,
reduced to a bare file name so nothing can be written elsewhere, and they never
replace a file that is already there.

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

The native interface uses `egui`/`eframe`; mail rendering stays deliberately
separate from the Proton and mailbox state layers.

```sh
cargo run
```

Before sending a change, these should all be quiet:

```sh
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

`proton-core` comes from a fork, which carries fixes for Proton's captcha and
two-factor flows and a little more of its message metadata, so the first build
fetches it from GitHub. It is pinned to one commit rather than to a branch:
the build that worked yesterday is the build you get today, and moving to a
newer one is a deliberate edit of `Cargo.toml`. That commit comes from the
fork's `dev/luiscuellar31` branch.

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
