# Privacy and local data

Ruston Mail is an unofficial client and has not received an independent
security audit. This page describes what the current code does.

## Mail content

Ruston Mail talks directly to Proton through `ruston-core`; it has no separate
application server. `ruston-core` handles authentication, key unlocking, and
mail cryptography.

Mail and mailbox lists stay in memory while the app runs. In addition to the
conversation on screen, up to eight previously opened conversations can remain
in memory for quick backtracking. Ruston Mail does not write a persistent mail
cache to disk.

HTML messages do not run in a browser view. They are sanitized, parsed, and
drawn with native UI elements. Scripts do not run, and remote images are never
requested. This also blocks tracking pixels.

Links open in the system browser. Ruston Mail shows the real destination before
opening it by default; this prompt can be disabled in Settings.

## Saved session and settings

The Proton password is not saved. `ruston-core` stores the access token, refresh
token, and key passphrase in the operating system's credential store:

- Keychain on macOS.
- Secret Service on Linux.
- Windows Credential Manager on Windows.

Non-secret session metadata is stored in `ruston-core`'s platform config
directory. On Unix, its session directory and file use modes `0700` and `0600`.

Ruston Mail keeps `settings.json` in its own platform config directory. It
contains preferences, the last folder, window size, and pane widths. It does not
contain account passwords or session tokens. A missing or damaged settings file
falls back to defaults.

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
