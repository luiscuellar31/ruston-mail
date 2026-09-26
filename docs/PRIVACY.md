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
directory and file use modes `0700` and `0600`. The desktop uses the `ruston`
profile; the CLI uses `default` unless `--profile` is given.

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

Attachments are downloaded only when selected. They go to the system Downloads
folder. Sender-provided paths are reduced to a plain file name, and existing
files are never overwritten.

## HTTP diagnostics

`RUSTON_DEBUG_HTTP=1` prints Proton request methods, paths, body kinds, status
codes, response sizes, and timings to standard error. It does not print
credentials, tokens, headers, request bodies, or response bodies.
