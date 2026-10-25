# Privacy and local data

Ruston Mail is an unofficial client and has not received an independent
security audit. This page describes the desktop client and the separate CLI
where their local data differs.

## Mail content

Typography reads a small set of installed local fonts, with bounded file sizes
and a process-lifetime cache. It does not download fonts or use email contents
to select filesystem paths. On Linux, the optional system interface font uses
a bounded `fc-match` subprocess to discover the configured sans-serif family.

Ruston Mail talks directly to Proton through `ruston-core`; it uses no remote
relay server. `ruston-core` handles authentication, key unlocking, and
mail cryptography. Its TLS connections offer the X25519MLKEM768 post-quantum
hybrid key exchange first, which Proton accepts, so traffic recorded today
stays protected as long as either half of that exchange holds. Requests never
follow a redirect to another host, so a request body, refresh token or
two-factor code cannot be forwarded off the API, and plain HTTP is refused
except for a server on the same machine.

While a new message is being written, the desktop asks Proton for the public
keys of each well-formed recipient address after its field loses focus, to show
whether that copy will be end-to-end encrypted. This is the request sending
makes for the same address; it happens earlier, and also for a message that is
never sent. Addresses are not looked up while you type. The demo answers
locally. Turn off **Check recipient encryption while writing** under
Settings → Advanced to make the request only when sending.

In the desktop client, mail and mailbox lists stay in memory while it runs.
In addition to the conversation on screen, up to eight previously opened
conversations can remain in memory for quick backtracking, and so can the
loaded lists of the eight folders left most recently. The desktop client
does not write a persistent mail cache to disk. Sign-in fields keep no undo
history, and a draft's undo history is dropped once the draft is sent,
discarded, or left behind by signing out, so undo cannot bring back another
draft or a typed password. A message being written when Proton ends the
session stays in memory, never on disk, for the same account's next sign-in:
another account's sign-in, discarding it or closing Ruston drops it.

Passwords and codes typed at sign-in are kept in buffers with room reserved
for them, so typing does not move them and leave earlier copies behind, and
they are erased when the form is cleared. The sign-in screen measures its
layout without them. The interface library still makes short-lived copies of a
field's text while drawing it, which are freed without being erased.

While Ruston is running, desktop activation listens on Linux and macOS on a
Unix socket inside its private `activation` directory, which other users cannot
enter, and on Windows on IPv4 loopback, at a randomly assigned port that any
local user could connect to. A path too long for a socket, as a long macOS home
folder can make, falls back to loopback. Either way it accepts bounded
email-link requests after a random 256-bit token handshake; it exposes no
account or mailbox data and cannot send mail. The `activation` directory under
Ruston's configuration directory contains an OS lock file, the socket, and an
endpoint record with the port (0 for the socket), token, and process ID. Unix
permissions are 0700 for the directory and 0600 for the files and the socket. Windows
keeps the directory in the local, non-roaming application data,
`%LOCALAPPDATA%\Ruston Mail\data\activation`, with the access rules it inherits
from the user's profile.
The endpoint is removed on orderly exit and replaced after a crash. The lock
file remains for safe coordination. `activation-demo` isolates the demo.
Pending email links stay in memory through sign-in and are cleared when a
session ends or the application exits. Ruston does not log their contents.
URLs passed as arguments can appear in OS process listings, as with other
applications launched as URL handlers.

When desktop notifications are enabled, a new-mail notification can show the
sender and subject. A pending notification result is discarded during sign-out
or after the session changes. Disabling the option also invalidates pending
detection and native delivery work. On Windows and Linux, a new notification
replaces the previous one. Signing out or turning notifications off removes
Ruston's notifications from the Windows notification center, and on Linux
closes the one on screen; a notification server that also keeps a history may
keep its own copy there. On macOS, once handed to the operating system,
notifications can remain in its notification history; signing out does not
remove notifications already delivered. No
email content is logged by the
notification integration. Headers are bounded and treated as literal text,
without controls or invisible direction marks; notification delivery does not
load links or remote images.

The open desktop polls about once a minute with focus and every three minutes
without it, with failure backoff up to five minutes. It stops on sign-out or
application exit and does not install a background service. If notifications
are enabled, an additional bounded request reads the latest 20 Inbox message
metadata records, including read records, without downloading bodies or
attachments. Its reference IDs and timestamp stay in process memory. The first
successful snapshot is silent; new unread IDs can notify even when the unread
count does not grow. Turning the option off clears this reference.

Windows toast registration needs `Ruston Mail.lnk` in the user's Start menu
Programs folder, carrying Ruston's AppUserModelID. The installer creates it and
removes it on uninstall; a portable copy creates it at the first notification,
pointing to the running executable. It does not replace a shortcut belonging
to another application and requires no administrator privileges. A portable
copy's shortcut remains when the app closes; delete it with the folder. macOS asks
for system authorization when notifications become active for a signed-in
account; a bare development executable does not request permission or notify.

HTML messages do not run in a browser view. They are sanitized, parsed, and
drawn with native UI elements. Sanitizing keeps only `http`, `https` and
`mailto` links and drops relative ones, so EML exports and quoted replies
carry no other kind. Scripts do not run, and remote images are never
requested. This also blocks tracking pixels.

The CLI's default text view converts HTML to readable Markdown-style text.
It omits hidden content and images, including tracking pixels, without making
remote requests. Quotes and lists show at most eight nesting levels. Converted
text is limited to 1 MiB including its truncation notice; conversion may stop
earlier if its temporary buffers reach their limit. Oversized link destinations
are shown as plain labels. The notice explains how to read the full body with
`--format raw` or save it with `--output`. These display limits do not truncate
explicit HTML/raw formats, JSON, or exported files.

The CLI filters terminal controls in displayed mail, metadata,
prompts, status messages, errors, and diagnostics; line feeds and tabs remain.
This includes escape sequences that could otherwise change the clipboard,
terminal title, or cursor position, and invisible direction marks that could
reorder what follows, such as an attachment's extension or a sender's address.
The default text output is filtered even when redirected. JSON escapes these
characters while retaining the original decoded values. Explicit `--format html` and `--format raw` preserve body content
when redirected to a file or pipe; when displayed on a terminal, they also
filter controls. `messages read --output` and EML exports preserve the body
content supplied by core, including its HTML sanitization. `--output` writes a
file only you can read, as exports are below, and replaces whatever is at that
path, a symbolic link included, instead of writing through it.
When replying or forwarding as HTML, sender details and quoted plain text are
escaped before insertion; quoted HTML keeps the markup sanitized during reading.

Links open in the system browser. Ruston Mail shows the real destination before
opening it by default; this prompt can be disabled in Settings.

## CLI cache

The CLI uses `ruston-core`'s SQLite cache for `sync`, optional
backfills, and local `index` and `search` commands. The cache is stored under
the platform cache directory for `ruston-mail`, in
`accounts/<profile>/<account-fingerprint>.db`. The fingerprint is SHA-256 of
the API base URL and authenticated account ID; the database also stores and
checks that full identity before use. Different accounts in the same profile
use separate databases, including when an earlier sync or index is still running.
A new login to the same account and server reuses its cache. Signing out does
not delete these databases.

Older `<profile>.db` caches have no verified account identity. Ruston Mail leaves
them on disk and does not use or import their contents. Run `sync` with backfill
and `index` again to rebuild the current account's cache; local search is empty
until you index again. Remove old databases yourself if you no longer need them.

Sync and backfill store message metadata. Running `index` also stores decrypted
message bodies and other searchable fields in SQLite full-text search. This
database is **not encrypted at rest by Ruston Mail**. The desktop client does
not use this cache. By default `index` reads every folder except Trash and Spam
(`--folder almost-all`); `--folder all` includes them. A message moved to Trash
or Spam after it was indexed keeps its indexed body until `sync` applies its
deletion or the cache is rebuilt. Once `sync` applies a deletion, it removes the active search
entry. A full message update removes its indexed body; run `index` again to
make the updated body searchable. Removed or replaced bodies and search
entries are overwritten inside the database file, with SQLite's
`secure_delete` and FTS5's `secure-delete`, instead of lingering in its free
space. This does not erase copies left by SQLite's temporary journal, the file
system, the storage device, or backups. Content removed by Ruston Mail 0.4.0 or
earlier can remain in the file; to rebuild it without those remnants, delete the
database and run `sync` with backfill and `index` again.
Opening an existing cache upgrades its search lookup automatically in a
transaction, preserving the stored bodies, metadata, account binding, and sync
cursor. This upgrade does not require downloading or indexing mail again. If
it fails, the previous search data remains intact and the operation reports an
error; a later opening can retry the upgrade.

On Unix, the default cache directory uses mode `0700` and each SQLite database
uses mode `0600`. Opening an older cache at the default location tightens those
permissions before SQLite reads it, without deleting its contents. A custom
cache path must have a private `0700` parent directory; symlinks at the cache
directory or database path are rejected. SQLite's journal files live beside the
database, so the private directory also protects them. These checks cannot undo
past exposure of an older cache or detect separate filesystem ACL grants. On
Windows, access depends on the destination directory's ACLs; this code does
not manage Windows ACLs.

When Proton requests a full refresh, sync rebuilds all cached message metadata
before replacing the current cache. If the rebuild fails, cached offline data
and the sync cursor remain available. A completed refresh clears the local
search index; run `index` again to make message bodies searchable.
Concurrent incremental sync runs commit each event batch with its cursor. If
another process has advanced the cursor, the stale run stops with a retry error
and leaves that batch unapplied.
Indexing drops a message's older body when Proton says the message is gone or
its body cannot be decrypted. Any other failed body download, such as a lost
connection or rate limiting, stops `index` with an error and keeps what was
indexed for that message.
On a cache that has never synchronized, backfill and indexing first record
Proton's latest event as the sync cursor, so the next `sync` also removes a
message deleted after they listed it, instead of leaving it searchable.
Backfill and indexing also check the cursor before saving downloaded data,
including when they drop a body. If sync changes the cursor while a page
or body is being downloaded, the operation stops with a retry error and does
not restore deleted mail or overwrite the newer cached data with that result.
Run the command again to continue; earlier committed pages and messages remain
cached. These checks apply to changes already synchronized into the local
cache; run `sync` to receive remote changes.

## Saved session and settings

The Proton password is not saved. The CLI can read sign-in secrets from
`PROTON_PASSWORD`, `PROTON_MAILBOX_PASSWORD`, `PROTON_TOTP` and
`PROTON_HV_TOKEN`; a browser it opens for human verification does not inherit
them. On a terminal it asks, hidden, for the account password, a TOTP code and
a separate mailbox password when they are needed and not given, and
`messages send --eo-password` without a value asks for the message password.
None of them then goes on the command line, where process listings and shell
history would show it. Both apps open human verification only on Proton's own
hosts: a challenge that names another host gets Proton's default page, since
the page receives the challenge's token. While it waits for a CAPTCHA, the CLI listens on a loopback port and takes
a token only with the random code the captcha page it opened carries at the end
of its address, so no other web page can hand it one. The address's fragment
never reaches Proton. A capture bookmarklet saved by an earlier version is
refused and has to be saved again. With `--captcha-chrome`, the page opens in
Chrome with a new throwaway profile: a folder the CLI creates itself in the
temporary directory, under a random name and, on Unix, for this user alone, so
another user cannot prepare it. It is removed when verification ends. `ruston-core` stores the access token, refresh
token, and key passphrase in the operating system's credential store:

- Keychain on macOS.
- Secret Service on Linux.
- Windows Credential Manager on Windows.

Access and refresh tokens are saved together in one credential-store entry.
The API address the session was created for is saved beside them. A session
file that names a different address is not resumed; sign in again to replace
it. Sessions saved before this record existed resume only on the default
Proton API address. Signing in removes the previous session's tokens and key
passphrase before saving anything of the new one, so a save that fails part
way leaves no session to resume, rather than old tokens beside a new address.
A sign-in that fails after Proton accepted the password, at the second factor,
the mailbox password or saving the session, asks Proton to end the session it
opened, for up to 10 seconds, instead of leaving it valid on the account with
nothing using it.
If the credential store is locked or rejects a token refresh, the request
reports that Ruston Mail could not access the saved session, and the desktop
stays signed in so the next request can succeed once the store is unlocked.
The new tokens remain in memory, but reopening the app may require signing in
again.

Non-secret session metadata is stored in `ruston-core`'s platform config
directory under the `ruston-mail` storage name. Besides the session's UID and
API address, it records the account's Proton user ID, which names the local
cache, so cached listings and local search open that cache without contacting
Proton. On Unix, its session directory and file use modes `0700` and `0600`. The desktop and CLI share the
`ruston` profile by default. Use CLI `--profile` for a separate session; an
earlier CLI `default` session is still accessible with
`--profile default`. Profile names must be a single path component; empty names
and names with path separators are rejected.
Signing out of the shared profile affects both frontends.

Ruston Mail keeps `settings.json` in its own platform config directory; a demo
process keeps `settings-demo.json` beside it instead, so nothing changed while
trying the demo reaches a real account. It
contains preferences, the last folder, the window's size and place on screen,
whether it was maximized, and pane widths. When a
session ends, a remembered folder or label you created is replaced by the
Inbox, so its name never reaches the next account to sign in. It does not
contain account passwords or session tokens. A missing, incomplete, or damaged
settings file falls back to defaults. One that is there but cannot be read is
first renamed with `.unreadable` added, such as `settings.json.unreadable`, so
the defaults saved instead replace nothing; if it cannot be renamed, it is left
as it is and that run saves no preferences. A save that fails, on a full or
read-only disk for example, is tried again with the next change and when Ruston
Mail quits.

Signing out first removes local session metadata and credentials, then tries
to revoke the server session for up to 30 seconds. If the access token has
expired, the revocation refreshes it in memory first; those tokens are never
saved. `ruston-cli logout` reports the session as revoked only when Proton
confirms it. Remote failures or timeouts do not prevent local cleanup. Cleanup runs outside the asynchronous workers
under the profile lock, attempts every removal, and continues if its caller is
cancelled after cleanup starts. If local storage rejects a removal, the CLI
reports an error and the desktop warns that credentials may remain on the device;
the desktop still clears its in-memory mailbox and draft. Closing the desktop
window while it signs out waits for the sign-out to finish, since quitting
could stop the cleanup half done and leave the session saved; **Close now**
closes at once.
A pending logout checks the saved session identity before deleting it, protecting
a later login in the same profile. When the CLI cannot resume the saved session
(offline, revoked, after a password change, or bound to another API address),
`logout` still removes it from the device and says that Proton was not asked to
end it.

macOS may ask for Keychain access again after Ruston Mail is rebuilt or updated.
Packaging the application into `Ruston Mail.app` via `./packaging/macos/package.sh`
applies an ad-hoc code signature (`codesign -s -`) with bundle identifier
`com.luiscuellar.ruston-mail`. An ad-hoc signature identifies one build only:
the same build keeps its Keychain authorization, but a new build or release asks
again. Only a Developer ID signature keeps one identity across versions.
The signature turns on the hardened runtime, so macOS does not load libraries
injected with `DYLD_INSERT_LIBRARIES` into the process that reads the saved
session from the Keychain.

On Windows, everything stays in the user's profile, which Windows' default
access rules keep to that user, the system and administrators:

| Data | Location |
| --- | --- |
| Tokens and key passphrase | Credential Manager, generic credentials named `<profile>:…ruston-mail` |
| Session metadata | `%APPDATA%\ruston-mail\config\sessions` |
| Desktop settings | `%APPDATA%\Ruston Mail\config\settings.json` |
| CLI cache | `%LOCALAPPDATA%\ruston-mail\cache\accounts` |
| Single-instance lock | `%LOCALAPPDATA%\Ruston Mail\data\activation` |

`%APPDATA%` follows a roaming profile to other computers, and so do the
Credential Manager entries, which use enterprise persistence. Any program
running as the same user can read those entries; Windows has no per-app access
check like the macOS Keychain's. Uninstalling removes the program, its Start
menu shortcut, the email-link registration and the `PATH` entry, and leaves
the rest: sign out first to remove the session from Credential Manager, then
delete the folders above to remove settings and the cache.

## Attachments

Desktop attachments are downloaded only after **Save** or a confirmed **Save as…**
destination. **Save** uses the system Downloads folder; **Save as…** uses the
exact path chosen by the user, and cancelling its dialog starts no download.
The CLI saves them in `--output-dir`, or the current directory
by default. Both reduce sender-provided paths to a plain file name. Invisible
direction marks are removed, so a name cannot disguise its real extension.
Characters no file system accepts become `_`, a name longer than 240 bytes is
cut keeping its extension, leading dots and trailing dots and spaces are
removed, a name left empty becomes `attachment`, Windows device names (`CON`,
`COM1`, `LPT¹` and the like) receive an underscore prefix, and existing files
are never overwritten. On Windows,
both mark a saved attachment as downloaded from the internet, as browsers do
(a `Zone.Identifier` stream naming the Internet zone), so SmartScreen checks it
before it runs and Office opens it in Protected View. A file system without
such streams, such as FAT32, keeps the file unmarked.
Sender-provided names are sanitized before suggesting a name in the save
dialog too. Revealing a saved file sends its local path to the operating
system's file manager (a file URI over the session bus on Linux).
Attachments with a nonempty detached signature are verified against available
sender keys before their plaintext reaches a file writer. Keys Proton marks as
compromised are never used to verify attachments or message bodies, and sender
keys are fetched again after ten minutes, so a key marked compromised during a
session stops verifying. Missing usable keys, malformed signatures, and
signature mismatches fail the download; plaintext
discarded on this path is zeroized. This check is shared by desktop and CLI
downloads, including bulk downloads and EML export. Unsigned attachments remain
downloadable without a signature-based authenticity guarantee. Body signature
status is shown beside the sender's address in the desktop reader: a closed lock
means the body signature verified against an available sender key; an open lock
means unsigned or unverified, and a red warning means invalid: altered, or
made by a key that is not one of the sender's. Hover over the
icon for the specific result. The sender's email domain does not determine this
status; it does not authenticate the sender's identity independently
or describe an attachment's verification status.
The application's own HTTP safety limits are 32 MiB for ordinary responses and
64 MiB for an encrypted attachment response; they are not Proton Mail's
attachment quotas. A received attachment can be opened up to 50 MiB, the most
decryption accepts; one Proton reports as larger than the response limit is
refused without being downloaded.
The CLI downloads and saves `--all` attachments one at a time. If a later
attachment fails, files saved earlier in that command remain in the destination.
The core API that returns all attachment bytes has a 128 MiB total limit.

New outgoing local attachments have plaintext limits of 32 MiB per file and
128 MiB combined. Ruston checks sizes before creating a draft and bounds the
actual read even if a file changes. It reads and encrypts files on blocking
workers, allowing at most two preparations or uploads per process. Plaintext
buffers are zeroized after encryption; encrypted request buffers are shared
across HTTP retries. Cancelling a send can leave a started blocking operation
running until it finishes, but that operation retains its admission slot and
cannot start uploading after cancellation. Forwarded attachments already on
Proton are not read from local files and do not count toward these limits.

## CLI EML export

The live CLI `export --out DIRECTORY` saves decrypted mail as reconstructed
MIME `.eml` files. Each file contains the selected message body and all
attachments available through the message API, including inline attachments.
HTML bodies are sanitized before export. The export is not a byte-for-byte
copy of the received message: the original MIME structure, some original
headers, and inline Content-ID references are not preserved. The offline demo
exports only fictional sample text and does not include attachment bytes.

Exported files contain plaintext mail and attachments. New files can be opened
only by you: on Unix they use mode `0600`, and on Windows an access list for
you, administrators and the system, as your profile's folders have, that takes
nothing from the destination folder, so a shared folder does not share them.
Protect the destination directory's backups on all systems.
The command never replaces an existing file. In live export, each file is
written under a hidden temporary name (`.message-….eml.tmp`) and moved to its
final name only once complete, so a file under its final name is always
whole, and running the export again skips it. A message that cannot be
decrypted, or whose attachments cannot, is skipped and its temporary file
removed. A failed request or write removes the current temporary file and
stops; files exported earlier remain. An interrupted process may leave a
hidden temporary file holding plaintext, which the next export into that
directory replaces when it reaches that message. Files that an older version
left incomplete count as exported.

## Network connections

Ruston Mail uses the system's proxy settings: the HTTP and HTTPS proxies
configured in macOS network settings or Windows Internet Options, and on every
platform the `HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY`, and `NO_PROXY`
environment variables, which take precedence. Automatic proxy configuration
(PAC) is not read. A proxy sees which Proton servers Ruston Mail contacts and
when; the HTTPS traffic itself stays encrypted end to end between Ruston Mail
and Proton.

## HTTP diagnostics

`RUSTON_DEBUG_HTTP=1` prints Proton request methods, paths, body kinds, status
codes, response sizes, and timings to standard error. It does not print
credentials, tokens, headers, request bodies, or response bodies.
Request diagnostics use only the prepared URL's path; they omit its query
entirely, including parameter names and values, as well as its origin, URL
credentials, and fragment. Search terms and filters are still sent to Proton
but do not appear in these events, including retry attempts. Core also removes
the attached URL when converting a `reqwest` error, so displayed HTTP errors
retain their failure kind and cause without that URL. The CLI also prints the
causes beneath an error, such as `tcp connect error: Connection refused`; they
come from the network stack and do not include the request's path or query.

Paths may contain resource IDs. This policy covers core's HTTP request events
and its conversion of HTTP errors; it does not anonymize other core targets,
server-provided error text, or dependency diagnostics enabled by the CLI's
broader verbosity levels or `RUST_LOG`.
