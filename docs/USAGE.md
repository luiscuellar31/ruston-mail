# Using Ruston Mail

Ruston Mail has three panes: folders on the left, conversations in the middle,
and the open conversation on the right. Drag either divider to resize them.
The app remembers the layout for the next run.

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

Typing in the search field narrows the conversations already loaded. Press
`Enter` to ask Proton to search the whole mailbox. Clear the field to return to
the open folder.

Folder lists load 50 conversations at a time. Server search currently returns
one batch of up to 50 results and cannot load another page.

## Reading and organizing

Messages in a conversation appear oldest first. The newest message starts open;
older messages and quoted replies start folded. These defaults can be changed in
Settings.

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

Ruston Mail does not permanently delete mail. It also cannot create, rename, or
remove folders and labels yet.

Links open in the system browser. A confirmation prompt shows the destination
first by default.

## Attachments

Select an attachment to save it in the system Downloads folder. Ruston Mail
uses a safe file name and never replaces an existing file; it adds a number to
the new name instead.

The app saves attachments but does not open them. Files cannot be attached to a
new outgoing message yet.

## Writing

Select **Write** for a new message. Recipient fields accept multiple addresses
separated by commas or semicolons. Invalid addresses stop the whole send instead
of being silently skipped.

The composer opens in the reading pane by default, so folders and conversations
stay visible. Settings can open it in a separate window instead. The same choice
applies to new messages, replies, and forwards.

The reader also offers **Reply**, **Reply all**, and **Forward**. Proton
derives reply recipients and subjects from the original message.

Messages can be sent as plain text or HTML. The editor remains plain text in
both modes: text that looks like an HTML tag is sent as text, not executed as
markup.

Ruston Mail does not save drafts, schedule messages, or offer self-destructing
messages yet.

## Settings

Settings control:

- Whether opening mail marks it read.
- Whether all messages and quoted text start unfolded.
- Whether links require confirmation.
- Whether the composer opens in the reading pane or a new window.
- Plain-text or HTML sending.
- The folder shown at startup.
- Interface scale.

Window size and pane widths are remembered automatically.

## Keyboard shortcuts

| Key | Action |
| --- | --- |
| `j` or `Down` | Open the next conversation |
| `k` or `Up` | Open the previous conversation |
| `Enter` | Open the selected conversation; search all mail from the search field |
| `Esc` | Close the topmost view or prompt |
| `Cmd`/`Ctrl` + `R` | Refresh the folder |
| `Cmd`/`Ctrl` + `F` | Focus the search field |

Shortcuts do not run while a text field is using the same key.
