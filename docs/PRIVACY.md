# Privacy and local data

Ruston Mail is an unofficial client and has not received an independent
security audit. This page describes the desktop client and the separate CLI
where their local data differs.

## Mail content

Ruston Mail talks directly to Proton through `ruston-core`; it uses no remote
relay server. `ruston-core` handles authentication, key unlocking, and
mail cryptography.

In the desktop client, mail and mailbox lists stay in memory while it runs.
In addition to the conversation on screen, up to eight previously opened
conversations can remain in memory for quick backtracking. The desktop client
does not write a persistent mail cache to disk.

Desktop activation listens only on IPv4 loopback, at a randomly assigned port,
while Ruston is running. It accepts bounded email-link requests after a random
256-bit token handshake; it exposes no account or mailbox data and cannot send
mail. The private `activation` directory under Ruston's configuration directory
contains an OS lock file and an endpoint record with the port, token, and process
ID. Unix permissions are 0700 for the directory and 0600 for the files; Windows
uses the current user's application-data directory and inherited access rules.
The endpoint is removed on orderly exit and replaced after a crash. The lock
file remains for safe coordination. `activation-demo` isolates the demo.
Pending email links stay in memory through sign-in and are cleared when a
session ends or the application exits. Ruston does not log their contents.
URLs passed as arguments can appear in OS process listings, as with other
applications launched as URL handlers.

When desktop notifications are enabled, a new-mail notification can show the
sender and subject. A pending notification result is discarded during sign-out
or after the session changes. Disabling the option also invalidates pending
detection and native delivery work. Once handed to the operating system,
notifications can remain in its notification history; signing out does not
remove notifications already delivered. No email content is logged by the
notification integration. Headers are bounded and treated as literal text;
notification delivery does not load links or remote images.

The open desktop polls about once a minute with focus and every three minutes
without it, with failure backoff up to five minutes. It stops on sign-out or
application exit and does not install a background service. If notifications
are enabled, an additional bounded request reads the latest 20 Inbox message
metadata records, including read records, without downloading bodies or
attachments. Its reference IDs and timestamp stay in process memory. The first
successful snapshot is silent; new unread IDs can notify even when the unread
count does not grow. Turning the option off clears this reference.

Windows toast registration creates `Ruston Mail.lnk` in the user's Start menu
Programs folder, pointing to the running executable and carrying Ruston's
AppUserModelID. It does not replace a shortcut belonging to another application
and requires no administrator privileges. The shortcut remains when the app
closes; remove it when uninstalling a manually copied executable. macOS asks
for system authorization when notifications become active for a signed-in
account; a bare development executable does not request permission or notify.

HTML messages do not run in a browser view. They are sanitized, parsed, and
drawn with native UI elements. Scripts do not run, and remote images are never
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
terminal title, or cursor position. The default text output is filtered even
when redirected. JSON escapes these controls while retaining the original
decoded values. Explicit `--format html` and `--format raw` preserve body content
when redirected to a file or pipe; when displayed on a terminal, they also
filter controls. `messages read --output` and EML exports preserve the body
content supplied by core, including its HTML sanitization.
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
not use this cache. Once `sync` applies a deletion, it removes the active search
entry. A full message update removes its indexed body; run `index` again to
make the updated body searchable. SQLite deletion does not guarantee secure
erasure of bytes already written to disk or copied into backups.
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
Backfill and indexing also check the cursor before saving downloaded data,
including when a body download fails. If sync changes the cursor while a page
or body is being downloaded, the operation stops with a retry error and does
not restore deleted mail or overwrite the newer cached data with that result.
Run the command again to continue; earlier committed pages and messages remain
cached. These checks apply to changes already synchronized into the local
cache; run `sync` to receive remote changes.

## Saved session and settings

The Proton password is not saved. `ruston-core` stores the access token, refresh
token, and key passphrase in the operating system's credential store:

- Keychain on macOS.
- Secret Service on Linux.
- Windows Credential Manager on Windows.

Access and refresh tokens are saved together in one credential-store entry.
The API address the session was created for is saved beside them. A session
file that names a different address is not resumed; sign in again to replace
it. Sessions saved before this record existed resume only on the default
Proton API address.
If the credential store rejects a token refresh, the request reports an error.
The new tokens remain in memory, but reopening the app may require signing in
again.

Non-secret session metadata is stored in `ruston-core`'s platform config
directory under the `ruston-mail` storage name. On Unix, its session
directory and file use modes `0700` and `0600`. The desktop and CLI share the
`ruston` profile by default. Use CLI `--profile` for a separate session; an
earlier CLI `default` session is still accessible with
`--profile default`. Profile names must be a single path component; empty names
and names with path separators are rejected.
Signing out of the shared profile affects both frontends.

Ruston Mail keeps `settings.json` in its own platform config directory. It
contains preferences, the last folder, window size, and pane widths. It does not
contain account passwords or session tokens. A missing, incomplete, or damaged
settings file falls back to defaults.

Signing out first removes local session metadata and credentials, then tries
to revoke the server session for up to 30 seconds. Remote failures or timeouts
do not prevent local cleanup. Cleanup runs outside the asynchronous workers
under the profile lock, attempts every removal, and continues if its caller is
cancelled after cleanup starts. If local storage rejects a removal, the CLI
reports an error and the desktop warns that credentials may remain on the device;
the desktop still clears its in-memory mailbox and draft.
A pending logout checks the saved session identity before deleting it, protecting
a later login in the same profile.

Self-built macOS binaries may ask for Keychain access again after a rebuild.
Without a stable code signature, macOS can treat each build as a different app.
Packaging the application into `Ruston Mail.app` via `./packaging/macos/package.sh`
applies an ad-hoc code signature (`codesign -s -`) with bundle identifier
`com.luiscuellar.ruston-mail`, providing a stable identity that retains Keychain authorization.

## Attachments

Desktop attachments are downloaded only after **Save** or a confirmed **Save as…**
destination. **Save** uses the system Downloads folder; **Save as…** uses the
exact path chosen by the user, and cancelling its dialog starts no download.
The CLI saves them in `--output-dir`, or the current directory
by default. Both reduce sender-provided paths to a plain file name. Names with
invalid characters become `attachment`, Windows device names receive an
underscore prefix, and existing files are never overwritten.
Sender-provided names are sanitized before suggesting a name in the save
dialog too. Revealing a saved file sends its local path to the operating
system's file manager (a file URI over the session bus on Linux).
Attachments with a nonempty detached signature are verified against available
sender keys before their plaintext reaches a file writer. Missing usable keys,
malformed signatures, and signature mismatches fail the download; plaintext
discarded on this path is zeroized. This check is shared by desktop and CLI
downloads, including bulk downloads and EML export. Unsigned attachments remain
downloadable without a signature-based authenticity guarantee. Body signature
status is shown beside the sender's address in the desktop reader: a closed lock
means the body signature verified against an available sender key; an open lock
means unsigned or unverified, and a red warning means invalid. Hover over the
icon for the specific result. The sender's email domain does not determine this
status; it does not authenticate the sender's identity independently
or describe an attachment's verification status.
The application's own HTTP safety limits are 32 MiB for ordinary responses and
128 MiB for an encrypted attachment response; they are not Proton Mail's
attachment quotas.
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

Exported files contain plaintext mail and attachments. On Unix, new files use
mode `0600`; protect the destination directory and its backups on all systems.
The command never replaces an existing file. In live export, if an attachment
or write fails, it removes the current incomplete file and stops; files exported
earlier remain. An interrupted process may leave an incomplete file.

## HTTP diagnostics

`RUSTON_DEBUG_HTTP=1` prints Proton request methods, paths, body kinds, status
codes, response sizes, and timings to standard error. It does not print
credentials, tokens, headers, request bodies, or response bodies.
Request diagnostics use only the prepared URL's path; they omit its query
entirely, including parameter names and values, as well as its origin, URL
credentials, and fragment. Search terms and filters are still sent to Proton
but do not appear in these events, including retry attempts. Core also removes
the attached URL when converting a `reqwest` error, so displayed HTTP errors
retain their failure kind and cause without that URL.

Paths may contain resource IDs. This policy covers core's HTTP request events
and its conversion of HTTP errors; it does not anonymize other core targets,
server-provided error text, or dependency diagnostics enabled by the CLI's
broader verbosity levels or `RUST_LOG`.
