# Using Ruston Mail

Ruston Mail has three panes: folders on the left, conversations in the middle,
and the open conversation on the right. Drag either divider to resize them.
The app remembers the layout for the next run.

When a small window or a high interface scale leaves too little room for all
three panes, the app shows one pane at a time. Use **Folders**, **Mail**, and
**Reading** at the top to move between them; **Reading** becomes **Message**
while writing in the reading pane. Opening a conversation switches to its
content, and the search shortcut returns to **Mail**. Widening the window or
reducing the interface scale brings back the three panes and their saved widths.
Switching panes keeps the open conversation, draft, and list position.

## Signing in

Enter your Proton username or email and password. Ruston Mail asks for a TOTP
code, a separate mailbox password, or Proton's browser-based human verification
when the account needs one.

A saved session opens automatically on later runs. Signing out asks Proton to
revoke that session and removes its local copy.

FIDO2 and WebAuthn security keys are not supported yet.

## Finding mail

The sidebar contains Proton's standard folders, followed by your custom folders
and labels. Unread counts update as mail changes.
Folders and labels scroll independently when they do not fit. The account,
Settings, and sign-out actions remain at the bottom of the sidebar.
While the desktop has focus, it checks for new mail about once a minute. After
being away for at least 30 seconds, it checks again when you return.

Typing in the search field narrows the conversations already loaded. Press
`Enter` to ask Proton to search the whole mailbox. Clear the field to return to
the open folder.

Folder lists and server search load 50 conversations at a time. Use **Load more**
at the end of the list to request another page when available. Loading another
page keeps the open message and your position in the list. Refresh keeps verified
message groups visible while updating them in the background. Repeated refreshes
do not add duplicate messages. Conversation grouping preserves the reader and
anchors the list to the same message or conversation when rows change. A list
at the top stays at the top, including on startup. If the open message's row is
temporarily absent, its content stays visible; row actions become available again
when that row returns.

## Reading and organizing

Messages in a conversation appear oldest first. The newest message starts open;
older messages and quoted replies start folded. These defaults can be changed in
Settings.

Ruston checks some grouped conversations in the background to separate repeated
incoming messages. These checks are limited and cancelled when you change
folders, submit a search, refresh, or sign out. Conversations keep Proton's
grouping when a check cannot run or finish.

Each message shows its body signature status, even when folded: verified,
unsigned, not verified, or invalid. An invalid signature is highlighted in red;
treat that message's contents with caution. The status applies to the body,
not to its attachments. Verification uses available sender keys and does not
establish the sender's identity independently.

HTML mail is converted to native text and layout. Headings, lists, links, basic
formatting, and quotes remain, but scripts and remote content do not run. Remote
images appear as their description when one is available.

The reader can:

- Archive a conversation.
- Star or unstar it.
- Mark it read or unread.
- Move it to Spam or Trash.
- Add or remove a custom label.
- Undo the latest move while its notice remains visible.

If an automatic read is still pending, **Mark unread** waits for it to succeed
before applying your choice. If that read fails or times out, the pending action
reports an error; it is not shown as a successful change.

Ruston Mail does not permanently delete mail. It also cannot create, rename, or
remove folders and labels yet.

Links open in the system browser. A confirmation prompt shows the destination
first by default.

## Attachments

Select an attachment to save it in the system Downloads folder. Ruston Mail
uses a safe file name and never replaces an existing file; it adds a number to
the new name instead.

The app can reveal a saved attachment in the file manager. Use **Attach files**
in the composer to add local files to an outgoing message.

When an incoming attachment carries a signature, Ruston verifies it before
saving the file. An invalid or malformed signature, or missing verification
keys, prevents the download and shows an error. You can retry if keys become
available later. Attachments without a signature can still be saved; their
contents have not been authenticated by a signature. These checks also apply
to CLI downloads and EML exports.

New local attachments are limited to 32 MiB per file and 128 MiB combined in
both the desktop and CLI. These are Ruston's safety limits; Proton may impose
additional limits. Files that become too large before upload are rejected too.
The desktop keeps your message open and displays the size error. Forwarded
attachments already stored on Proton do not count toward this local file limit.

## Writing

Select **Write** for a new message. Recipient fields accept multiple addresses
separated by commas or semicolons. Invalid addresses stop the whole send instead
of being silently skipped.

The composer opens in the reading pane by default, so folders and conversations
stay visible. Settings can open it in a separate window instead. The same choice
applies to new messages, replies, and forwards.
The composer scrolls vertically when its fields or attachments exceed the
available height, in either placement.

The reader also offers **Reply**, **Reply all**, and **Forward**. Proton
derives reply recipients and subjects from the original message.

Messages can be sent as plain text or HTML. The editor remains plain text in
both modes: text that looks like an HTML tag is sent as text, not executed as
markup.

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

Settings control:

- Whether opening mail marks it read.
- Whether all messages and quoted text start unfolded.
- Whether links require confirmation.
- Whether the composer opens in the reading pane or a new window.
- Plain-text or HTML sending.
- The folder shown at startup.
- Dark or light appearance. Dark remains the default for existing settings.
- Interface scale.
- A button to reset the window and pane sizes to their defaults immediately.
- On macOS, whether the Dock icon shows the unread inbox count.

Window size and pane widths are remembered automatically. Use **Reset window and
pane sizes** under Settings → Interface to restore the defaults; this action
takes effect immediately and does not apply other pending preference changes.
Choose Dark or Light under Settings → Interface and press Apply. The selection
is saved for the next run; the app does not follow the system theme.
The sign-in screen also has a theme button in its header and a control beside
each password field to show or hide what you type.

## Keyboard shortcuts

| Key | Action |
| --- | --- |
| `j` or `Down` | Open the next conversation |
| `k` or `Up` | Open the previous conversation |
| `Enter` | Open the selected conversation; search all mail from the search field |
| `Esc` | Close the topmost view or prompt |
| `Cmd`/`Ctrl` + `R` | Refresh the folder |
| `Cmd`/`Ctrl` + `F` | Focus the search field |
| `Cmd`/`Ctrl` + `,` | Open or close Settings |
| `Cmd`/`Ctrl` + `N` | Write a new message |
| `Cmd`/`Ctrl` + `Enter` | Send the message being written |
| `Cmd` + `Backspace` / `Delete` | Move the selected conversation to Trash |

Shortcuts do not run while a text field is using the same key.
