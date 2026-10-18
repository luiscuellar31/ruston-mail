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

- [`src/main.rs`](../src/main.rs) validates desktop URI arguments before starting
  the UI. [`src/launch.rs`](../src/launch.rs) holds a per-user OS file lock and
  forwards bounded `mailto:` requests to the existing instance over an
  authenticated IPv4 loopback connection. Its worker waits in `accept` and
  serves one connection at a time, which has 200 ms to present the token,
  compared in constant time, and a total deadline, so idle connections from
  other local users cannot use up a later launch's retries. It wakes egui
  through a bounded channel; shutdown joins it before releasing the lock. The
  private activation directory contains only the lock and a
  port/token/process-ID record, never email contents. Windows keeps it in the
  local application data, which does not roam with the profile. The record lives only as
  long as the process, so it is not flushed to disk; if it cannot be written,
  the window still opens, a warning goes to standard error, and later launches
  cannot hand it their links. A stale record that cannot be removed still stops
  startup, so no link reaches a reused port. A launch that finds the lock held
  but no instance to connect to keeps trying both for three seconds: an
  instance that is closing has removed its record, and once its lock comes
  free the new launch takes over with its links.
  Demo activation uses a separate directory. Windows secondary launches grant
  foreground rights to the primary when Windows permits it.
  [`launch/macos.rs`](../src/launch/macos.rs) registers a retained `GURL` Apple
  Event target before the event loop starts, without replacing winit's delegate.
  Linux's `.desktop` file passes `%u`; Windows exposes a selectable per-user
  handler through [`register-mailto.ps1`](../packaging/windows/register-mailto.ps1),
  which [`unregister-mailto.ps1`](../packaging/windows/unregister-mailto.ps1)
  removes.
- On Windows the desktop is a window app with no console of its own
  (`windows_subsystem` in [`src/main.rs`](../src/main.rs)), so standard error
  reaches nobody unless the caller hands it down. [`src/diagnostics.rs`](../src/diagnostics.rs)
  shows a failure to start, or to hand a link to the open window, and a panic
  that ends the process in a message box. With `RUSTON_DEBUG_HTTP` it attaches
  to the console of the terminal Ruston was started from, unless standard
  error already goes to a file or pipe, and ignores that console's Ctrl+C.
- [`src/mailto.rs`](../src/mailto.rs) accepts only recipient, subject, and plain
  text body defaults. It decodes UTF-8 escapes once, preserves literal `+`,
  rejects control/header injection and ambiguous duplicate fields, and ignores
  unsupported headers/attachments. Recipients pass the composer's address
  check with ASCII domains only, whatever Settings allow for typed ones.
  URLs are limited to 16 KiB and 100 distinct recipients, since each address a
  link fills in is looked up while the draft is written. Its request fields are private, so requests are
  constructed only by this parser.
  Each external ingress (arguments, TCP, or Apple Events) validates once;
  process-local `Activate`, `Mailto(Request)`, and `Rejected` events carry the
  outcome to the shell. A mailto link that does not parse becomes `Rejected`
  at every ingress, so the window still opens or comes forward and says so;
  any other argument stops the launch. Argument URIs are retained only until
  startup/forwarding; the wire protocol still sends the original URI and
  validates it in the receiving process. A link over the size limit is
  forwarded as `mailto:%`, which every version refuses to open, because a
  receiver drops a longer frame without answering. `App` queues the parsed
  request without decoding again.
  [`app/mailto.rs`](../src/app/mailto.rs) owns up to 16 pending requests in memory,
  retaining them during sign-in. A closed session clears them. Internal links
  use this same queue after the existing link-confirmation policy. The link
  confirmation is a dialog the root window draws over everything, from
  `reader::link_dialog`, so it stays visible whatever the reader shows, and
  `Esc` answers it before Settings or a draft.
  The shell opens one request only when authenticated and no draft, Settings,
  link confirmation, native file dialog, or closing decision owns the UI. It
  shares its dialog/closing guard between opening and focusing; `App` owns
  the common authentication, draft, Settings, link-prompt, and queue readiness
  predicate, also checked when it consumes a request. Opening a request never
  replaces even an untouched draft or sends automatically. The pending
  banner offers dismissal; normal draft close/send completion allows the next
  request to open. Linux also requests native user attention, since Wayland
  ignores ordinary focus requests. Activation remains subject to compositor policy.
- [`src/ui/`](../src/ui/) draws widgets with `eframe` and `egui`, gathers user
  actions, dispatches `Message`s, and applies effects that require the UI thread.
- [`ui/graphics.rs`](../src/ui/graphics.rs) picks the renderer: OpenGL (glow)
  on macOS and Linux; on Windows DirectX 12 through wgpu, on the GPU that uses
  the least power or on WARP, Windows' software renderer, without one, and
  OpenGL when `RUSTON_RENDERER=opengl`. If DirectX 12 fails before the app is
  created, `ui::run` opens the window again with OpenGL. The activation
  instance waits in a `Cell` until the app takes it, so the retry keeps the
  single-instance lock and link hand-off.
- The Inbox unread count goes on the Dock tile on macOS and, through
  [`ui/taskbar.rs`](../src/ui/taskbar.rs), over the Windows taskbar button as
  an overlay icon: a red disc with the count, up to "99+", drawn in Segoe UI
  with GDI, and the count as its description. The shell sets it once the
  window is visible and again when the count or the Settings option changes.
- [`ui/identity.rs`](../src/ui/identity.rs) supplies the shared logo and app ID
  to the mailbox and composer viewports. It sets the Windows process identity
  before native windows are created. The ID matches the existing macOS bundle
  and the Linux desktop filename; session and preference paths keep their
  separate storage identifiers.
- [`ui/window_place.rs`](../src/ui/window_place.rs) reads the main window's
  place from winit and puts it back before the window first shows, only when
  its top edge lands on a connected screen, then maximizes it if it was. Places
  are points on macOS and pixels elsewhere; Wayland gives none. `ui/mod.rs`
  remembers the normal size and place, but nothing while the window is full
  screen or minimized, and on Windows and Linux keeps the normal size while it
  is maximized. On macOS, zoom is remembered as the larger frame it is.
- On macOS, [`ui/macos.rs`](../src/ui/macos.rs) measures the root window's
  traffic lights with eframe's `WindowChromeMetrics` on each logic pass.
  [`ui/theme.rs`](../src/ui/theme.rs) converts native points to egui points
  using application zoom and removes the inset in fullscreen. The previous
  28 native points remain a fallback when no handle/metrics are available.
  The same module extends winit's existing menus with Settings and New Message,
  queues actions for the desktop shell, and routes the existing menu Quit
  command through the root close guard. It names winit's About, Hide and Quit
  items after the bundle and adds File › Close Window, Edit, View and Window
  menus. Their items use AppKit's standard actions with no target, so the key
  window answers them: a window's own close guards, a native file dialog's text
  fields, or, at the end of each winit window's responder chain, the menu's
  target. That target turns Cut, Copy, Paste and Select All into the egui input
  their keys would have produced, for the main window in its `logic` pass and
  for the separate composer in that window's next pass. Undo and Redo stop at
  the window, whose own undo history is empty, so their keys still reach egui's
  text fields. View's Actual Size, Zoom In and Zoom Out queue zoom steps, which
  the shell applies like `Cmd`/`Ctrl` with `0`, `+`, `=` or `-` on every
  platform, pressed in the mailbox or the separate composer, whose window
  hands the step back to the shell: one of Settings' scale levels, saved at
  once and copied into an open Settings draft. egui's own keyboard zoom is off, since the shell
  reapplies the saved zoom every pass. It retains
  selector targets, validates authentication/modal availability, and restores
  owned menu and responder changes on drop; it does not replace winit's
  application or window delegates.
- [`ui/mod.rs`](../src/ui/mod.rs) handles root close requests in `logic`,
  including minimized windows. An in-flight send or native file dialog blocks
  closing. Unapplied preferences use the existing Settings exit decisions
  before proceeding to the draft check. [`ui/close.rs`](../src/ui/close.rs)
  requests explicit discard of a written message in either composer placement;
  repeated close requests never approve it. Discard/cancel reuse App messages,
  and closing rechecks current send/draft state. Signing out or leaving the
  demo asks through the same confirmation, then signs out once the message is
  discarded. A close with nothing else to ask about waits while
  `App::unsent_changes` counts mailbox actions or automatic reads Proton has
  not confirmed: `close::OnceSent` holds it, the window says why, and the close
  goes ahead once they arrive, or at once on **Close now** or a second request.
  `App` itself refuses to sign out over a written message or an
  in-flight send; during a send the sidebar's sign-out button is disabled. The
  separate composer is disabled during a root confirmation. macOS menu Quit/Cmd+Q uses this same
  path; direct OS termination (including Dock Quit) bypasses these guards.
- [`ui/dialogs.rs`](../src/ui/dialogs.rs) associates rfd file dialogs with native
  windows. The shell retains the root winit window until a dialog finishes and
  admits only one file dialog at a time. macOS and Windows use the active native
  window for a separate composer; Linux uses the root because eframe's public
  API exposes no native handle for an immediate child viewport. Dialog creation
  stays on the UI thread; results return through the runtime. Cancelled compose
  pickers return an empty result to release the dialog slot, while draft IDs
  still prevent results from modifying a closed or replaced composer. Files
  dropped on the window that shows the draft, the reading pane's or the
  composer's own, which hands them back like its zoom keys, join it through the
  same message, but only while no file dialog or confirmation is open, so a
  drop never releases a dialog's slot. winit delivers no drops on Wayland.
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
  decisions together when cached, for the eight folders left most recently;
  reopening one that was let go loads it again. Older pages append in date order, while
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
  A server search submitted while a folder page loads waits for that page, so
  the search does not turn the folder response into a stale one. A failed list
  request records what it asked for (`ListAttempt`: the folder, a next page or
  a search), so the list's Try again sends the same request again.
  A refreshed listing can regroup a message or omit its row without proving
  deletion; the reader remains visible and loses its cache stamp if its row is
  absent. Typing in the search field keeps the reader open the same way, even
  when its row no longer matches; a move of that hidden row opens nothing in its
  place. Server results, folder changes, and organizing actions retain their
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
  The reader's buttons act on the open row (`Message::ApplyAction`); a row's
  context menu names its row (`Message::ApplyRowAction`) and offers the same
  moves and flags, built by `reader::moves` and `reader::flags`. Both reach
  `App::apply_row_action`, which acts through the demo service or
  `Mailbox::start_action_on`, so a row acted on from its menu is never opened
  or read automatically.
  An action shows as soon as it is asked for. A move out of the view takes the
  row away, offers its undo and names the row to open in its place. Starred and
  labels list mail wherever it sits, so a move to another folder keeps the row
  there and the mailbox remembers where it went, for the reader's buttons and a
  later undo; Spam and Trash still take it out. When a confirmed move, such as
  a move back, belongs in the view but its row is not listed, `App` reloads the
  list. A read or
  star flag changes on every copy of the row, and the flag it replaced is kept
  by request. One action is on its way to Proton at a time: actions asked for
  meanwhile wait in order in the mailbox, so a move back follows its move, and
  each completion returns the next request to `App`, which creates the
  asynchronous effect only then. A refused action puts back the flag it
  replaced, unless a later action changes the same flag, and drops its undo
  offer; a row a refused move took out of the view comes back with the list,
  which `App` reloads. A confirmation applies its flag again, over a confirmed
  automatic read, and takes out again a row that a reload brought back while
  its move was on the way.
  The mailbox state machine also orders read mutations by row. An explicit
  read/unread action accepted during an automatic read waits at the head of
  that line until the read succeeds. A failed or timed-out predecessor fails it
  visibly instead of sending it with uncertain remote ordering, and the action
  behind it is still sent. Automatic reads cannot start while an explicit read
  mutation on the same row is pending or waiting. Stale or duplicate
  completions never release a queued request. Successful automatic reads still
  update local state, so a later explicit failure does not hide an already
  confirmed read.
- `App` asks for the account's folders and labels when a mailbox opens; the
  Proton adapter requests the two lists at the same time. A list
  that arrives renames the open custom folder or, once it is gone, falls back
  to Inbox. A failed request changes neither the open nor the remembered
  folder, and the next refresh asks for the list again.
- [`src/runtime.rs`](../src/runtime.rs) runs asynchronous effects off the UI
  thread and sends their results back as messages. UI drawing does not wait for
  network operations.
- [`src/mail/`](../src/mail/) translates between the desktop mailbox model and
  either [`ProtonMailService`](../src/mail/proton.rs), backed by `ruston-core`, or
  the local [`DemoMailbox`](../src/mail/demo.rs). Its
  [`model.rs`](../src/mail/model.rs) defines the types the desktop uses.
  Actions on a conversation row use Proton's conversation endpoints, one
  request each: a move, a star or a custom label applies to every message in
  the conversation, as in Proton's web client. A sign-in has two minutes of
  its own time: `PromptTime` sets aside how long its CAPTCHA, code and mailbox
  password prompts wait for the person, and running out is a timeout of its
  own, not a failed connection. The mailbox password of a two-password account
  is a prompt of the same sign-in, through core's `LoginPrompts`, so the
  password and code are not entered or checked twice.
- [`src/settings.rs`](../src/settings.rs) stores desktop preferences, a demo
  process in a file of its own, and moves a file it cannot read aside before
  saving defaults in its place;
  [`src/downloads.rs`](../src/downloads.rs) writes downloaded attachments.
  Incoming attachment saves have a session and unique request ID. **Save as**
  chooses a destination before downloading; cancellation starts no download,
  and stale or duplicate choices/completions cannot change a later save. Both
  save actions use the backend's signature checks before starting filesystem
  work on a blocking worker. Downloads use sanitized sender names with numeric
  collision suffixes; explicit destinations use the user's exact absolute path.
  Both use exclusive creation and never replace existing files or follow a
  final-component symlink. [`src/file_manager.rs`](../src/file_manager.rs)
  reveals saved files: Finder on macOS, FileManager1.ShowItems with an encoded
  file URI on Linux, and on Windows the shell's `SHOpenFolderAndSelectItems`,
  called on a blocking worker in a single-threaded COM apartment, which takes
  the path whole where Explorer's command line splits it at commas. Linux
  bounds connection and reply to three seconds and falls back to opening the
  containing folder with `xdg-open`.
  The appearance preference is applied by [`ui/theme.rs`](../src/ui/theme.rs)
  to egui visuals and custom-painted widget colors; egui gives each window's
  title bar the same theme when it changes. Following the system sets egui's
  `ThemePreference::System`, which tracks the theme winit reports, with the
  stored scheme as its fallback; settings keep that choice in a field of its
  own, so an older version still reads a scheme it knows. winit 0.30 reports
  no system theme on Linux, so Settings offers the choice on macOS and Windows.
  With Windows' Animation effects off, read on every pass, `theme` sets egui's
  animation time to zero and scrolls without gliding, so loading pulses and
  spinners keep the still picture of an unfocused window. macOS's and Linux's
  reduced-motion settings are not read yet. winit gives a Windows title bar
  the Windows theme again whenever Windows announces a changed setting, so
  `theme::TitleBar` repeats egui's request for the main and composer windows
  when the Windows theme changes or the window comes back to the front. A
  change announced while the window keeps focus shows until then. The shell
  stores the preference for the next run and applies
  [`ui/fonts.rs`](../src/ui/fonts.rs)'s font policy.
  Bundled typography stays primary by default. The optional system UI face is
  San Francisco/Helvetica Neue on macOS, Segoe UI/Arial on Windows, or
  Fontconfig's sans-serif family on Linux (with known local alternatives).
  Installed language fonts are appended to both families; the monospace
  primary and all existing sizes remain unchanged. Missing/invalid fonts are
  skipped. Reads are capped at 32 MiB per file and 48 MiB total for fallbacks;
  at most 32 collection faces are examined for a normal, regular-weight face.
  Accepted bytes are cached for the process lifetime as borrowed font data,
  avoiding egui 0.36's copies of owned data per context/scale. Linux Fontconfig
  discovery has a one-second timeout and bounded output. Installed fonts load
  on a background thread, once per process, so reading them (about 23 MB on
  macOS) never delays the first window: the bundled fonts apply at once, and
  the installed ones are applied on the frame after they finish loading. Tests
  load them in place for a consistent font set. Switching the optional
  preference reinstalls fonts only when the mode changes, on the next egui pass.
  No fonts are downloaded or included from the OS in the application bundle.
  The signed-out screen offers the same appearance choice directly through
  `App::update`; password visibility stays only in transient UI state. The
  desktop dispatch clears it on submit, cancellation, or a move away from a
  password step.
- [`app/layout.rs`](../src/app/layout.rs) owns the minimum width for usable
  columns and chooses compact presentation below 602 available egui points.
  [`ui/mailbox.rs`](../src/ui/mailbox.rs) then shows a single pane with Folders,
  Mail, and Reading/Message navigation. The local selection lives in `UiState`;
  `DesktopApp` follows accepted folder selections, opened conversations, search
  focus, and new drafts without adding persistent navigation state. Compact
  rendering never sends `PanelsResized`, so returning to columns preserves the
  saved ratios and egui's remembered divider widths. Explicit UI identities
  keep list and reader scroll/expansion state when their parent pane changes.
  Search controls reserve one horizontal row in both layouts, leaving the
  remaining height for visible, selectable conversations. Centered controls
  must not allocate the list's full remaining height.
  The sidebar scrolls its folders and labels independently, reserving the
  account, Settings, and sign-out actions at the bottom;
  [`ui/compose.rs`](../src/ui/compose.rs) scrolls the editor in both placements.
  eframe 0.36 connects AccessKit only to the main window, so screen readers
  find nothing in the composer's own window; Settings says so beside the
  placement, whose default is the reading pane.
- With a mailbox open, [`ui/mailbox.rs`](../src/ui/mailbox.rs) replaces the
  conversation list and reader with
  [`ui/settings.rs`](../src/ui/settings.rs) when `App` shows Settings. The desktop
  keeps the sidebar visible in the three-pane layout and gives Settings the
  full content width in compact presentation. Compact navigation leaves
  Settings through the same unsaved-change confirmation as normal navigation.
  The UI holds preference edits until Apply and confirms whether to apply or discard
  them before leaving for a folder, composer, or sign-out. Resetting the layout
  immediately restores the default window and pane sizes in `App`; the UI
  requests the window resize and clears egui's cached pane widths once it settles.
  Choices most people can leave alone, recipient encryption lookups and
  international domain names, share an Advanced card before Keyboard.
  Settings ends with an About card containing the existing logo, package version,
  technology and author credits, license, and a Source code button. The button
  dispatches `OpenSourceCode` through `App`, which opens the package repository
  using the existing background browser effect. That effect, like the CLI's
  human-verification page in [`hv.rs`](../crates/ruston-cli/src/hv.rs), hands
  the address to the system's opener without a shell: `open`, `xdg-open`, or
  on Windows `rundll32 url.dll,FileProtocolHandler` from System32, never a
  `rundll32` that Windows would first look for beside the executable.

For example, opening a conversation starts in
[`ui/mailbox.rs`](../src/ui/mailbox.rs). The action reaches
[`App::update`](../src/app/mod.rs), which requests a load through `MailBackend`.
The runtime executes it, `ruston-core` reads and decrypts the messages, and a
result message updates [`app/reader.rs`](../src/app/reader.rs). Only the newest
load is kept: opening another conversation, changing folders or ending the
session aborts the one in flight, and the reader ignores its answer. Each
message keeps Proton's unread flag, so its first unread message, or else the
newest, opens and is scrolled into view once, on the pass after the
conversation opens, never on a refresh. Only the
answer the reader waits for marks a conversation read, so a late one cannot
mark the conversation on screen. Nothing is marked while a draft covers the
reader: once a message uncovers it, `App::update` marks the conversation
opened behind the draft, if it is still the open one. The desktop
adapter converts sanitized HTML into the native rich-body model in
[`mail/html.rs`](../src/mail/html.rs). It appends visible text in runs instead
of comparing styles and link targets for every character. Styled spans share
each normalized target as `Arc<str>`, keeping the body safe to send between
workers and the UI. When equal adjacent anchors merge, the stored span adopts
the current shared target so later runs compare the same allocation. The reader
creates an owned URL string when dispatching a link click. It draws at most four
levels of quotes and indents lists at most eight levels deep, as the CLI does;
deeper content keeps the last indent, so text never leaves the column.

The adapter maps the entire core `FullMessage` into `MailMessage`, preserving
the body's `Verdict` through both conversation and individual message reads.
The reader shows body signature status beside the sender's address, including
collapsed cards, where the address follows the sender's display name and the
name is truncated first: a closed lock only for verified signatures, an open lock for
unsigned or unverified bodies, and a danger-colored warning for invalid
signatures.
Hover text and accessibility labels explain the verdict. The sender's domain
does not determine the icon. This verdict applies to the body alone, not its
attachments. A message core could not decrypt keeps its headers and reply
recipients with an `Unreadable` body: the reader shows a notice in place of the
text, and the header says the message could not be decrypted.
Loading and loaded readers share the scroll container, bounded column, heading,
and toolbar layout. Loading uses the listing's subject and message count and
disables actions; unknown message fields remain skeletons. Skeletons and the
spinner of the waiting screens (opening, signing out) share one clock in
[`ui/theme.rs`](../src/ui/theme.rs): with focus they ask for a frame every
50 ms, and without focus they hold still and ask for none. The first skeleton
card follows the initial expansion preference and reserves the real header,
sender icon inset, and reply area before its body markers. The scroll reset for
a new selection is applied during loading rather than waiting for the response.
The Labels button is a fixed-size tag icon, filled while the conversation
carries a label, so nothing reflows when the detail arrives with its labels.
Body lengths and attachment lists remain content-dependent.

Desktop new-mail detection is owned by
[`app/notifications.rs`](../src/app/notifications.rs), independently of unread
counts and the visible folder. A metadata-only adapter request reads the latest
20 Inbox messages; it neither decrypts bodies nor starts thread inspection.
The first successful snapshot after sign-in or enabling notifications is silent.
Later unread message IDs at or beyond the observed timestamp watermark can
produce one notification per check. A bounded set of IDs at that watermark
handles equal timestamps; it is process-local and not a complete event log.
Older/backdated or already-read mail and arrivals outside that recent page may
be omitted. Detection admits one request at a time, checks session plus request
ID on completion, and clears on disable/sign-out. Failed requests retain the
previous reference and release their slot; session expiry closes the mailbox.

[`ui/auto_refresh.rs`](../src/ui/auto_refresh.rs) schedules 60-second foreground
and 180-second background polls with one pending page request and backoff up to
300 seconds. egui requests timed repaints while unfocused/minimized. Busy mailbox
or send operations defer polling. Returning after 30 seconds can refresh early
if a recent success or failure backoff does not prevent it. Only accepted page
responses update timing; stale successes cannot reset backoff. Navigation releases
superseded scheduler requests even if their old response never arrives. No poller or
service survives the open desktop process or authenticated session.

[`ui/notifications.rs`](../src/ui/notifications.rs) owns native delivery and the
permission/service status shown in Settings. Revocable activation flags and
separate status allocations keep queued native work and callbacks from becoming
valid again in a later activation. Linux sends bounded D-Bus Notify requests
with the desktop identity and escaped body text. Windows uses WinRT toasts,
literal XML text nodes, an AUMID-bearing per-user Start menu shortcut, and an
in-memory activation callback to restore/focus the running mailbox. Every toast
carries one tag, so a new one replaces the last, and when notifications stop
for an account the history Windows keeps for the app ID is cleared on a thread
of its own. Windows reports no notification setting for the app until its
first toast shows, so only a reported setting denies one. COM and file
operations stay on a blocking worker. macOS uses UserNotifications authorization
only with the matching `.app` bundle; it rereads authorization for each arrival
and submits local notifications through completion blocks. Native errors update
status without interrupting mail operations. Already delivered OS notifications
are outside pending-work cancellation. Demo mode never requests OS authorization
or delivers notifications.

Sending follows the same return path: [`ui/compose.rs`](../src/ui/compose.rs)
collects input; [`app/compose.rs`](../src/app/compose.rs) manages the draft in
memory; [`mail/outgoing.rs`](../src/mail/outgoing.rs) validates outgoing data;
[`mail/proton.rs`](../src/mail/proton.rs) calls
[`ruston-core`'s send operation](../crates/ruston-core/src/mail/send.rs). The
result returns to the app as a message.

The composer's protection notice takes the same path. When a recipient field
loses focus, [`app/compose.rs`](../src/app/compose.rs) asks the backend about
addresses it has not looked up for this draft;
[`mail/proton.rs`](../src/mail/proton.rs) calls core's `recipient_protection`,
which classifies the key lookup the way sending picks a package. Answers carry
the session epoch and draft ID, and an address removed meanwhile is ignored.
The `check_recipient_protection` preference in
[`settings.rs`](../src/settings.rs) turns the lookup off; turning it off also
forgets the open draft's answers, so no notice remains.

[`mail/outgoing.rs`](../src/mail/outgoing.rs) reads recipient fields under a
`Domains` rule that takes ASCII domains only, unless the
`accept_international_domains` preference is on. Then a domain in another
script is converted to its IDNA `xn--` form with `url::Host::parse`, after
refusing percent escapes and results that are not domain names. The address is
looked up and sent in that form, and each typed domain is kept beside it for
the composer's warning. The draft holds the rule from Settings when it opens
and follows a change at once, forgetting lookups for addresses that change
form. Email links always use the ASCII rule. The draft reads its recipient
fields when one of them or the rule changes and keeps the result, so the checks
the window repeats on every frame neither parse addresses nor copy the body or
attachments; only Send builds the outgoing message. Duplicates are found with
hash sets, so a long recipient list reads in one pass. The composer reads each
attachment's size once, when the attachment first shows, and keeps it with
the draft's editor memory: asking the file system on every frame would wait
on a network share that stopped answering.

Core [`mail/send.rs`](../crates/ruston-core/src/mail/send.rs) owns outgoing file
validation, shared by the desktop backend and all SDK/CLI send paths. New local
attachments must be regular readable files, at most 32 MiB each and 128 MiB in
total. Core validates sizes before creating a draft, then checks each opened
handle and the actual bytes read against the remaining budget. A bounded read
includes only one overflow byte, so a growing file cannot bypass the limit.
Inherited forwarded attachments remain on Proton and are outside this local
file budget; their session keys are recovered from the draft's key packets and
wrapped for every recipient like new uploads, and a draft attachment without a
key packet stops the send. Reading, encryption, and multipart construction run together on a
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

Each password-protected (encrypted-outside) recipient gets its own SRP verifier,
token, and body and attachment keys wrapped with the message password. Each of
those takes a costly key derivation, so a recipient's entry is built on the
blocking pool, one recipient at a time.

If the final send request has no reliable success or explicit rejection, core
[`mail/send.rs`](../crates/ruston-core/src/mail/send.rs) returns an unconfirmed
outcome and does not try to delete the draft. The desktop leaves the
composer open and asks the sender to check Sent before trying again; the CLI
reports the draft ID with the error. A desktop send timeout is also shown as
unconfirmed because it may happen at any stage of the send pipeline. That
deadline adds core's 180-second transfer deadline for every new attachment, so
a stalled upload ends with core's definite error, which removes the draft,
instead of an unknown outcome.

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
  text in one pass, removing C0/C1 controls and DEL except line feeds and tabs,
  and the invisible direction and zero-width marks that core's
  `is_invisible_mark` names. Human-readable output is filtered even when
  redirected. JSON escapes these characters without changing the decoded
  values, and goes out while it serializes, so a large thread is not held a
  second time as text. Explicit `raw` and `html` bodies retain
  their contents when redirected, but are filtered on a terminal. File exports,
  attachment writes, and the CAPTCHA helper's HTTP responses bypass this display
  layer. Core and the desktop retain their own logging setup. Standard output
  goes through one writer: when its reader closes the pipe early, as `| head`
  does, the command exits quietly with status 0, and any other write failure
  ends it with status 1. Diagnostics on standard error are best effort. A
  failed command prints its error followed by each underlying cause, so a
  failed request says whether DNS, the connection, TLS or a timeout failed.
  `--help` ends with the exit statuses; 2 covers both usage errors and a
  missing, expired or refused session. Arguments that can only fail, such as
  `attachments download` without an ID or `--all`, are refused while parsing,
  before a session is resumed.
- [`commands/messages.rs`](../crates/ruston-cli/src/commands/messages.rs)
  offers a numbered sender choice when `messages send` runs in a terminal with
  multiple account addresses and no `--from`. Noninteractive and JSON sends
  keep the primary-address default from core. It also detects likely HTML in
  an interactive send without `--html` and asks before changing the format;
  scripted sends keep the explicit flag behavior. Permanent deletes ask first:
  `messages delete` lists the subject and sender each reference matched,
  taken from the search for search text and fetched only for an exact ID,
  `drafts delete` lists each draft, and `messages empty` accepts only Trash or
  Spam. Without a terminal to ask on, all three need `--yes`. Core's
  `delete_draft` refuses the whole request if any ID is not a draft.
  `save_draft` and `update_draft` upload attached files to the draft once its
  content is written, and refuse a delivery time or an expiration, which only
  sending applies; the draft commands do not offer those, nor a message
  password. A new draft whose attachments fail to upload is deleted again.
  `update_draft` refuses any other message and keeps the draft's address
  unless another is named; a draft with attachments, which are keyed to that
  address, cannot change it.
- CLI [demo mode](../crates/ruston-cli/src/demo.rs) is dispatched before live
  commands and uses the shared attachment filename policy. The desktop has its
  own demo mailbox in `src/mail/demo.rs`.
- [`mail/`](../crates/ruston-core/src/mail/) exposes the high-level `Client`
  operations, including read, send, organize, drafts, and sync. The public API
  is summarized in [`lib.rs`](../crates/ruston-core/src/lib.rs).
  HTML replies and forwards escape sender details and quoted plain text; quoted
  HTML retains the sanitized markup produced by the read path. Plain-text ones
  quote an HTML original as its text. An answer's subject gains "Re: " or
  "Fw: " unless it already starts with "Re:", "Fw:" or "Fwd:", and the draft
  tells Proton whether it replies, replies to all, or forwards. The original is
  fetched once and decrypted for its text alone, so quoting looks up no sender
  keys. One function,
  `reply_recipients`, decides where `Client::reply` sends an answer; mail the
  account sent, without also receiving it, is answered to its recipients
  rather than to the account itself. Read messages carry their Reply-To
  addresses so the desktop composer can show the same recipients before
  sending, and `reply_redirected` flags a Reply-To that bypasses the sender.
  Conversation reads fetch missing bodies with at most three requests in flight,
  then decrypt them in oldest-first order before returning the complete thread
  to either frontend. The keys of every distinct sender are asked for together
  first, so each sender is looked up once and no body waits for another's
  lookup. Decrypting, MIME parsing and sanitizing run on the blocking pool, up
  to three bodies at a time while the next ones download. A decrypted MIME body
  reads as every part of its HTML version, in order, or of its text version
  when it has none, so text around an image is not cut after the first part;
  attachment
  decryption and signature checks run there too. Both frontends read threads
  through `read_conversation_each`: a message that downloads but cannot be
  opened comes back as `ConversationMessage::Unreadable`, with its headers,
  and the rest still open. A failed request still fails the whole read, and
  `read_conversation` keeps failing on any message it cannot open.
- [`mail/attachments.rs`](../crates/ruston-core/src/mail/attachments.rs) also
  owns the filename policy shared by both frontends. Each frontend chooses its
  destination and creates the file exclusively, then marks it with core's
  `mark_downloaded`: on Windows a `Zone.Identifier` stream naming the Internet
  zone, best effort, so SmartScreen and Office's Protected View apply; nothing
  elsewhere. Core can pass decrypted
  attachments to a caller one at a time; the CLI uses this path for `--all`,
  fetching message metadata once and saving each file before the next download.
  The convenience API that returns all attachment bytes has a total size limit.
  Before downloading a signed attachment, core resolves the sender's public
  keys through its bounded key cache, whose entries expire after ten minutes.
  Keys Proton marks as compromised are left out, for attachments and message
  bodies alike. A failed lookup returns its own error, and a body read during
  one is shown unverified. After decryption it verifies the nonempty
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
  using the same metadata, writing MIME parts sequentially to a hidden
  temporary file. A complete file is hard-linked to its final name, which
  fails rather than replace an existing export; file systems without hard
  links rename it once the name is checked free. A message whose file already
  exists counts as exported, so a rerun resumes. `export_folder_report` lists
  the messages it skipped because they, or their attachments, could not be
  opened; a failed request or write stops the export. The CLI
  [export handler](../crates/ruston-cli/src/commands/export.rs) chooses the
  destination and reports the result, exiting with an error when a message was
  skipped; demo mode writes fictional sample mail.
- [`api/`](../crates/ruston-core/src/api/) contains typed Proton endpoints.
  Actions on several messages or conversations read the response Proton gives
  for each ID (`Code` 1001 with `Responses`), and fail with an API error that
  names the IDs it refused, instead of reporting all of them done;
  [`model/`](../crates/ruston-core/src/model/) contains API data types;
  [`transport/`](../crates/ruston-core/src/transport/) handles HTTP requests,
  authentication headers, token refresh, retries, bounded response bodies, and
  a total deadline per HTTP attempt. A response is judged by its envelope
  (`Code`, `Error`, `Details`) without building the rest of the body, which the
  caller then decodes once; bodies a caller does not need are skipped, and an
  API error keeps the first 4 KiB of its body. Ordinary requests have 60 seconds;
  attachment transfers have 180 seconds. Opening a connection has its own
  10-second limit within that deadline. Responses are requested
  gzip-compressed. TLS prefers the X25519MLKEM768 post-quantum hybrid key
  exchange: core declares reqwest's rustls only to turn on its
  `prefer-post-quantum` feature. Requests stay at the API's origin: a redirect
  to another scheme, host or port fails, at most ten are followed, and no
  `Referer` is sent. Plain HTTP is refused unless the API address is
  `localhost` or a loopback address. Sign-in and resume build the client with
  `HttpClient::try_new`, so a system without root certificates gets an error
  instead of a panic; `HttpClient::new` keeps its panicking signature. The client follows the HTTP and HTTPS proxies configured
  on macOS and Windows, and the proxy environment variables on every
  platform. The desktop also applies deadlines to
  complete view operations, and those that move attachments (saving one,
  sending new ones) include the transfer deadline; opening a conversation
  allows a request's time for it and one for each message its row lists. The
  CLI uses the transport deadlines directly.
  The application's own safety limits are 32 MiB for ordinary responses and
  64 MiB for encrypted attachment responses. These are transport limits, not
  Proton Mail attachment quotas. The attachment limit leaves room for OpenPGP
  overhead above the 50 MiB plaintext that decryption accepts (proton-rpgp's
  reading limit); core refuses an attachment Proton reports above it before
  downloading, and reports both cases as `AttachmentTooLarge`. They count decompressed bytes, so a
  compressed response cannot exceed them. A response that states its length
  within the limit has that length reserved before it is read; a compressed one
  states none and grows as it arrives. Complete attachment multipart uploads also
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
- [`auth/`](../crates/ruston-core/src/auth/) handles sign-in. `Client::login_with_prompts`
  asks through `LoginPrompts` for what the account needs and the options do not
  carry: a TOTP code once any human verification is done, and a separate mailbox
  password once the account password is accepted, so neither repeats the other.
  Once Proton has accepted the password, `auth::or_end_session` ends the session
  if the second factor, unlocking the keys or saving the session then fails.
  [`crypto/`](../crates/ruston-core/src/crypto/) unlocks keys and handles
  message cryptography. Each address key is unlocked once per session and kept
  as an unlocked export in zeroized memory, so later operations skip its
  passphrase derivation; the armored key and passphrase stay as a fallback.
  An address's keys are kept primary first, then those Proton has not marked
  compromised or obsolete, whatever order the API lists them in; messages are
  signed and drafts encrypted with the first. [`html.rs`](../crates/ruston-core/src/html.rs) sanitizes
  HTML before either frontend renders it. Links keep only `http`, `https` and
  `mailto` URLs and relative ones are dropped, for exports and quoted replies
  as much as for the readers; the sanitizer is built once. Its crate-private
  `to_text` turns an outgoing HTML body into plain text for PGP/Inline
  recipients, who can only receive text: `send.rs` encrypts that version
  under a session key of its own for their package alone. It is a third HTML
  tokenizer beside the CLI's and the desktop's converters.

## Where to start

| Change or question | Start here | Follow the boundary into |
| --- | --- | --- |
| Desktop sign-in and prompts | [`src/app/auth.rs`](../src/app/auth.rs), [`src/ui/login.rs`](../src/ui/login.rs) | [`src/mail/proton.rs`](../src/mail/proton.rs), core [`auth/`](../crates/ruston-core/src/auth/), [`session/`](../crates/ruston-core/src/session/) |
| CLI sign-in and human verification | [`commands/auth.rs`](../crates/ruston-cli/src/commands/auth.rs), [`hv.rs`](../crates/ruston-cli/src/hv.rs) | Core [`auth/`](../crates/ruston-core/src/auth/), [`transport/`](../crates/ruston-core/src/transport/), [`session/`](../crates/ruston-core/src/session/) |
| Mailbox lists, search, and thread grouping | [`src/app/mailbox.rs`](../src/app/mailbox.rs), [`src/mail/threading.rs`](../src/mail/threading.rs) | [`src/mail/proton.rs`](../src/mail/proton.rs), core [`mail/read.rs`](../crates/ruston-core/src/mail/read.rs) and [`api/conversations.rs`](../crates/ruston-core/src/api/conversations.rs) |
| Reading and HTML display | [`src/app/reader.rs`](../src/app/reader.rs), [`src/ui/reader.rs`](../src/ui/reader.rs) | [`src/mail/html.rs`](../src/mail/html.rs), core [`mail/read.rs`](../crates/ruston-core/src/mail/read.rs) and [`html.rs`](../crates/ruston-core/src/html.rs) |
| Desktop email links and instance activation | [`src/mailto.rs`](../src/mailto.rs), [`src/launch.rs`](../src/launch.rs), [`src/app/mailto.rs`](../src/app/mailto.rs) | [`src/launch/macos.rs`](../src/launch/macos.rs), [`src/ui/mod.rs`](../src/ui/mod.rs), platform packaging |
| CLI message reading | [`commands/messages.rs`](../crates/ruston-cli/src/commands/messages.rs) | [`render.rs`](../crates/ruston-cli/src/render.rs), [`render/html.rs`](../crates/ruston-cli/src/render/html.rs), core [`mail/read.rs`](../crates/ruston-core/src/mail/read.rs) |
| Compose, send, and outgoing attachments | [`src/app/compose.rs`](../src/app/compose.rs), [`src/mail/outgoing.rs`](../src/mail/outgoing.rs) | [`src/mail/proton.rs`](../src/mail/proton.rs), core [`mail/send.rs`](../crates/ruston-core/src/mail/send.rs), [`mail/attachments.rs`](../crates/ruston-core/src/mail/attachments.rs) |
| Downloading received attachments | Desktop [`src/app/mod.rs`](../src/app/mod.rs), [`src/downloads.rs`](../src/downloads.rs); CLI [`commands/attachments.rs`](../crates/ruston-cli/src/commands/attachments.rs) | [`src/mail/proton.rs`](../src/mail/proton.rs), core [`mail/attachments.rs`](../crates/ruston-core/src/mail/attachments.rs) |
| Native file dialogs and revealing saved files | [`src/ui/dialogs.rs`](../src/ui/dialogs.rs), [`src/file_manager.rs`](../src/file_manager.rs) | [`src/ui/mod.rs`](../src/ui/mod.rs), [`src/app/effect.rs`](../src/app/effect.rs) |
| CLI EML export | [`commands/export.rs`](../crates/ruston-cli/src/commands/export.rs) | Core [`mail/export.rs`](../crates/ruston-core/src/mail/export.rs), [`mail/read.rs`](../crates/ruston-core/src/mail/read.rs), [`mail/attachments.rs`](../crates/ruston-core/src/mail/attachments.rs) |
| CLI syntax, behavior, or output | [`crates/ruston-cli/src/cli.rs`](../crates/ruston-cli/src/cli.rs), [`commands/`](../crates/ruston-cli/src/commands/) | [`render.rs`](../crates/ruston-cli/src/render.rs), corresponding core `mail/` operation |
| Proton request or response | [`crates/ruston-core/src/api/`](../crates/ruston-core/src/api/) | [`transport/`](../crates/ruston-core/src/transport/), [`model/`](../crates/ruston-core/src/model/), [wire tests](../crates/ruston-core/tests/api_wiremock.rs) |
| Sessions and desktop preferences | Core [`session/`](../crates/ruston-core/src/session/) | Desktop [`settings.rs`](../src/settings.rs), [privacy guide](PRIVACY.md) |
| CLI sync, watch, and local search | [`commands/sync.rs`](../crates/ruston-cli/src/commands/sync.rs), [`commands/watch.rs`](../crates/ruston-cli/src/commands/watch.rs), [`commands/search.rs`](../crates/ruston-cli/src/commands/search.rs) | Core [`mail/sync.rs`](../crates/ruston-core/src/mail/sync.rs), [`cache.rs`](../crates/ruston-core/src/cache.rs), [privacy guide](PRIVACY.md) |
| Offline demo data | [`src/mail/demo.rs`](../src/mail/demo.rs), [`crates/ruston-cli/src/demo.rs`](../crates/ruston-cli/src/demo.rs) | Each frontend's entry point |
| Desktop identity and icons | [`src/ui/identity.rs`](../src/ui/identity.rs), [`build.rs`](../build.rs) | [`packaging/linux/`](../packaging/linux/), [`assets/icons/hicolor/`](../assets/icons/hicolor/), [`assets/windows/`](../assets/windows/), [`packaging/macos/`](../packaging/macos/) |
| macOS menus, window chrome, and safe root closing | [`src/ui/macos.rs`](../src/ui/macos.rs), [`src/ui/close.rs`](../src/ui/close.rs) | [`src/ui/mod.rs`](../src/ui/mod.rs), [`src/ui/theme.rs`](../src/ui/theme.rs), [`src/ui/settings.rs`](../src/ui/settings.rs), [`src/app/compose.rs`](../src/app/compose.rs) |
| Desktop new-mail detection and notifications | [`src/app/notifications.rs`](../src/app/notifications.rs), [`src/ui/notifications.rs`](../src/ui/notifications.rs) | [`src/ui/auto_refresh.rs`](../src/ui/auto_refresh.rs), [`src/mail/proton.rs`](../src/mail/proton.rs), native [`src/ui/notifications/`](../src/ui/notifications/) |
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
tokens as one keychain entry and the session's API address as another; loading
rejects metadata whose address differs from it. Session writes and token refresh use a per-profile
file lock. On a 401, another process's rotated tokens are reloaded before
refreshing again. Tokens reloaded that way, or renewed meanwhile by another
request, may have expired too: a 401 with them refreshes once more, so a
request asks Proton for new tokens at most once. Persistence errors still
reach the request. The desktop
reports them as a session it could not access and keeps the mailbox open,
since unlocking the credential store fixes the next request; only an expired
session signs it out, including one found while saving an attachment. That reload reads
only the tokens and the bound API address, never the key passphrase. Once the refresh
request is sent, the rotation finishes on a task of its own that holds both
locks, so cancelling the request that needed it, through a view deadline or a
discarded load, cannot lose the new token pair. A refresh
answered with 429, a 5xx status or a verification challenge reports that error
and keeps the session; only a refused refresh token is `Unauthorized`. Signing out of a
shared profile signs out both frontends. Core runs local session cleanup under
that lock on a blocking worker before trying remote revocation. The 30-second
revocation deadline does not include or cancel cleanup, and a started cleanup
worker continues if its caller is cancelled. Revocation runs on a detached
copy of the HTTP client: it may refresh an expired access token in memory, but
never loads or saves the shared session, and `Client::logout` reports whether
Proton confirmed the revocation.
Cleanup checks the saved session UID under the profile lock so a pending logout
cannot erase a replacement login in the same profile. When the CLI cannot resume
a session to log out, `Client::forget_session` clears it under the same lock
without contacting Proton.
Local storage failures reach the CLI and are shown on the desktop sign-in screen;
the desktop still discards its in-memory mailbox and draft.

The desktop keeps mailbox data in memory. The core also offers a SQLite cache
used by CLI sync and local search. Login and resume ask for the user and its
addresses at once, and login asks for the key salts alongside them. They retain
the authenticated `User.ID` alongside the API base URL in `Client`; session UIDs and email addresses
are not cache identities. All client cache operations go through `open_cache`
in [`mail/sync.rs`](../crates/ruston-core/src/mail/sync.rs). The session
metadata also records the `User.ID`, so `LocalCache` opens the same database
from the saved session without a request; the CLI's `messages list --cached`
and `search` use it and work offline. A session saved before the ID was kept is
resumed once instead, which records it. Metadata writes replace the file with
one rename under the profile lock, and a resume records the ID only for the
session it loaded. It selects
`accounts/<profile>/<sha256-of-identity>.db` under the cache directory and checks
the complete account identity stored in SQLite before using the database. A
test pins the identity's JSON and the file name it yields: changing either
would leave every existing cache, and its decrypted bodies, orphaned.
An empty database is bound under an immediate transaction; an unexpected owner
or unbound data causes an error. Account bindings are never reassigned, so a
pending index or sync for one account cannot write into another account's cache.
Signing in again to the same account and server reuses its cache. Legacy
`<profile>.db` caches have no verified owner and are left untouched and unused;
run CLI sync/backfill and `index` to recreate them for the current account.
Signing out clears credentials, but retains the account's disk cache.
Indexing a folder stores decrypted message bodies in that local database;
it is not encrypted at rest
by this code. It keeps no signature verdicts, so it decrypts without looking up
sender keys. Up to three bodies download at once and are saved in listing
order, so a stop leaves exactly the messages before it indexed. On Unix, core creates the cache directory and database with
private permissions and tightens older cache permissions before opening them;
explicit cache paths require a private parent directory. Cache writes and index
maintenance live in [`cache.rs`](../crates/ruston-core/src/cache.rs), which
removes indexed bodies when messages are deleted. FTS maintenance resolves
message IDs through the indexed `msg_fts_rows` table and targets FTS `rowid`s,
avoiding a full scan for each deletion, invalidation, or header update. A
header update skips rows whose subject and participants did not change, since
FTS5 rewrites a whole document, body included, for any change. Opening
an older cache migrates this relation in an immediate transaction: it renames
the existing FTS table to `msg_fts_index`, preserves its contents and rowids,
and backfills only the ID/rowid map. `msg_fts` remains a compatibility view;
its mutation triggers update FTS and the map together, including writes from
older processes. The view exposes the FTS search and rank columns so existing
readers keep working. Bulk cache resets clear FTS and the map directly in the
same transaction to avoid materializing bodies through the view.
Duplicate IDs and orphaned bodies are included in the
map, and deletion removes every matching row. A failed migration rolls back;
reopening is idempotent. Shadow tables are never modified directly. Every
connection turns on SQLite's `secure_delete`, and opening turns on FTS5's
persistent `secure-delete` once, reading only FTS5's settings table to see
whether it already is. Removed rows and index entries are then overwritten in
the file instead of lingering in free pages or until segments merge.
Cached folder listings use
an index on label and message time. Opening an older cache backfills that time
in one transaction; triggers keep the index current when an older process
writes without the new column. That index also serves lookups by label alone,
so opening drops the label-only index earlier versions kept; an older process
recreates it, and the next newer open drops it again. The event stream in
[`mail/sync.rs`](../crates/ruston-core/src/mail/sync.rs)
invalidates an indexed body on a full message update. Each incremental event
batch and its cursor commit together only if the stored cursor still matches
the one used to fetch the batch; a competing sync causes a retry error without
applying stale data. On a cache with no cursor yet, backfill and indexing first
pin one to Proton's latest event, so the next sync applies every change made
after their listing; rows stored without a cursor were never reconciled before.
They capture that cursor before each page request and retain it for all body
downloads from that page. Before writing, SQLite's immediate transaction checks
that cursor. A competing sync or resync causes a retry error without applying
the stale result. Backfill commits each page atomically; indexing commits metadata
and the body together, or metadata and body invalidation when the message is
gone or its body cannot be opened. Any other failed read stops indexing with
its error and writes nothing for that message.
No database transaction is held during a network request or decryption, and
completed pages or messages remain cached if a later result is rejected.
Initial cursor creation is also conditional. When Proton
requests a full refresh, sync pages through all message metadata into temporary
SQLite tables, then replaces the cache and event cursor in one transaction. A
failed rebuild leaves the cached offline data and cursor intact. Sync catches up
on events from the new cursor after the replacement. The full refresh clears
the local body index; run CLI `index` again to rebuild search. See
[Privacy and local data](PRIVACY.md) before changing storage or diagnostics.
Demo modes use fictional data and do not contact Proton. A desktop process
started in demo mode keeps that promise after **Exit demo**: `App` remembers how
it was booted and [`app/auth.rs`](../src/app/auth.rs) refuses to start a sign-in.

CLI [`watch --folder`](../crates/ruston-cli/src/commands/watch.rs) backfills
metadata and indexes the selected folder on its first tick. Later ticks use
sync events for metadata and reindex only after creates, content updates, or a
full refresh; flag and label changes keep the indexed bodies. `watch` polls the event API at the configured interval; it does not
use Server-Sent Events. Backfill and indexing failures stop the command
instead of producing a successful tick.

Most unit tests live beside the code they cover. Core HTTP contract tests are
in [`tests/api_wiremock.rs`](../crates/ruston-core/tests/api_wiremock.rs).
[`tests/live.rs`](../crates/ruston-core/tests/live.rs) requires explicit Proton
test credentials and otherwise skips its live cases. Each case signs in under
a profile of its own and signs out before its checks, so a failure leaves no
session or keychain entry behind. CLI parser and demo tests
are in [`crates/ruston-cli/src/main.rs`](../crates/ruston-cli/src/main.rs).
Run and check commands are in [Development](DEVELOPMENT.md).

The shared [CI workflow](../.github/workflows/ci.yml) checks Linux, macOS and
Windows on x86_64 and arm64, Windows with the native MSVC toolchain. All
platforms run the same formatting, Clippy, documentation, and test gates.
Linux validates the desktop entry and the AppStream metadata in
[`packaging/linux/`](../packaging/linux/), which describe the application to
software centers and AppImage catalogs. Windows additionally builds both
executables and checks them with
[`verify-windows.ps1`](../packaging/windows/verify-windows.ps1): architecture,
window or console subsystem, DLLs Windows includes, manifest, and version
information. Root [`build.rs`](../build.rs) uses `winresource` only for Windows
targets to embed the ICO, Cargo version and an application manifest in
`ruston.exe`; the CLI's [`build.rs`](../crates/ruston-cli/build.rs) embeds its
version and manifest, inline so the published crate needs no files outside it.
[`.cargo/config.toml`](../.cargo/config.toml) links the C runtime statically
for MSVC targets, so neither executable needs the Visual C++ redistributable.
Committed Linux PNGs
and the Windows ICO are derived from the existing logo by
[`packaging/generate-desktop-icons.py`](../packaging/generate-desktop-icons.py).
A Windows-only identity test reads back the process AppUserModelID with the
native API without opening a window.
Native Windows Credential Manager, file dialogs, and window behavior need an
interactive session; the [development guide](DEVELOPMENT.md#windows-native-validation)
defines those manual smoke checks separately from automated test coverage.

The [release workflow](../.github/workflows/release-artifacts.yml) builds the
release artifacts on every push to `main` without publishing them. For a
`vX.Y.Z` tag on a commit of `main` that matches the workspace version, it builds
them again and publishes a draft release. Each artifact is built for one
architecture on a native runner:
- macOS runners run [`package.sh`](../packaging/macos/package.sh), which puts
  `ruston-cli` inside the signed app, then
  [`verify-bundle.sh`](../packaging/macos/verify-bundle.sh).
- Ubuntu 22.04 runners run [`appimage.sh`](../packaging/linux/appimage.sh) with
  pinned, digest-checked AppImage tools. The AppImage's
  [`AppRun`](../packaging/linux/AppRun) starts the app or the CLI, and
  [`check-glibc.sh`](../packaging/linux/check-glibc.sh) holds the glibc floor at
  2.35.
- Windows Server 2025 and Windows 11 ARM runners run
  [`package.ps1`](../packaging/windows/package.ps1) with a pinned,
  digest-checked Inno Setup. It checks both executables with
  `verify-windows.ps1` and writes a per-user installer, from
  [`ruston-mail.iss`](../packaging/windows/ruston-mail.iss), and a portable ZIP.
  The installer adds the Start menu shortcut with the app's AppUserModelID and
  optionally the email-link registration and a `PATH` entry, all removed again
  on uninstall.
- Every macOS and Linux app is started once with
  [`smoke-test.sh`](../packaging/smoke-test.sh). On Windows,
  [`smoke-test.ps1`](../packaging/windows/smoke-test.ps1) requires the window
  to open and draw without an error box, since a failure to start shows one
  instead of ending the process, and
  [`test-installer.ps1`](../packaging/windows/test-installer.ps1) installs,
  starts and uninstalls the installer.

Only the publishing job may write to the repository. It adds `SHA256SUMS.txt`,
build provenance attestations, and notes that
[`release-notes.sh`](../packaging/release-notes.sh) writes from the Conventional
Commits since the previous tag, grouped by the areas that
[`release-note-areas.tsv`](../packaging/release-note-areas.tsv) gives their
scopes. It never replaces files already uploaded, since builds are not
bit-for-bit reproducible: a published release fails the job, and so does a
draft that already has files. A draft keeps its notes, which may have been
edited by hand.

## Keeping this map current

Review this page with every change. Update the affected section in the same
change when a package boundary, responsibility, important flow, persistence
behavior, or starting path changes. Keep the route to code and tests useful;
avoid copying API details that already live beside the implementation.
