# Architecture

This is a map of the repository: start here when you need to locate a feature
or understand which package owns it. For build commands, see
[Development](DEVELOPMENT.md); for contribution and documentation rules, see
[Contributing](../CONTRIBUTING.md).

## Workspace at a glance

The root [Cargo workspace](../Cargo.toml) has three packages. Its default
member is the desktop package, so `cargo run` starts the desktop client.

| Package | Entry point | Responsibility |
| --- | --- | --- |
| [`ruston-mail`](../src/main.rs) | `src/main.rs` (`ruston`) | Native desktop client: UI, application state, and a mailbox adapter. |
| [`ruston-cli`](../crates/ruston-cli/src/main.rs) | `crates/ruston-cli/src/main.rs` (`ruston-cli`) | Terminal commands, prompts, and text or JSON output. |
| [`ruston-core`](../crates/ruston-core/src/lib.rs) | `crates/ruston-core/src/lib.rs` | Shared Proton client: authentication, mail operations, cryptography, transport, sessions, and local cache. |

Both frontends depend on `ruston-core`; neither depends on the other. Proton
requests go through the core library. The desktop's `src/mail/` is an adapter
for its own mailbox model, while `crates/ruston-core/src/mail/` implements the
SDK's high-level mail operations.

```text
desktop: UI -> App -> Runtime -> MailBackend -> ruston-core -> Proton API
CLI:     parser -> dispatch -> command handler -> ruston-core -> Proton API
```

## Desktop boundaries

- [`src/ui/`](../src/ui/) draws widgets with `eframe` and `egui`, gathers user
  actions, dispatches `Message`s, and applies effects that require the UI thread.
- [`src/app/`](../src/app/) owns application state and decisions. `App::update`
  consumes messages and returns `Effects`; [`effect.rs`](../src/app/effect.rs)
  defines the work requested by the state machine.
- [`src/runtime.rs`](../src/runtime.rs) runs asynchronous effects off the UI
  thread and sends their results back as messages. UI drawing does not wait for
  network operations.
- [`src/mail/`](../src/mail/) translates between the desktop mailbox model and
  either [`ProtonMailService`](../src/mail/proton.rs), backed by `ruston-core`, or
  the local [`DemoMailbox`](../src/mail/demo.rs). Its
  [`model.rs`](../src/mail/model.rs) defines the types the desktop uses.
- [`src/settings.rs`](../src/settings.rs) stores desktop preferences;
  [`src/downloads.rs`](../src/downloads.rs) writes downloaded attachments.

For example, opening a conversation starts in
[`ui/mailbox.rs`](../src/ui/mailbox.rs). The action reaches
[`App::update`](../src/app/mod.rs), which requests a load through `MailBackend`.
The runtime executes it, `ruston-core` reads and decrypts the messages, and a
result message updates [`app/reader.rs`](../src/app/reader.rs). The desktop
adapter converts sanitized HTML into the native rich-body model in
[`mail/html.rs`](../src/mail/html.rs).

Sending follows the same return path: [`ui/compose.rs`](../src/ui/compose.rs)
collects input; [`app/compose.rs`](../src/app/compose.rs) manages the draft in
memory; [`mail/outgoing.rs`](../src/mail/outgoing.rs) validates outgoing data;
[`mail/proton.rs`](../src/mail/proton.rs) calls
[`ruston-core`'s send operation](../crates/ruston-core/src/mail/send.rs). The
result returns to the app as a message.

## CLI and core boundaries

- [`cli.rs`](../crates/ruston-cli/src/cli.rs) defines commands and global
  options. [`main.rs`](../crates/ruston-cli/src/main.rs) dispatches them to
  [`commands/`](../crates/ruston-cli/src/commands/).
- Command handlers call `ruston_core::Client`. Put terminal prompts and output
  formatting in the CLI; put shared mail behavior in the core. Output helpers
  are in [`render.rs`](../crates/ruston-cli/src/render.rs).
- CLI [demo mode](../crates/ruston-cli/src/demo.rs) is dispatched before live
  commands. The desktop has its own demo mailbox in `src/mail/demo.rs`.
- [`mail/`](../crates/ruston-core/src/mail/) exposes the high-level `Client`
  operations, including read, send, organize, drafts, and sync. The public API
  is summarized in [`lib.rs`](../crates/ruston-core/src/lib.rs).
- [`api/`](../crates/ruston-core/src/api/) contains typed Proton endpoints;
  [`model/`](../crates/ruston-core/src/model/) contains API data types;
  [`transport/`](../crates/ruston-core/src/transport/) handles HTTP requests,
  authentication headers, token refresh, and retries.
- [`auth/`](../crates/ruston-core/src/auth/) handles sign-in;
  [`crypto/`](../crates/ruston-core/src/crypto/) unlocks keys and handles
  message cryptography. [`html.rs`](../crates/ruston-core/src/html.rs) sanitizes
  HTML before the desktop adapter parses it.

## Where to start

| Change or question | Start here | Follow the boundary into |
| --- | --- | --- |
| Desktop sign-in and prompts | [`src/app/auth.rs`](../src/app/auth.rs), [`src/ui/login.rs`](../src/ui/login.rs) | [`src/mail/proton.rs`](../src/mail/proton.rs), core [`auth/`](../crates/ruston-core/src/auth/), [`session/`](../crates/ruston-core/src/session/) |
| CLI sign-in and human verification | [`commands/auth.rs`](../crates/ruston-cli/src/commands/auth.rs), [`hv.rs`](../crates/ruston-cli/src/hv.rs) | Core [`auth/`](../crates/ruston-core/src/auth/), [`transport/`](../crates/ruston-core/src/transport/), [`session/`](../crates/ruston-core/src/session/) |
| Mailbox lists, search, and thread grouping | [`src/app/mailbox.rs`](../src/app/mailbox.rs), [`src/mail/threading.rs`](../src/mail/threading.rs) | [`src/mail/proton.rs`](../src/mail/proton.rs), core [`mail/read.rs`](../crates/ruston-core/src/mail/read.rs) and [`api/conversations.rs`](../crates/ruston-core/src/api/conversations.rs) |
| Reading and HTML display | [`src/app/reader.rs`](../src/app/reader.rs), [`src/ui/reader.rs`](../src/ui/reader.rs) | [`src/mail/html.rs`](../src/mail/html.rs), core [`mail/read.rs`](../crates/ruston-core/src/mail/read.rs) and [`html.rs`](../crates/ruston-core/src/html.rs) |
| Compose, send, and outgoing attachments | [`src/app/compose.rs`](../src/app/compose.rs), [`src/mail/outgoing.rs`](../src/mail/outgoing.rs) | [`src/mail/proton.rs`](../src/mail/proton.rs), core [`mail/send.rs`](../crates/ruston-core/src/mail/send.rs), [`mail/attachments.rs`](../crates/ruston-core/src/mail/attachments.rs) |
| Downloading received attachments | Desktop [`src/app/mod.rs`](../src/app/mod.rs), [`src/downloads.rs`](../src/downloads.rs); CLI [`commands/attachments.rs`](../crates/ruston-cli/src/commands/attachments.rs) | [`src/mail/proton.rs`](../src/mail/proton.rs), core [`mail/attachments.rs`](../crates/ruston-core/src/mail/attachments.rs) |
| CLI syntax, behavior, or output | [`crates/ruston-cli/src/cli.rs`](../crates/ruston-cli/src/cli.rs), [`commands/`](../crates/ruston-cli/src/commands/) | [`render.rs`](../crates/ruston-cli/src/render.rs), corresponding core `mail/` operation |
| Proton request or response | [`crates/ruston-core/src/api/`](../crates/ruston-core/src/api/) | [`transport/`](../crates/ruston-core/src/transport/), [`model/`](../crates/ruston-core/src/model/), [wire tests](../crates/ruston-core/tests/api_wiremock.rs) |
| Sessions and desktop preferences | Core [`session/`](../crates/ruston-core/src/session/) | Desktop [`settings.rs`](../src/settings.rs), [privacy guide](PRIVACY.md) |
| CLI sync and local search | [`commands/sync.rs`](../crates/ruston-cli/src/commands/sync.rs), [`commands/search.rs`](../crates/ruston-cli/src/commands/search.rs) | Core [`mail/sync.rs`](../crates/ruston-core/src/mail/sync.rs), [`cache.rs`](../crates/ruston-core/src/cache.rs), [privacy guide](PRIVACY.md) |
| Offline demo data | [`src/mail/demo.rs`](../src/mail/demo.rs), [`crates/ruston-cli/src/demo.rs`](../crates/ruston-cli/src/demo.rs) | Each frontend's entry point |
| Build, packaging, and CI | [Workspace manifest](../Cargo.toml), [`packaging/macos/`](../packaging/macos/) | [CI workflows](../.github/workflows/), [development guide](DEVELOPMENT.md) |

## Local state and tests

The desktop uses the `ruston` session profile; the CLI defaults to `default`
and accepts `--profile`. Both use core session storage: non-secret metadata in
a platform config directory and credentials in the OS keychain. The current
storage identifier is `protonmail-cli`. Desktop preferences live separately
in `Ruston Mail`'s config directory.

The desktop keeps mailbox data in memory. The core also offers a per-profile
SQLite cache used by CLI sync and local search. Indexing a folder stores
decrypted message bodies in that local database; it is not encrypted at rest
by this code. See [Privacy and local data](PRIVACY.md) before changing storage
or diagnostics. Demo modes use fictional data and do not contact Proton.

Most unit tests live beside the code they cover. Core HTTP contract tests are
in [`tests/api_wiremock.rs`](../crates/ruston-core/tests/api_wiremock.rs).
[`tests/live.rs`](../crates/ruston-core/tests/live.rs) requires explicit Proton
test credentials and otherwise skips its live cases. CLI parser and demo tests
are in [`crates/ruston-cli/src/main.rs`](../crates/ruston-cli/src/main.rs).
Run and check commands are in [Development](DEVELOPMENT.md).

## Keeping this map current

Review this page with every change. Update the affected section in the same
change when a package boundary, responsibility, important flow, persistence
behavior, or starting path changes. Keep the route to code and tests useful;
avoid copying API details that already live beside the implementation.
