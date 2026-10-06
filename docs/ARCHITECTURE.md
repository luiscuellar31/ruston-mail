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
- [`ui/identity.rs`](../src/ui/identity.rs) supplies the shared logo and app ID
  to the mailbox and composer viewports. It sets the Windows process identity
  before native windows are created. The ID matches the existing macOS bundle
  and the Linux desktop filename; session and preference paths keep their
  separate storage identifiers.
- [`src/app/`](../src/app/) owns application state and decisions. `App::update`
  consumes messages and returns `Effects`; [`effect.rs`](../src/app/effect.rs)
  defines the work requested by the state machine.
- [`app/inspection.rs`](../src/app/inspection.rs) bounds best-effort conversation
  inspection to 50 distinct candidate IDs including at most four running tasks.
  Waiting IDs retain no backend or task; completion admits the next candidate.
  Only accepted page responses enqueue work, including already displayed splits;
  targets removed before admission are skipped. Folder changes, server search,
  accepted refreshes, and sign-out cancel running futures and clear waiting IDs;
  dropping the owner does too.
  Completion must match its session, folder, and unique running request before
  applying results or releasing a slot. A confirmed local read or organizing
  mutation advances the mailbox's inspection revision. Older inspection data
  cannot overwrite that mutation, but completion still releases its slot and
  admits queued work. Cancelled responses cannot affect a later visit to the
  same folder. The Proton adapter's 15-second deadline covers
  both semaphore admission and the metadata request. Overflow, timeout, and
  non-authentication failures preserve Proton's grouping.
- [`app/mailbox.rs`](../src/app/mailbox.rs) owns folder and search pagination.
  Loaded folders keep their rows, ID index, page cursor, and inspected grouping
  decisions together when cached; older pages append in date order, while
  overlapping dates are merged. Each split retains its server conversation
  metadata and member IDs, with a reverse message-to-conversation index. Refresh
  reuses the displayed members when that conversation's metadata is unchanged
  apart from read/star flags. Deeper split rows are classified by their parent's
  time and identity, so an older member cannot survive as an unrelated page row.
  Changed conversations invalidate their old members before using the new server
  row. Inspection replaces all prior members together, with fresh metadata and
  unique IDs, and can update an already displayed split without collapsing it.
  Grouping records are pruned with their loaded rows; no disk storage is added.
  Ordinary appended pages do not rebuild or scan the grouping index; refresh
  uses an ID-to-row index to move existing metadata into the new listing.
  Page responses preserve the open reader, including expansion and quote state.
  A refreshed listing can regroup a message or omit its row without proving
  deletion; the reader remains visible and loses its cache stamp if its row is
  absent. Explicit filtering, folder changes, and organizing actions retain their
  existing selection behavior. Reader scroll resets only when opening another row.
  [`ui/mailbox.rs`](../src/ui/mailbox.rs) keeps two visible row IDs, their parent
  conversation, indices, and the scroll offset for the current folder and search.
  Appends with unchanged
  indices take a constant-time check. When a merge, refresh, or inspection moves
  those rows, the next frame restores the exact message or its conversation's
  visible representative at the previous screen position, using egui's scroll
  state. A message anchor survives a temporary grouped parent. A list at offset
  zero stays at the top during startup and inspection. A new mailbox view resets
  native scroll and animation state left by a previous session. Explicit reveal
  requests take precedence, and sign-out clears the anchors. No complete listing snapshot or
  persistent scroll data is added.
  The mailbox state machine also orders read mutations by row. An explicit
  read/unread action accepted during an automatic read remains in the existing
  single action slot until that read succeeds. Its completion returns the next
  request to `App`, which creates the asynchronous effect only then. A failed or
  timed-out predecessor fails the queued action visibly instead of sending it with uncertain remote
  ordering. Automatic reads cannot start while an explicit read mutation on the
  same row is pending; other rows and action kinds keep their existing behavior.
  Stale or duplicate completions never release a queued request. Successful
  automatic reads still update local state, so a later explicit failure does
  not hide an already confirmed read.
- [`src/runtime.rs`](../src/runtime.rs) runs asynchronous effects off the UI
  thread and sends their results back as messages. UI drawing does not wait for
  network operations.
- [`src/mail/`](../src/mail/) translates between the desktop mailbox model and
  either [`ProtonMailService`](../src/mail/proton.rs), backed by `ruston-core`, or
  the local [`DemoMailbox`](../src/mail/demo.rs). Its
  [`model.rs`](../src/mail/model.rs) defines the types the desktop uses.
- [`src/settings.rs`](../src/settings.rs) stores desktop preferences;
  [`src/downloads.rs`](../src/downloads.rs) writes downloaded attachments.
  The appearance preference is applied by [`ui/theme.rs`](../src/ui/theme.rs)
  to egui visuals and the colors of custom-painted widgets. The desktop shell
  applies the choice to open windows and stores it for the next run.
  The signed-out screen offers the same appearance choice directly through
  `App::update`; password visibility stays only in transient UI state. The
  desktop dispatch clears it on submit, cancellation, or a move away from a
  password step.
- With a mailbox open, [`ui/mailbox.rs`](../src/ui/mailbox.rs) keeps the sidebar
  visible and replaces the conversation list and reader with
  [`ui/settings.rs`](../src/ui/settings.rs) when `App` shows Settings. The desktop
  UI holds preference edits until Apply and confirms whether to apply or discard
  them before leaving for a folder, composer, or sign-out. Resetting the layout
  immediately restores the default window and pane sizes in `App`; the UI
  requests the window resize and clears egui's cached pane widths once it settles.
  Settings ends with an About card containing the existing logo, package version,
  technology and author credits, license, and a Source code button. The button
  dispatches `OpenSourceCode` through `App`, which opens the package repository
  using the existing background browser effect.

For example, opening a conversation starts in
[`ui/mailbox.rs`](../src/ui/mailbox.rs). The action reaches
[`App::update`](../src/app/mod.rs), which requests a load through `MailBackend`.
The runtime executes it, `ruston-core` reads and decrypts the messages, and a
result message updates [`app/reader.rs`](../src/app/reader.rs). The desktop
adapter converts sanitized HTML into the native rich-body model in
[`mail/html.rs`](../src/mail/html.rs). It appends visible text in runs instead
of comparing styles and link targets for every character. Styled spans share
each normalized target as `Arc<str>`, keeping the body safe to send between
workers and the UI. When equal adjacent anchors merge, the stored span adopts
the current shared target so later runs compare the same allocation. The reader
creates an owned URL string when dispatching a link click.

The adapter maps the entire core `FullMessage` into `MailMessage`, preserving
the body's `Verdict` through both conversation and individual message reads.
The reader shows verified, unsigned, unverified, or invalid status on each
message card, including collapsed cards. An invalid body signature is shown in
the danger color. This verdict applies to the body alone, not its attachments.

Desktop new-mail notifications also return through the app state machine. Their
conversation result carries the session epoch and is ignored after sign-out or
when another account opens.

Sending follows the same return path: [`ui/compose.rs`](../src/ui/compose.rs)
collects input; [`app/compose.rs`](../src/app/compose.rs) manages the draft in
memory; [`mail/outgoing.rs`](../src/mail/outgoing.rs) validates outgoing data;
[`mail/proton.rs`](../src/mail/proton.rs) calls
[`ruston-core`'s send operation](../crates/ruston-core/src/mail/send.rs). The
result returns to the app as a message.

Core [`mail/send.rs`](../crates/ruston-core/src/mail/send.rs) owns outgoing file
validation, shared by the desktop backend and all SDK/CLI send paths. New local
attachments must be regular readable files, at most 32 MiB each and 128 MiB in
total. Core validates sizes before creating a draft, then checks each opened
handle and the actual bytes read against the remaining budget. A bounded read
includes only one overflow byte, so a growing file cannot bypass the limit.
Inherited forwarded attachments remain on Proton and are outside this local
file budget. Reading, encryption, and multipart construction run together on a
blocking worker. Plaintext is zeroized and dropped before constructing multipart;
the encrypted intermediate is dropped before network upload. A process-wide
semaphore admits at most two preparations or uploads across clients and sends.
The worker owns its permit, and a prepared upload retains it until completion
or cancellation, so detached blocking work cannot escape admission limits.
Started filesystem or crypto work can continue after its caller is cancelled;
it cannot resume the cancelled send pipeline or start its upload.
Desktop Proton sends rely on that core preflight inside the existing send
deadline, rather than checking files outside the timeout. The local demo backend
uses the same validator before accepting a fictional send.

If the final send request has no reliable success or explicit rejection, core
[`mail/send.rs`](../crates/ruston-core/src/mail/send.rs) returns an unconfirmed
outcome and does not try to delete the draft. The desktop leaves the
composer open and asks the sender to check Sent before trying again; the CLI
reports the draft ID with the error. A desktop send timeout is also shown as
unconfirmed because it may happen at any stage of the send pipeline.

## CLI and core boundaries

- [`cli.rs`](../crates/ruston-cli/src/cli.rs) defines commands and global
  options. [`main.rs`](../crates/ruston-cli/src/main.rs) dispatches them to
  [`commands/`](../crates/ruston-cli/src/commands/).
- Command handlers call `ruston_core::Client`. Put terminal prompts and output
  formatting in the CLI; put shared mail behavior in the core. Output helpers
  are in [`render.rs`](../crates/ruston-cli/src/render.rs). Its
  [`render/html.rs`](../crates/ruston-cli/src/render/html.rs) converts HTML
  messages to readable Markdown-style text for the default read format. It
  represents at most eight levels of quotes and lists and caps converted output
  at 1 MiB, including a truncation notice. Output is accumulated directly in a
  bounded buffer; staging text, span metadata, and link targets also have a byte
  budget, so conversion can stop earlier when staging is full. The tokenizer
  receives UTF-8 chunks and stops receiving input after truncation. List state
  stores only represented levels and counts omitted levels for correct unwinding.
  Link targets are shared across spans, and oversized destinations are rendered
  as plain labels. Visible text is appended in runs within the staging budget
  at UTF-8 boundaries. Merging spans and formatting link groups retain the
  current shared target, including equal adjacent anchors, so subsequent runs
  use pointer equality instead of rereading a long URL. Explicit HTML/raw, JSON,
  and file exports keep the full body.
- [`terminal.rs`](../crates/ruston-cli/src/terminal.rs) owns the CLI output
  boundary. Renderers, command status messages, interactive prompts, argument
  errors, and diagnostics use its shared control filter. It processes formatted
  text in one pass, removing C0/C1 controls and DEL except line feeds and tabs.
  Human-readable output is filtered even when redirected. JSON escapes controls
  without changing the decoded values. Explicit `raw` and `html` bodies retain
  their contents when redirected, but are filtered on a terminal. File exports,
  attachment writes, and the CAPTCHA helper's HTTP responses bypass this display
  layer. Core and the desktop retain their own logging setup.
- [`commands/messages.rs`](../crates/ruston-cli/src/commands/messages.rs)
  offers a numbered sender choice when `messages send` runs in a terminal with
  multiple account addresses and no `--from`. Noninteractive and JSON sends
  keep the primary-address default from core. It also detects likely HTML in
  an interactive send without `--html` and asks before changing the format;
  scripted sends keep the explicit flag behavior.
- CLI [demo mode](../crates/ruston-cli/src/demo.rs) is dispatched before live
  commands and uses the shared attachment filename policy. The desktop has its
  own demo mailbox in `src/mail/demo.rs`.
- [`mail/`](../crates/ruston-core/src/mail/) exposes the high-level `Client`
  operations, including read, send, organize, drafts, and sync. The public API
  is summarized in [`lib.rs`](../crates/ruston-core/src/lib.rs).
  HTML replies and forwards escape sender details and quoted plain text; quoted
  HTML retains the sanitized markup produced by the read path.
  Conversation reads fetch missing bodies with at most three requests in flight,
  then decrypt them in oldest-first order before returning the complete thread
  to either frontend.
- [`mail/attachments.rs`](../crates/ruston-core/src/mail/attachments.rs) also
  owns the filename policy shared by both frontends. Each frontend chooses its
  destination and creates the file exclusively. Core can pass decrypted
  attachments to a caller one at a time; the CLI uses this path for `--all`,
  fetching message metadata once and saving each file before the next download.
  The convenience API that returns all attachment bytes has a total size limit.
  Before downloading a signed attachment, core resolves the sender's public
  keys through its bounded key cache. After decryption it verifies the nonempty
  armored detached `Signature` against the plaintext. Missing usable keys,
  malformed signatures, and mismatches return `AttachmentVerificationFailed`
  before any bytes reach a caller or saving callback; discarded plaintext is
  zeroized. Unsigned attachments retain their existing behavior. All download
  methods and EML export share this check. The desktop preserves the typed
  failure through its adapter and shows that the file was not saved; the CLI
  reports the core error. Low-level crypto decryption remains separate from
  signature verification for compatibility.
- [`mail/export.rs`](../crates/ruston-core/src/mail/export.rs) owns live CLI EML
  reconstruction. It decrypts one message and then downloads each attachment
  using the same metadata, writing MIME parts sequentially to a new file. It
  encodes untrusted headers and never overwrites an existing export. The CLI
  [export handler](../crates/ruston-cli/src/commands/export.rs) chooses the
  destination and reports the result; demo mode writes fictional sample mail.
- [`api/`](../crates/ruston-core/src/api/) contains typed Proton endpoints;
  [`model/`](../crates/ruston-core/src/model/) contains API data types;
  [`transport/`](../crates/ruston-core/src/transport/) handles HTTP requests,
  authentication headers, token refresh, retries, bounded response bodies, and
  a total deadline per HTTP attempt. Ordinary requests have 60 seconds;
  attachment transfers have 180 seconds. The desktop also applies deadlines to
  complete view operations; the CLI uses the transport deadlines directly.
  The application's own safety limits are 32 MiB for ordinary responses and
  128 MiB for encrypted attachment responses. These are transport limits, not
  Proton Mail attachment quotas. Complete attachment multipart uploads also
  have a 128 MiB safety cap and reserve their final size once. The HTTP transport
  moves raw buffers into a prepared reqwest request and shares its allocation
  between attempts, including authentication, rate-limit, and verification
  retries; it does not clone the byte vector or rebuild JSON on each attempt.
  The public `Request` and `Body` types retain their existing constructors and
  representations. Requests sharing auth state coordinate refresh
  after a 401 and reuse successfully rotated tokens.
  HTTP request diagnostics borrow the prepared URL's path rather than rebuilding
  or logging the complete URL. They omit queries, origins, URL credentials and
  fragments on every attempt. Core [`error.rs`](../crates/ruston-core/src/error.rs)
  owns the shared `From<reqwest::Error>` conversion, which strips the attached
  URL while preserving the error kind and source. This covers request building,
  execution and response-body failures, so both frontends receive the same
  sanitized HTTP errors. Paths can still contain resource IDs; other logging
  targets and server-provided errors keep their existing behavior.
- [`auth/`](../crates/ruston-core/src/auth/) handles sign-in;
  [`crypto/`](../crates/ruston-core/src/crypto/) unlocks keys and handles
  message cryptography. [`html.rs`](../crates/ruston-core/src/html.rs) sanitizes
  HTML before either frontend renders it.

## Where to start

| Change or question | Start here | Follow the boundary into |
| --- | --- | --- |
| Desktop sign-in and prompts | [`src/app/auth.rs`](../src/app/auth.rs), [`src/ui/login.rs`](../src/ui/login.rs) | [`src/mail/proton.rs`](../src/mail/proton.rs), core [`auth/`](../crates/ruston-core/src/auth/), [`session/`](../crates/ruston-core/src/session/) |
| CLI sign-in and human verification | [`commands/auth.rs`](../crates/ruston-cli/src/commands/auth.rs), [`hv.rs`](../crates/ruston-cli/src/hv.rs) | Core [`auth/`](../crates/ruston-core/src/auth/), [`transport/`](../crates/ruston-core/src/transport/), [`session/`](../crates/ruston-core/src/session/) |
| Mailbox lists, search, and thread grouping | [`src/app/mailbox.rs`](../src/app/mailbox.rs), [`src/mail/threading.rs`](../src/mail/threading.rs) | [`src/mail/proton.rs`](../src/mail/proton.rs), core [`mail/read.rs`](../crates/ruston-core/src/mail/read.rs) and [`api/conversations.rs`](../crates/ruston-core/src/api/conversations.rs) |
| Reading and HTML display | [`src/app/reader.rs`](../src/app/reader.rs), [`src/ui/reader.rs`](../src/ui/reader.rs) | [`src/mail/html.rs`](../src/mail/html.rs), core [`mail/read.rs`](../crates/ruston-core/src/mail/read.rs) and [`html.rs`](../crates/ruston-core/src/html.rs) |
| CLI message reading | [`commands/messages.rs`](../crates/ruston-cli/src/commands/messages.rs) | [`render.rs`](../crates/ruston-cli/src/render.rs), [`render/html.rs`](../crates/ruston-cli/src/render/html.rs), core [`mail/read.rs`](../crates/ruston-core/src/mail/read.rs) |
| Compose, send, and outgoing attachments | [`src/app/compose.rs`](../src/app/compose.rs), [`src/mail/outgoing.rs`](../src/mail/outgoing.rs) | [`src/mail/proton.rs`](../src/mail/proton.rs), core [`mail/send.rs`](../crates/ruston-core/src/mail/send.rs), [`mail/attachments.rs`](../crates/ruston-core/src/mail/attachments.rs) |
| Downloading received attachments | Desktop [`src/app/mod.rs`](../src/app/mod.rs), [`src/downloads.rs`](../src/downloads.rs); CLI [`commands/attachments.rs`](../crates/ruston-cli/src/commands/attachments.rs) | [`src/mail/proton.rs`](../src/mail/proton.rs), core [`mail/attachments.rs`](../crates/ruston-core/src/mail/attachments.rs) |
| CLI EML export | [`commands/export.rs`](../crates/ruston-cli/src/commands/export.rs) | Core [`mail/export.rs`](../crates/ruston-core/src/mail/export.rs), [`mail/read.rs`](../crates/ruston-core/src/mail/read.rs), [`mail/attachments.rs`](../crates/ruston-core/src/mail/attachments.rs) |
| CLI syntax, behavior, or output | [`crates/ruston-cli/src/cli.rs`](../crates/ruston-cli/src/cli.rs), [`commands/`](../crates/ruston-cli/src/commands/) | [`render.rs`](../crates/ruston-cli/src/render.rs), corresponding core `mail/` operation |
| Proton request or response | [`crates/ruston-core/src/api/`](../crates/ruston-core/src/api/) | [`transport/`](../crates/ruston-core/src/transport/), [`model/`](../crates/ruston-core/src/model/), [wire tests](../crates/ruston-core/tests/api_wiremock.rs) |
| Sessions and desktop preferences | Core [`session/`](../crates/ruston-core/src/session/) | Desktop [`settings.rs`](../src/settings.rs), [privacy guide](PRIVACY.md) |
| CLI sync, watch, and local search | [`commands/sync.rs`](../crates/ruston-cli/src/commands/sync.rs), [`commands/watch.rs`](../crates/ruston-cli/src/commands/watch.rs), [`commands/search.rs`](../crates/ruston-cli/src/commands/search.rs) | Core [`mail/sync.rs`](../crates/ruston-core/src/mail/sync.rs), [`cache.rs`](../crates/ruston-core/src/cache.rs), [privacy guide](PRIVACY.md) |
| Offline demo data | [`src/mail/demo.rs`](../src/mail/demo.rs), [`crates/ruston-cli/src/demo.rs`](../crates/ruston-cli/src/demo.rs) | Each frontend's entry point |
| Desktop identity and icons | [`src/ui/identity.rs`](../src/ui/identity.rs), [`build.rs`](../build.rs) | [`packaging/linux/`](../packaging/linux/), [`assets/icons/hicolor/`](../assets/icons/hicolor/), [`assets/windows/`](../assets/windows/), [`packaging/macos/`](../packaging/macos/) |
| Build, packaging, and CI | [Workspace manifest](../Cargo.toml), [`packaging/`](../packaging/) | [CI workflows](../.github/workflows/), [development guide](DEVELOPMENT.md) |

## Local state and tests

Both frontends use the `ruston` session profile by default, so a login in one
can be resumed by the other. The CLI accepts `--profile` for separate sessions;
its former `default` session remains available with `--profile default`.
Core validates profile names as single path components before resolving session
and cache paths; the CLI applies the same validation while parsing `--profile`.
Both use core session storage: non-secret metadata in a platform config
directory and credentials in the OS keychain. The storage identifier is
`ruston-mail`, defined in core
[`lib.rs`](../crates/ruston-core/src/lib.rs). Desktop preferences live
separately in `Ruston Mail`'s config directory. Core
[`session/`](../crates/ruston-core/src/session/) stores access and refresh
tokens as one keychain entry. Session writes and token refresh use a per-profile
file lock. On a 401, another process's rotated tokens are reloaded before
refreshing again; persistence errors still reach the request. Signing out of a
shared profile signs out both frontends. Core runs local session cleanup under
that lock on a blocking worker before trying remote revocation. The 30-second
revocation deadline does not include or cancel cleanup, and a started cleanup
worker continues if its caller is cancelled. Revocation never refreshes tokens.
Cleanup checks the saved session UID under the profile lock so a pending logout
cannot erase a replacement login in the same profile.
Local storage failures reach the CLI and are shown on the desktop sign-in screen;
the desktop still discards its in-memory mailbox and draft.

The desktop keeps mailbox data in memory. The core also offers a SQLite cache
used by CLI sync and local search. Login and resume retain the authenticated
`User.ID` alongside the API base URL in `Client`; session UIDs and email addresses
are not cache identities. All client cache operations go through `open_cache`
in [`mail/sync.rs`](../crates/ruston-core/src/mail/sync.rs). It selects
`accounts/<profile>/<sha256-of-identity>.db` under the cache directory and checks
the complete account identity stored in SQLite before using the database.
An empty database is bound under an immediate transaction; an unexpected owner
or unbound data causes an error. Account bindings are never reassigned, so a
pending index or sync for one account cannot write into another account's cache.
Signing in again to the same account and server reuses its cache. Legacy
`<profile>.db` caches have no verified owner and are left untouched and unused;
run CLI sync/backfill and `index` to recreate them for the current account.
Signing out clears credentials, but retains the account's disk cache.
Indexing a folder stores decrypted message bodies in that local database;
it is not encrypted at rest
by this code. On Unix, core creates the cache directory and database with
private permissions and tightens older cache permissions before opening them;
explicit cache paths require a private parent directory. Cache writes and index
maintenance live in [`cache.rs`](../crates/ruston-core/src/cache.rs), which
removes indexed bodies when messages are deleted. FTS maintenance resolves
message IDs through the indexed `msg_fts_rows` table and targets FTS `rowid`s,
avoiding a full scan for each deletion, invalidation, or header update. Opening
an older cache migrates this relation in an immediate transaction: it renames
the existing FTS table to `msg_fts_index`, preserves its contents and rowids,
and backfills only the ID/rowid map. `msg_fts` remains a compatibility view;
its mutation triggers update FTS and the map together, including writes from
older processes. The view exposes the FTS search and rank columns so existing
readers keep working. Bulk cache resets clear FTS and the map directly in the
same transaction to avoid materializing bodies through the view.
Duplicate IDs and orphaned bodies are included in the
map, and deletion removes every matching row. A failed migration rolls back;
reopening is idempotent. Shadow tables are not accessed or modified directly.
Cached folder listings use
an index on label and message time. Opening an older cache backfills that time
in one transaction; triggers keep the index current when an older process
writes without the new column. The event stream in
[`mail/sync.rs`](../crates/ruston-core/src/mail/sync.rs)
invalidates an indexed body on a full message update. Each incremental event
batch and its cursor commit together only if the stored cursor still matches
the one used to fetch the batch; a competing sync causes a retry error without
applying stale data. Backfill and indexing capture the local cursor before each
page request and retain it for all body downloads from that page. Before writing,
SQLite's immediate transaction checks that cursor, including an uninitialized
cursor. A competing sync or resync causes a retry error without applying the
stale result. Backfill commits each page atomically; indexing commits metadata
and the body together, or metadata and body invalidation when a read fails.
No database transaction is held during a network request or decryption, and
completed pages or messages remain cached if a later result is rejected.
Initial cursor creation is also conditional. When Proton
requests a full refresh, sync pages through all message metadata into temporary
SQLite tables, then replaces the cache and event cursor in one transaction. A
failed rebuild leaves the cached offline data and cursor intact. Sync catches up
on events from the new cursor after the replacement. The full refresh clears
the local body index; run CLI `index` again to rebuild search. See
[Privacy and local data](PRIVACY.md) before changing storage or diagnostics.
Demo modes use fictional data and do not contact Proton.

CLI [`watch --folder`](../crates/ruston-cli/src/commands/watch.rs) backfills
metadata and indexes the selected folder on its first tick. Later ticks use
sync events for metadata and reindex only after creates, updates, or a full
refresh. `watch` polls the event API at the configured interval; it does not
use Server-Sent Events. Backfill and indexing failures stop the command
instead of producing a successful tick.

Most unit tests live beside the code they cover. Core HTTP contract tests are
in [`tests/api_wiremock.rs`](../crates/ruston-core/tests/api_wiremock.rs).
[`tests/live.rs`](../crates/ruston-core/tests/live.rs) requires explicit Proton
test credentials and otherwise skips its live cases. CLI parser and demo tests
are in [`crates/ruston-cli/src/main.rs`](../crates/ruston-cli/src/main.rs).
Run and check commands are in [Development](DEVELOPMENT.md).

The shared [CI workflow](../.github/workflows/ci.yml) checks Linux and macOS on
x86_64 and arm64, and Windows on x86_64 using the native MSVC toolchain. All
platforms run the same formatting, Clippy, documentation, and test gates.
Linux validates the desktop entry. Windows additionally builds the desktop
executable and checks its embedded version information. Root [`build.rs`](../build.rs)
uses `winresource` only for Windows targets to embed the ICO and Cargo version;
it leaves the CLI and core packages' binaries untouched. Committed Linux PNGs
and the Windows ICO are derived from the existing logo by
[`packaging/generate-desktop-icons.py`](../packaging/generate-desktop-icons.py).
A Windows-only identity test reads back the process AppUserModelID with the
native API without opening a window.
Native Windows Credential Manager, file dialogs, and window behavior need an
interactive session; the [development guide](DEVELOPMENT.md#windows-native-validation)
defines those manual smoke checks separately from automated test coverage.

## Keeping this map current

Review this page with every change. Update the affected section in the same
change when a package boundary, responsibility, important flow, persistence
behavior, or starting path changes. Keep the route to code and tests useful;
avoid copying API details that already live beside the implementation.
