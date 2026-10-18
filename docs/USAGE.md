# Using Ruston Mail

Ruston Mail has three panes: folders on the left, conversations in the middle,
and the open conversation on the right. Drag either divider to resize them.
The app remembers the layout for the next run, along with the window's size and
place, and on Windows and Linux whether it was maximized. A place on a screen
that is no longer connected is not reused.

When a small window or a high interface scale leaves too little room for all
three panes, the app shows one pane at a time. The window can be as narrow as
480 points, so it fits half a laptop screen in this mode. Use **Folders**, **Mail**, and
**Reading** at the top to move between them; **Reading** becomes **Message**
while writing in the reading pane. Opening a conversation switches to its
content, and the search shortcut returns to **Mail**. Widening the window or
reducing the interface scale brings back the three panes and their saved widths.
Switching panes keeps the open conversation, draft, and list position.

## Signing in

Enter your Proton username or email and password. Ruston Mail asks for a TOTP
code, a separate mailbox password, or Proton's browser-based human verification
when the account needs one. Each step opens with the cursor in its first empty
field, so you can type right away.

A saved session opens automatically on later runs. Signing out asks Proton to
revoke that session and removes its local copy. If you were reading one of your
own folders or labels, the next sign-in opens the Inbox instead. If you are writing a message,
Ruston asks before discarding it. While a message is being sent, **Sign out**
is unavailable until the send finishes.

FIDO2 and WebAuthn security keys are not supported yet.

## Finding mail

The sidebar contains Proton's standard folders, followed by your custom folders
and labels. Unread counts update as mail changes. If your folders and labels
cannot be loaded, the open folder stays open and the next refresh tries again.
Folders and labels scroll independently when they do not fit. The account,
Settings, and sign-out actions remain at the bottom of the sidebar.
While the desktop has focus, it checks for new mail about once a minute. While
open without focus, it checks about every three minutes, including minimized
windows. Returning after at least 30 seconds can trigger an earlier check,
unless one just succeeded or a failure is still backing off. Busy operations
delay automatic checks; failed folder refreshes increase the wait to at most five minutes.
Closing the app or signing out stops these checks. The demo does not poll Proton.
These checks leave the **Refresh** button as it is; pressing it during one shows
that refresh instead of starting another.

When **Show desktop notifications for new mail** is enabled, Ruston checks the
20 latest Inbox message records separately from the folder on screen. The first
successful check establishes a silent reference, so existing mail is not
announced at sign-in or when enabling the option. Later checks announce at most
one newly observed unread message, even if another message became read and the
total unread count stayed unchanged. Replies have their own message IDs;
marking existing mail unread does not create a new-mail notification. This is
periodic, best-effort detection rather than push delivery; large bursts,
backdated messages, or mail already read elsewhere can be omitted.

Notifications use the Linux desktop notification service, Windows native
toasts under Ruston's identity, and UserNotifications on macOS. Use the installed
macOS `.app` bundle and allow the system permission prompt. Settings shows the
permission/service status. System notification preferences and Do Not Disturb
still determine presentation; macOS normally suppresses banners while Ruston
is in the foreground. On Windows, Ruston creates a per-user Start menu shortcut
if needed. Clicking a transient toast while Ruston is running restores and
focuses the mailbox window.

Typing in the search field narrows the conversations already loaded, and the
open conversation stays open even when it no longer matches. Press `Enter` to
ask Proton to search the whole mailbox; while the folder is still loading, the
search starts once it has loaded. Clear the field to return to the open folder.

Folder lists and server search load 50 conversations at a time. Use **Load more**
at the end of the list to request another page when available. Loading another
page keeps the open message and your position in the list. Refresh keeps verified
message groups visible while updating them in the background. Repeated refreshes
do not add duplicate messages. Conversation grouping preserves the reader and
anchors the list to the same message or conversation when rows change. A list
at the top stays at the top, including on startup. If the open message's row is
temporarily absent, its content stays visible; row actions become available again
when that row returns.

If a refresh, **Load more**, or a search fails while conversations are listed, a
line above them says so; **Try again** repeats what failed, so after a failed
search it searches again rather than reloading the folder.

## Reading and organizing

Messages in a conversation appear oldest first. The newest message and any you
have not read start open; older messages and quoted replies start folded. These
defaults can be changed in Settings. A long conversation opens scrolled to its
first unread message, or to its newest one.

Ruston checks some grouped conversations in the background to separate repeated
incoming messages. These checks are limited and cancelled when you change
folders, submit a search, refresh, or sign out. Conversations keep Proton's
grouping when a check cannot run or finish.

Each message shows its body signature status, even when folded: verified,
unsigned, not verified, or invalid. Folded messages keep the sender's address
next to the name the sender chose, because a signature vouches for the address,
not for the name. An invalid signature is highlighted in red;
treat that message's contents with caution. The status applies to the body,
not to its attachments. Verification uses available sender keys, never one
Proton marks as compromised, and does not establish the sender's identity
independently. If the sender's keys cannot be fetched, the message shows as not
verified.

A message that cannot be decrypted, for example because it was encrypted to a
key your account no longer has, shows a notice instead of its text. The rest of
the conversation still opens, and you can still reply to it or forward it,
without its text quoted.

HTML mail is converted to native text and layout. Headings, lists, links, basic
formatting, and quotes remain, but scripts and remote content do not run. Remote
images appear as their description when one is available.

The reader can:

- Archive a conversation.
- Star or unstar it.
- Mark it read or unread.
- Move it to Spam or Trash.
- Move it back to the Inbox. In Archive and Trash this is the first button; in
  Spam it is **Not spam**.
- Move it into one of your folders, or back to the Inbox, with **Move to…**.
- Add or remove a custom label from the tag button, which is filled while the
  conversation carries one.
- Undo the latest move while its notice remains visible. Under a label,
  Starred, or in search results the conversation's folder is not known, so a
  move made there offers no undo until you have moved it once from that view;
  **Move to…** can still put it back.

Right-click a conversation in the list, or click it with two fingers on a
trackpad, for the same moves the reader offers it, **Mark as read** or **Mark as
unread**, and **Add star** or **Remove star**. They act on that conversation
without opening it, so it is not marked read, and the open one stays open.

Actions show at once, without waiting for Proton: a moved conversation leaves
the list and the next one opens, and its undo is offered right away. Under
Starred or a label, a conversation moved to another folder keeps its star and
labels, so it stays in the list; a move to Spam or Trash still takes it out.
Actions taken in quick succession go to Proton one after another, in order. If
Proton refuses one, the reader says so and the change is taken back; a moved
conversation comes back when the folder reloads.

If an automatic read is still pending, **Mark unread** waits for it to succeed
before it is sent. If that read fails or times out, the pending action reports
an error and is taken back.

Ruston Mail does not permanently delete mail. It also cannot create, rename, or
remove folders and labels yet.

Right-click a link in a message for **Open link…**, which asks first like a
click does, or **Copy link**, which copies its address without opening it.

Web links open in the system browser. A confirmation dialog over the window
shows the destination first by default; a long address scrolls inside it, and
`Esc` dismisses it before anything else. Email (`mailto:`) links use Ruston's
composer after the same confirmation, which names every recipient the new
message would get, those a `?to=` adds included; past three it names the
first three and how many more.

## Opening email links

When Ruston is selected as your system's email-link handler, an external
`mailto:` link opens a new message with its recipients, subject, and body.
You review and send it yourself. UTF-8 text and up to 100 different recipients
are supported. A link with more recipients is rejected, and so is one whose
recipient domain is not written in ASCII, as in the composer. CC/BCC,
attachments, and arbitrary mail headers
in the link are ignored. Link bodies are treated as text, never interpreted as HTML.
A link Ruston cannot open still opens the window, or brings it forward, with a
notice that the link was not opened.

If needed, sign in first; the link waits in memory. An existing draft is never
replaced, including a blank draft or a message being sent. A notice lets you
dismiss the waiting link; otherwise, finish or close your draft normally to
open it. Links also wait while Settings, link confirmation, a file dialog, or
the main-window closing confirmation is open. Closing the app or ending a
session clears pending links. At most 16 links can wait, with 16 KiB per URL.

Launching Ruston again activates the existing instance for that user; the demo
has its own instance. The system can restrict bringing a window to the front,
especially on Wayland, where it can show an attention indicator instead.
See the [setup instructions](DEVELOPMENT.md#email-link-handlers-and-activation)
to enable email-link handling on your platform. On Windows, the installer's
email-link option registers Ruston; then choose it for MAILTO in Settings >
Apps > Default apps.

## Attachments

Choose **Save** to save an attachment in the system Downloads folder. Ruston Mail
uses a safe file name and never replaces an existing file; it adds a number to
the new name instead. On Windows, saved attachments are marked as downloaded
from the internet, like a browser's downloads, so SmartScreen checks a program
before it runs and Office opens documents in Protected View.

Choose **Save as…** to select another folder or filename in the native save
dialog. Cancelling does not download or write the attachment. Choose a new
filename: this action also never replaces an existing file, even if the native
dialog offers replacement. If the destination exists, Ruston shows an error
and you can choose another name.

The app can reveal a saved attachment in the file manager. Use **Attach…**
beside **Send** in the composer to add local files to an outgoing message, or
drop files on the window that shows the message being written. Dropping files
is not available on Wayland.
On Linux, **Show in folder** asks the file manager to select the saved file;
if the desktop does not support selection, it opens the containing folder.

Received attachments larger than 50 MiB cannot be opened.

When an incoming attachment carries a signature, Ruston verifies it before
saving the file. An invalid or malformed signature, or missing verification
keys, prevents the download and shows an error. You can retry if keys become
available later. If the sender's keys cannot be fetched, for example while
offline, Ruston reports that problem instead, and you can retry. Attachments
without a signature can still be saved; their contents have not been
authenticated by a signature. These checks also apply to CLI downloads and EML
exports.

New local attachments are limited to 32 MiB per file and 128 MiB combined in
both the desktop and CLI. These are Ruston's safety limits; Proton may impose
additional limits. Files that become too large before upload are rejected too.
The desktop keeps your message open and displays the size error. Forwarded
attachments already stored on Proton do not count toward this local file limit.

## Writing

Select **Write** for a new message. Recipient fields accept multiple addresses
separated by commas or semicolons. Invalid addresses stop the whole send instead
of being silently skipped. Domains are written in ASCII: unaccented letters,
digits, and hyphens. Type an international domain, including one with accents,
in its `xn--` form. Letters from another script that only look Latin, as in a
fake `paypal.com`, are refused. When a domain's script is the only problem, the
composer says how to send it anyway.

To type international domains as they are written, turn on **Accept
international domain names** under Settings → Advanced. The composer then
sends each such domain in its ASCII `xn--` form and names both forms in a
warning, such as `münchen.de is sent as xn--mnchen-3ya.de`; a fake
`paypal.com` shows up there as `xn--pypal-4ve.com`. Email links that open the
composer still refuse these domains, whatever the setting says.

After you leave a recipient field, the composer shows how Proton will protect
the new message: end-to-end encrypted for every recipient, or a warning that
names the addresses that will not get an end-to-end encrypted copy. That
includes a Proton address whose owner turned end-to-end encryption off, for
example because it forwards mail to another provider. Addresses
are looked up once per draft and only after the field loses focus. If a lookup
fails, the warning names that address too and claims nothing about it. Replies
show no notice; their recipients come from the original message. Turn off **Check
recipient encryption while writing** under Settings → Advanced to ask Proton
nothing until you send; the notice is not shown then.

The composer opens in the reading pane by default, so folders and conversations
stay visible. Settings can open it in a separate window instead. The same choice
applies to new messages, replies, and forwards. It opens with the cursor in
**To** for a new message or a forward, and in the message for a reply. While it
fills the reading pane, the list keys (`j`, `k`, the arrows and `Enter`) wait,
so they never open a conversation out of sight.
The composer scrolls vertically when its fields or attachments exceed the
available height, in either placement.

Closing the main window asks before discarding a written message, whether the
composer is in the reading pane or a separate window. Choose **Keep writing**
to cancel or **Discard and close** to exit. Repeating the close request does not
discard the draft. Closing is blocked while a message is being sent or a native
file dialog is open. Unapplied Settings changes are resolved before the draft.
On macOS, **Quit Ruston Mail** in the application menu and `Cmd+Q` use the same
confirmation. Direct termination, including **Quit** from the Dock or system
shutdown, can bypass it; drafts are not saved to disk.

On macOS, **File > New Message** (`Cmd+N`) uses the same composer and preserves
an existing draft. **Ruston Mail > Settings…** (`Cmd+,`) opens preferences;
both commands are available while signed in. The main window reserves space
for native window buttons according to their measured size and interface zoom.
**File > Close Window** (`Cmd+W`) closes the window in front with the same
confirmation as its close button; closing the main window quits. **Edit** has
Cut, Copy, Paste and Select All for text, as well as in native file dialogs,
along with the system's Emoji & Symbols and Dictation; Undo and Redo keep
working with `Cmd+Z` and `Shift+Cmd+Z`. **View** has **Actual Size**
(`Cmd+0`), **Zoom In** (`Cmd++`) and **Zoom Out** (`Cmd+-`), which step the
interface scale from Settings, and **Enter Full Screen**. **Window > Minimize**
(`Cmd+M`), **Zoom** and the list of open windows work as in other Mac apps.

The reader also offers **Reply**, **Reply all**, and **Forward**. A reply goes
to the addresses the original message asks replies to go to (its Reply-To), or
to its sender; **Reply all** also copies in its other recipients, except your
own addresses. Replying to a message you sent continues with the people you sent
it to. The composer lists these recipients, and warns when the
message's Reply-To sends the reply somewhere other than its sender. The subject
is written for you. Replies and forwards are sent from the address the original
message reached.

Messages can be sent as plain text or HTML. The editor remains plain text in
both modes: text that looks like an HTML tag is sent as text, not executed as
markup. Recipients outside Proton who receive PGP-encrypted mail through a
published key get an HTML message as plain text, since that kind of PGP mail
carries text only; links keep their address in angle brackets.

Ruston Mail does not save drafts, schedule messages, or offer self-destructing
messages yet.

## Settings

Select **Settings** in the sidebar to show preferences beside it, in place of
the conversation list and reader. **Apply** saves changes without leaving.
Press `Esc`, select a folder, or choose **Back to mail** to return. If you have
unapplied changes, Ruston Mail asks whether to apply them, discard them, or keep
editing. The same confirmation appears before writing a new message or signing
out from Settings.

The **About** section at the bottom shows the app version, credits, and license.
Choose **Source code** to open the project's repository in your browser.
**Advanced**, above the keyboard shortcuts, holds choices most people can leave
alone.

Settings control:

- Whether opening mail marks it read.
- Whether all messages and quoted text start unfolded.
- Whether links require confirmation.
- Whether the composer opens in the reading pane or a new window.
- Plain-text or HTML sending.
- Whether the composer checks recipient encryption while you write (Advanced).
- Whether recipient fields accept international domain names (Advanced).
- The folder shown at startup.
- Dark or light appearance, or on macOS and Windows the system's. Dark remains
  the default.
- Interface scale.
- Optional system interface font, with Ruston's fonts retained as fallback.
- A button to reset the window and pane sizes to their defaults immediately.
- On macOS, whether the Dock icon shows the unread inbox count; on Windows,
  whether the taskbar button does. Windows shows it only while badges are on
  in its taskbar settings (Settings › Personalization › Taskbar › Taskbar
  behaviors).

Window size and pane widths are remembered automatically. Use **Reset window and
pane sizes** under Settings → Interface to restore the defaults; this action
takes effect immediately and does not apply other pending preference changes.
Choose Dark or Light, or on macOS and Windows System, under Settings →
Interface and press Apply. The selection is saved for the next run. System
follows the light or dark mode of macOS or Windows as it changes and keeps the
last scheme you chose, which an older version of Ruston opens with. Linux has
no System choice yet, since its windowing library reports no system mode. The
sign-in screen also has a theme button in its header, which switches to the
other scheme from the one on screen, and a control beside each password field
to show or hide what you type.

**Use system interface font** under Settings → Interface is optional and takes
effect after Apply, without changing text sizes. The default keeps Ruston's
existing look. macOS tries San Francisco, Windows Segoe UI, and Linux the
configured Fontconfig sans-serif family; unavailable fonts fall back to Ruston's.
Installed language fonts also provide missing characters in either mode. They
load in the background after the window opens, so text in other scripts can
show placeholder boxes for a moment at startup.
Coverage depends on local fonts; there are no font downloads. This expands
glyph coverage but does not provide complete bidirectional/RTL text layout.

## Graphics on Windows

On Windows, Ruston Mail draws with DirectX 12 on the graphics processor that
uses the least power, or with Windows' software renderer where there is none,
as in many virtual machines. If DirectX 12 cannot draw the window, Ruston uses
OpenGL instead. To use OpenGL from the start, for example when a graphics
driver draws the window wrongly, set `RUSTON_RENDERER` to `opengl` for your
user in PowerShell and start Ruston again:

```powershell
[Environment]::SetEnvironmentVariable('RUSTON_RENDERER', 'opengl', 'User')
```

Pass `$null` instead of `'opengl'` to go back to DirectX 12. macOS and Linux
always draw with OpenGL.

## Keyboard shortcuts

| Key | Action |
| --- | --- |
| `j` or `Down` | Open the next conversation |
| `k` or `Up` | Open the previous conversation |
| `Enter` | Open the selected conversation; search all mail from the search field |
| `Esc` | Close the open menu, or else the topmost view or prompt |
| `Cmd`/`Ctrl` + `R` | Refresh the folder |
| `Cmd`/`Ctrl` + `F` | Focus the search field |
| `Cmd`/`Ctrl` + `,` | Open Settings on macOS; open or close Settings on Linux/Windows |
| `Cmd`/`Ctrl` + `N` | Write a new message |
| `Cmd`/`Ctrl` + `Enter` | Send the message being written |
| `Delete`, or `Cmd`/`Ctrl` + `Backspace` | Move the selected conversation to Trash |
| `Cmd`/`Ctrl` + `+` (or `=`), `-`, `0` | Zoom in, zoom out, or back to 100%, in the interface scale's steps |

Shortcuts do not run while a text field is using the same key. When `Tab` has
put focus on a button or a row, which shows a violet ring, the arrows move that
focus and `Enter` presses it; `j` and `k` still move through the list. On
Windows, `Ctrl` + `Alt` with a key runs no `Ctrl` shortcut, since Windows types
with it what `AltGr` types.

Screen readers cannot read the composer when it opens in a new window yet:
with one, keep **Settings** › **Writing** on **Reading pane**, the default.

On Windows, turning off **Animation effects** (Windows Settings ›
Accessibility › Visual effects) holds Ruston still too: no fades or gliding
scrolls, and loading placeholders and spinners that do not move.

Screen readers announce each conversation row with what it shows: whether it is
unread, who and what it is about, its date, its star, how many messages it has
and whether it has attachments, and whether it is the conversation open.

## Command line

`ruston-cli` works with the same account from a terminal. Releases carry it
inside the app:

- **macOS:** at `Ruston Mail.app/Contents/MacOS/ruston-cli`. To run it as
  `ruston-cli`, link it into a folder on your `PATH`:

  ```sh
  sudo ln -sf "/Applications/Ruston Mail.app/Contents/MacOS/ruston-cli" /usr/local/bin/ruston-cli
  ```

- **Linux:** pass `--cli` as the AppImage's first argument, or start the
  AppImage through a link named `ruston-cli`.
- **Windows:** `ruston-cli.exe` sits beside `ruston.exe`, in
  `%LOCALAPPDATA%\Programs\Ruston Mail` after installing. Select *Add
  ruston-cli to the PATH* in the installer to run it as `ruston-cli` from any
  terminal; with the portable ZIP, run it from its folder or add the folder to
  your `PATH`.

From source, run `cargo run -p ruston-cli -- --help`. `ruston-cli --help` lists
every command, and `ruston-cli <command> --help` its options.

The CLI shares the desktop session by default. Use `--profile NAME` when you
want a separate login; an existing CLI `default` session remains available with
`--profile default`. `NAME` must be a single path component without separators.

When sending from a terminal, the CLI asks before treating a body that looks
like HTML as HTML. For scripts, pass `--html` explicitly. Recipients reached
through a published PGP key get an HTML body as plain text.

`drafts save` and `drafts edit` upload `--attach` files to the draft. If one
cannot be uploaded, `drafts save` deletes the draft it made, while `drafts
edit` keeps the files attached before it. An edited draft stays written from
its own address unless `--from` names another; one with attachments keeps its
address, since they are encrypted to it. A draft keeps no delivery time,
expiration, or message password: `--send-at`, `--expires`, and
`--eo-password` are options of `messages send`.

Output can be piped into tools that stop reading early, such as `head`: the
CLI then ends quietly with exit status 0.

Each CLI HTTP attempt has a 60-second deadline; attachment transfers have 180
seconds. Commands with multiple requests or a rate-limit retry can take longer.
A timed-out final send is reported as unconfirmed with its draft ID. Check Sent
before retrying, since Proton may have accepted the message.

The CLI's `export --out DIRECTORY` saves reconstructed `.eml` messages with
their decrypted attachments. These files are plaintext on disk. Existing files
are never replaced: running the same export again into the same directory
resumes it, skipping the messages already there, so use an empty directory to
start over. A message that cannot be decrypted, or whose attachments cannot,
is skipped and listed, the rest are still exported, and the command then exits
with an error. A network or session failure stops the export; run it again to
continue. The export does not preserve the original wire message or every
original header; see [Privacy and local data](PRIVACY.md) for details.

`index` builds an optional local search index for `search`. It stores
decrypted message bodies in a SQLite database that Ruston Mail does not
encrypt. A network, session, or rate-limit failure stops `index` with an
error and keeps what was already indexed; run it again to continue.
