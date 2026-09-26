# Privacy and local data

Ruston Mail is an unofficial client and has not received an independent
security audit. This page describes the desktop client and the separate CLI
where their local data differs.

## Mail content

Ruston Mail talks directly to Proton through `ruston-core`; it has no separate
application server. `ruston-core` handles authentication, key unlocking, and
mail cryptography.

In the desktop client, mail and mailbox lists stay in memory while it runs.
In addition to the conversation on screen, up to eight previously opened
conversations can remain in memory for quick backtracking. The desktop client
does not write a persistent mail cache to disk.

HTML messages do not run in a browser view. They are sanitized, parsed, and
drawn with native UI elements. Scripts do not run, and remote images are never
requested. This also blocks tracking pixels.

The CLI's default text view converts HTML to readable Markdown-style text.
It omits hidden content and images, including tracking pixels, without making
remote requests. Explicit HTML and JSON output retain the sanitized HTML body.
When replying or forwarding as HTML, sender details and quoted plain text are
escaped before insertion; quoted HTML keeps the markup sanitized during reading.

Links open in the system browser. Ruston Mail shows the real destination before
opening it by default; this prompt can be disabled in Settings.

## CLI cache

The CLI uses `ruston-core`'s per-profile SQLite cache for `sync`, optional
backfills, and local `index` and `search` commands. The cache is stored under
the platform cache directory for `ruston-mail`, in `<profile>.db`.
Sync and backfill store message metadata. Running `index` also stores decrypted
message bodies and other searchable fields in SQLite full-text search. This
database is **not encrypted at rest by Ruston Mail**. The desktop client does
not use this cache. Once `sync` applies a deletion, it removes the active search
entry. A full message update removes its indexed body; run `index` again to
make the updated body searchable. SQLite deletion does not guarantee secure
erasure of bytes already written to disk or copied into backups.

When Proton requests a full refresh, sync rebuilds all cached message metadata
before replacing the current cache. If the rebuild fails, cached offline data
and the sync cursor remain available. A completed refresh clears the local
search index; run `index` again to make message bodies searchable.

## Saved session and settings

The Proton password is not saved. `ruston-core` stores the access token, refresh
token, and key passphrase in the operating system's credential store:

- Keychain on macOS.
- Secret Service on Linux.
- Windows Credential Manager on Windows.

Access and refresh tokens are saved together in one credential-store entry.
If the credential store rejects a token refresh, the request reports an error.
The new tokens remain in memory, but reopening the app may require signing in
again.

Non-secret session metadata is stored in `ruston-core`'s platform config
directory under the `ruston-mail` storage name. On Unix, its session
directory and file use modes `0700` and `0600`. The desktop and CLI share the
`ruston` profile by default. Use CLI `--profile` for a separate session; an
earlier CLI `default` session and cache are still accessible with
`--profile default`. Profile names must be a single path component; empty names
and names with path separators are rejected.
Signing out of the shared profile affects both frontends.

Ruston Mail keeps `settings.json` in its own platform config directory. It
contains preferences, the last folder, window size, and pane widths. It does not
contain account passwords or session tokens. A missing, incomplete, or damaged
settings file falls back to defaults.

Signing out tries to revoke the server session, then removes the local session
metadata and credentials.

Self-built macOS binaries may ask for Keychain access again after a rebuild.
Without a stable code signature, macOS can treat each build as a different app.
Packaging the application into `Ruston Mail.app` via `./packaging/macos/package.sh`
applies an ad-hoc code signature (`codesign -s -`) with bundle identifier
`com.luiscuellar.ruston-mail`, providing a stable identity that retains Keychain authorization.

## Attachments

Desktop attachments are downloaded only when selected and saved to the system
Downloads folder. The CLI saves them in `--output-dir`, or the current directory
by default. Both reduce sender-provided paths to a plain file name. Names with
invalid characters become `attachment`, Windows device names receive an
underscore prefix, and existing files are never overwritten.
The application's own HTTP safety limits are 32 MiB for ordinary responses and
128 MiB for an encrypted attachment response; they are not Proton Mail's
attachment quotas.
The CLI downloads and saves `--all` attachments one at a time. If a later
attachment fails, files saved earlier in that command remain in the destination.
The core API that returns all attachment bytes has a 128 MiB total limit.

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
