# Ruston Mail

A desktop client for Proton Mail, written in Rust with a native interface
instead of a bundled browser. Unofficial, and not affiliated with Proton.

It reads mail, and it can write it. Replies, forwards and attachments are
still to come.

Three things it will not do, so you do not have to wonder: it never loads
remote images, it never deletes anything, and it tells you where a link goes
before opening it. You can turn the link prompt off. The other two stay.

## What it does today

**Signing in.** Your Proton account, with a two-factor code and a separate
mailbox password if you use them. The session is remembered, so the next
launch goes straight to your mail.

**Finding things.** Proton's folders, your own folders, and your labels, each
with its unread count. Typing in the search box filters what is already
loaded. Pressing Enter asks Proton instead, which searches everything. Empty
the box to get the folder back.

**Reading.** Messages appear oldest first with the newest one open. HTML mail
is drawn as real text rather than in a web view, so headings, lists and quotes
survive, and you can select and copy across them. Quoted replies start folded.
Images are not downloaded; you get their description in place, which keeps the
sender from learning you opened the mail.

**Sorting.** Archive, star, mark read or unread, move to spam or trash. After
a move a bar offers to undo it. Nothing is ever permanently deleted.

**Labels.** Press a label above an open conversation to add it, press again to
remove it. The mail stays in whatever folder it is in, which is what makes a
label different from a move.

**Attachments.** Listed with their sizes, saved to your downloads folder.

**Writing.** Press Write, name as many people as you like in To, Cc and Bcc,
and send. Ruston Mail tells you if something does not look like an address
rather than quietly sending to fewer people than you meant.

The three panes resize by dragging, and stay where you put them.

## Settings

Four choices change how mail behaves:

- Whether opening mail marks it read
- Whether every message in a conversation opens, or only the newest
- Whether quoted text starts unfolded
- Whether a link is confirmed before it opens

You also choose whether a message goes out as plain text or as HTML. Either
way you write text: a tag you type is shown as you typed it, never obeyed.

Each starts out the way the app worked before it could be configured, and each
takes effect on the conversation already open.

You can also scale the whole interface, pin the folder to start in, and see
the keyboard shortcuts. Window size and pane widths are remembered on their
own.

## Keyboard

| Key | What it does |
| --- | --- |
| `j` / `↓` | Open the next conversation |
| `k` / `↑` | Open the previous one |
| `Enter` | Open the selected conversation. In the search box, search all mail |
| `Esc` | Back out one layer: settings, link prompt, search, reader |
| `Cmd`/`Ctrl` + `R` | Refresh the folder |
| `Cmd`/`Ctrl` + `F` | Jump to the search box |

While you are typing, these stay out of the way.

## What it cannot do yet

- Reply or forward. You can write a new message, not answer one in place.
- Attach a file to a message you are sending.
- Open an attachment you received. It saves the file; opening it is your file
  manager's job.
- Page through search results. Proton answers in one batch, so a very common
  word may not reach as far back as you expect.
- Create, rename or delete folders and labels.

## Where your mail lives

Session tokens go to your operating system's keychain, never to disk in the
clear. Which account and which profile go in the platform config directory,
next to a `settings.json` with your preferences. If that file is damaged you
lose your window size and nothing else: it falls back to the defaults instead
of refusing to start.

Attachments are saved to your downloads folder under the sender's name for
them, reduced to a bare file name so nothing can be written elsewhere. An
existing file is never replaced.

Mail itself is not stored anywhere. Ruston Mail asks Proton for what the open
folder needs and keeps it in memory. Close the app and only the session
remains.

On macOS, a build you compiled yourself may ask you to unlock the keychain
every time. That is macOS, not Ruston Mail: a binary from `cargo` has no
stable signature, so each rebuild looks like a new application. A signed,
installed app is asked once.

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

### How the source is laid out

Three folders, one layer each: `mail` is the mail and where it comes from,
`app` decides what happens, `ui` draws it. `app` never mentions a widget, and
`ui` never talks to Proton.

Inside a folder, `mod.rs` holds the central type and what the rest of the
program may reach. Every other file is one area of behaviour, matched across
layers: `ui/login.rs` draws what `app/auth.rs` decides. Tests sit with the
code they cover, moving to a file of their own once they exercise a whole
layer.

### About the fork

`proton-core` comes from a fork with fixes for Proton's captcha and two-factor
flows, so the first build fetches it from GitHub. It is pinned to one commit
rather than a branch, so the build that worked yesterday is the build you get
today. Moving to a newer one is a deliberate edit of `Cargo.toml`.

### Demo mode

```sh
RUSTON_DEMO=1 cargo run
```

A made-up mailbox: no network, no account, nothing saved. Good for seeing the
interface, or working on it without touching real mail. Writing works here
too — messages land in its Sent folder and go nowhere, and it takes a few per
run so a mistake cannot fill memory. On Windows PowerShell, use
`$env:RUSTON_DEMO=1; cargo run`.

### HTTP diagnostics

```sh
RUSTON_DEBUG_HTTP=1 cargo run
```

Prints Proton request paths and response codes to stderr, enough to see which
request a server rejected. Credentials, tokens, headers and bodies are never
printed.

## Acknowledgements

Ruston Mail stands on [`proton-core`](https://github.com/filippofinke/protonmail-rs/tree/main/crates/proton-core),
part of [`protonmail-rs`](https://github.com/filippofinke/protonmail-rs) by
Filippo Finke. The hard parts, Proton's authentication and cryptography, are
that project's work.

Not affiliated with or endorsed by Proton AG.
