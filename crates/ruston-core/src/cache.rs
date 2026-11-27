//! The local SQLite cache: message metadata, the full-text index of
//! decrypted bodies, and the cursor sync goes on from.

use crate::api::events::{EventBatch, MessageEvent, action};
use crate::error::{Error, Result};
use crate::model::message::MessageMetadata;
use crate::session::validate_profile_name;
use rusqlite::{Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;

mod schema;

use schema::{
    FTS_COLUMNS, ensure_fts_secure_delete, migrate, needs_migration, needs_migration_to_write,
};

/// Any failure in the cache, its database or its files, as an
/// [`Error::Cache`] with its message.
fn cache_error<E: std::fmt::Display>(e: E) -> Error {
    Error::Cache(e.to_string())
}

#[cfg(unix)]
fn secure_cache_dir(dir: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::create_dir_all(dir).map_err(cache_error)?;
    let metadata = std::fs::symlink_metadata(dir).map_err(cache_error)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(Error::Cache("cache directory must not be a symlink".into()));
    }
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(cache_error)?;
    if std::fs::symlink_metadata(dir)
        .map_err(cache_error)?
        .permissions()
        .mode()
        & 0o777
        != 0o700
    {
        return Err(Error::Cache("cache directory is not private".into()));
    }
    Ok(())
}

/// Where SQLite opens the existing database at `path` with `flags`, and the
/// flags it opens it with: on Unix, from a private folder, with every link
/// resolved, and NOFOLLOW, which refuses a symlink anywhere in the path.
#[cfg(unix)]
fn read_only_target(path: &Path, flags: OpenFlags) -> Result<(PathBuf, OpenFlags)> {
    private_parent(path)?;
    let resolved = std::fs::canonicalize(path).map_err(cache_error)?;
    Ok((resolved, flags | OpenFlags::SQLITE_OPEN_NOFOLLOW))
}

/// Where SQLite opens the existing database at `path` with `flags`: there.
#[cfg(not(unix))]
fn read_only_target(path: &Path, flags: OpenFlags) -> Result<(PathBuf, OpenFlags)> {
    Ok((path.to_path_buf(), flags))
}

/// Where SQLite opens or makes the database at `path`, and the flags it
/// opens it with: on Unix, a file only its owner can open, given with every
/// link resolved, since NOFOLLOW refuses a symlink anywhere in the path.
#[cfg(unix)]
fn writable_target(path: &Path) -> Result<(PathBuf, OpenFlags)> {
    let flags = OpenFlags::default() | OpenFlags::SQLITE_OPEN_NOFOLLOW;
    if path == Path::new(":memory:") {
        return Ok((path.to_path_buf(), flags));
    }
    secure_cache_file(path)?;
    let resolved = std::fs::canonicalize(path).map_err(cache_error)?;
    Ok((resolved, flags))
}

/// Where SQLite opens or makes the database at `path`: there, with SQLite's
/// default flags.
#[cfg(not(unix))]
fn writable_target(path: &Path) -> Result<(PathBuf, OpenFlags)> {
    Ok((path.to_path_buf(), OpenFlags::default()))
}

#[cfg(unix)]
fn secure_cache_file(path: &Path) -> Result<()> {
    use std::fs::{OpenOptions, Permissions};
    use std::io::ErrorKind;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

    private_parent(path)?;

    let mut options = OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    match options.open(path) {
        Ok(file) => file
            .set_permissions(Permissions::from_mode(0o600))
            .map_err(cache_error)?,
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            let metadata = std::fs::symlink_metadata(path).map_err(cache_error)?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(Error::Cache("cache database must be a regular file".into()));
            }
            let file = OpenOptions::new()
                .read(true)
                .open(path)
                .map_err(cache_error)?;
            let opened = file.metadata().map_err(cache_error)?;
            if metadata.dev() != opened.dev() || metadata.ino() != opened.ino() {
                return Err(Error::Cache("cache database changed while opening".into()));
            }
            file.set_permissions(Permissions::from_mode(0o600))
                .map_err(cache_error)?;
        }
        Err(error) => return Err(cache_error(error)),
    }
    let metadata = std::fs::symlink_metadata(path).map_err(cache_error)?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o777 != 0o600 {
        return Err(Error::Cache("cache database is not private".into()));
    }
    Ok(())
}

/// Refuses a cache whose folder is not private to this user or is a
/// symlink, where someone else could put a file of their own in its place.
#[cfg(unix)]
fn private_parent(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let dir = std::fs::symlink_metadata(parent).map_err(cache_error)?;
    if !dir.is_dir() || dir.file_type().is_symlink() || dir.permissions().mode() & 0o777 != 0o700 {
        return Err(Error::Cache(format!(
            "cache directory {} must be private (mode 0700) and not a symlink",
            parent.display()
        )));
    }
    Ok(())
}

/// The searchable names and addresses of everyone a message names: who sent
/// it and whom it went to, copies included.
fn participants(m: &MessageMetadata) -> String {
    std::iter::once(&m.sender)
        .chain(&m.to_list)
        .chain(&m.cc_list)
        .chain(&m.bcc_list)
        .flat_map(|r| [r.name.as_str(), r.address.as_str()])
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

const DELETE_INDEX_SQL: &str = "DELETE FROM msg_fts WHERE rowid IN (
    SELECT fts_rowid FROM msg_fts_rows WHERE message_id=?1)";
/// FTS5 rewrites a whole document, body included, for any change to it, so a
/// row whose headers are unchanged is left alone.
const UPDATE_INDEX_SQL: &str = "UPDATE msg_fts SET subject=?2, participants=?3 WHERE rowid IN (
    SELECT fts_rowid FROM msg_fts_rows WHERE message_id=?1)
    AND (subject IS NOT ?2 OR participants IS NOT ?3)";

/// The SQL that writes a message's metadata to one pair of tables, its row
/// and its labels.
struct MetadataSql {
    /// Inserts the message's row, or replaces what an earlier one held.
    upsert_message: &'static str,
    /// Takes off every label the message had.
    clear_labels: &'static str,
    /// Puts one label on the message.
    insert_label: &'static str,
}

/// The cache's own tables.
const CACHED_SQL: MetadataSql = MetadataSql {
    upsert_message: "INSERT INTO messages(id, time, unread, json) VALUES(?1, ?2, ?3, ?4)
         ON CONFLICT(id) DO UPDATE SET
         time=excluded.time, unread=excluded.unread, json=excluded.json",
    clear_labels: "DELETE FROM message_labels WHERE message_id=?1",
    insert_label: "INSERT OR IGNORE INTO message_labels(message_id, label_id, time) VALUES(?1, ?2, ?3)",
};

/// The tables a resync stages its listing in, which replace the cache's
/// once it is complete.
const STAGED_SQL: MetadataSql = MetadataSql {
    upsert_message: "INSERT INTO resync_messages(id, time, unread, json) VALUES(?1, ?2, ?3, ?4)
         ON CONFLICT(id) DO UPDATE SET
         time=excluded.time, unread=excluded.unread, json=excluded.json",
    clear_labels: "DELETE FROM resync_labels WHERE message_id=?1",
    insert_label: "INSERT OR IGNORE INTO resync_labels(message_id, label_id, time) VALUES(?1, ?2, ?3)",
};

/// The statements a connection keeps compiled: every one the cache runs for
/// each message, in either pair of tables, and for each list, count and
/// search, with room to spare.
const KEPT_STATEMENTS: usize = 32;

/// Runs `sql` with `params`, compiled the first time and kept with the
/// connection. A resync, a backfill and a batch of events run the same few
/// statements for every message, and compiling one costs more than running
/// it.
fn execute(conn: &Connection, sql: &str, params: impl rusqlite::Params) -> Result<usize> {
    conn.prepare_cached(sql)
        .and_then(|mut statement| statement.execute(params))
        .map_err(cache_error)
}

/// Writes `m`'s row and labels with `sql`, replacing the labels it had.
fn write_metadata(conn: &Connection, sql: &MetadataSql, m: &MessageMetadata) -> Result<()> {
    let json = serde_json::to_string(m)?;
    execute(
        conn,
        sql.upsert_message,
        rusqlite::params![m.id, m.time, m.unread, json],
    )?;
    execute(conn, sql.clear_labels, [&m.id])?;
    for label in &m.label_ids {
        execute(
            conn,
            sql.insert_label,
            rusqlite::params![m.id, label, m.time],
        )?;
    }
    Ok(())
}

fn upsert_metadata(conn: &Connection, m: &MessageMetadata) -> Result<()> {
    write_metadata(conn, &CACHED_SQL, m)
}

fn write_message(conn: &Connection, m: &MessageMetadata) -> Result<()> {
    upsert_metadata(conn, m)?;
    execute(
        conn,
        UPDATE_INDEX_SQL,
        rusqlite::params![m.id, m.subject, participants(m)],
    )?;
    Ok(())
}

fn replace_index(conn: &Connection, m: &MessageMetadata, body: Option<&str>) -> Result<()> {
    upsert_metadata(conn, m)?;
    execute(conn, DELETE_INDEX_SQL, [&m.id])?;
    if let Some(body) = body {
        execute(
            conn,
            "INSERT INTO msg_fts(id, subject, body, participants) VALUES(?1, ?2, ?3, ?4)",
            rusqlite::params![m.id, m.subject, body, participants(m)],
        )?;
        execute(
            conn,
            "UPDATE msg_fts_rows SET as_text=1 WHERE message_id=?1",
            [&m.id],
        )?;
    }
    Ok(())
}

fn remove_message(conn: &Connection, id: &str) -> Result<()> {
    // The messages_delete_fts trigger removes the indexed body in this transaction.
    let deleted = execute(conn, "DELETE FROM messages WHERE id=?1", [id])?;
    if deleted == 0 {
        // A stray index row can still exist without cached metadata.
        execute(conn, DELETE_INDEX_SQL, [id])?;
    }
    execute(conn, "DELETE FROM message_labels WHERE message_id=?1", [id])?;
    Ok(())
}

/// Applies one event of a batch, adding what it changed to `counts`.
fn apply_message_event(
    conn: &Connection,
    event: &MessageEvent,
    counts: &mut BatchCounts,
) -> Result<()> {
    match (event.action, &event.message) {
        (action::DELETE, _) => {
            remove_message(conn, &event.id)?;
            counts.deleted += 1;
        }
        (action::CREATE, Some(message)) => {
            write_message(conn, message)?;
            counts.created += 1;
        }
        (action::UPDATE, message) => {
            // Events carry metadata, not the decrypted body.
            conn.execute(DELETE_INDEX_SQL, [&event.id])
                .map_err(cache_error)?;
            if let Some(message) = message {
                write_message(conn, message)?;
                counts.updated += 1;
            }
        }
        (action::UPDATE_FLAGS, Some(message)) => {
            upsert_metadata(conn, message)?;
            counts.updated += 1;
            counts.flags_updated += 1;
        }
        // Unknown actions, and events without the message they need, change
        // nothing; the batch's cursor still moves past them.
        _ => {}
    }
    Ok(())
}

/// The `meta` key the sync cursor is kept under.
const CURSOR_KEY: &str = "last_event_id";

/// The sync cursor, or `None` before the first sync stores one.
fn read_cursor(conn: &Connection) -> Result<Option<String>> {
    conn.query_row("SELECT value FROM meta WHERE key=?1", [CURSOR_KEY], |row| {
        row.get(0)
    })
    .optional()
    .map_err(cache_error)
}

/// Stores `cursor` as the sync cursor, in place of any other.
fn write_cursor(conn: &Connection, cursor: &str) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO meta(key, value) VALUES(?1, ?2)",
        [CURSOR_KEY, cursor],
    )
    .map_err(cache_error)?;
    Ok(())
}

/// The account a cache is bound to, if it is bound yet.
fn account_owner(conn: &Connection) -> Result<Option<String>> {
    conn.query_row(
        "SELECT value FROM meta WHERE key='account_identity_v1'",
        [],
        |row| row.get(0),
    )
    .optional()
    .map_err(cache_error)
}

/// Authenticated account identity, independent of session tokens and addresses.
///
/// A persisted format: its JSON is hashed into the database's file name and
/// stored inside it as `account_identity_v1`. Renaming, reordering or adding a
/// field starts every account on an empty cache and leaves the old one, with
/// its decrypted bodies, orphaned on disk.
#[derive(Clone, Serialize)]
pub(crate) struct CacheIdentity {
    base_url: String,
    user_id: String,
}

impl CacheIdentity {
    pub(crate) fn new(base_url: &str, user_id: &str) -> Result<Self> {
        if base_url.trim().is_empty() || user_id.trim().is_empty() {
            return Err(Error::Cache(
                "cache requires an authenticated account identity".into(),
            ));
        }
        Ok(Self {
            base_url: base_url.to_owned(),
            user_id: user_id.to_owned(),
        })
    }

    pub(crate) fn user_id(&self) -> &str {
        &self.user_id
    }

    fn serialized(&self) -> Result<String> {
        Ok(serde_json::to_string(self)?)
    }
}

/// What one event batch changed in the cache.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BatchCounts {
    pub(crate) created: usize,
    /// Every updated message, flag and label changes included.
    pub(crate) updated: usize,
    /// Of `updated`, messages whose flags or labels alone changed: their
    /// indexed bodies are still current.
    pub(crate) flags_updated: usize,
    pub(crate) deleted: usize,
}

impl std::ops::AddAssign for BatchCounts {
    fn add_assign(&mut self, other: Self) {
        self.created += other.created;
        self.updated += other.updated;
        self.flags_updated += other.flags_updated;
        self.deleted += other.deleted;
    }
}

/// A local metadata cache. Client operations use an account-specific database.
pub struct Cache {
    conn: Connection,
}

/// A label's cached messages, newest first, as `Cache::list` reads them.
fn list_sql(unread_only: bool) -> String {
    format!(
        "SELECT m.json FROM messages m JOIN message_labels l ON m.id=l.message_id \
         WHERE l.label_id=?1 {} ORDER BY l.time DESC LIMIT ?2 OFFSET ?3",
        if unread_only { "AND m.unread=1" } else { "" }
    )
}

/// The messages whose index entry matches an FTS5 query, best first, as
/// `Cache::search` reads them.
const SEARCH_SQL: &str = "SELECT m.json FROM msg_fts JOIN messages m ON m.id=msg_fts.id \
     WHERE msg_fts MATCH ?1 ORDER BY rank LIMIT ?2";

/// The FTS5 query for the words of `query`: each one a quoted phrase, with
/// its double quotes taken out, so that every word must be there and none
/// is read as FTS5's own syntax. `None` when there is no word.
fn match_expression(query: &str) -> Option<String> {
    let phrases: Vec<String> = query
        .split_whitespace()
        .map(|word| format!("\"{}\"", word.replace('"', "")))
        .collect();
    (!phrases.is_empty()).then(|| phrases.join(" "))
}

/// The messages a query of their metadata JSON returns, in its order. A row
/// whose JSON no longer reads as metadata is left out rather than failing
/// the rest.
fn read_metadata(
    statement: &mut rusqlite::Statement<'_>,
    params: impl rusqlite::Params,
) -> Result<Vec<MessageMetadata>> {
    let rows: Vec<String> = statement
        .query_map(params, |row| row.get(0))
        .and_then(|rows| rows.collect())
        .map_err(cache_error)?;
    Ok(rows
        .iter()
        .filter_map(|json| serde_json::from_str(json).ok())
        .collect())
}

/// The folder that holds the caches, created on first use.
fn cache_root() -> Result<PathBuf> {
    let dir = cache_root_path()?;
    create_cache_dir(&dir)?;
    Ok(dir)
}

/// Where the caches are kept, whether or not anything is there yet.
fn cache_root_path() -> Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", crate::STORAGE_NAME)
        .ok_or_else(|| Error::Cache("cannot resolve cache dir".into()))?;
    Ok(dirs.cache_dir().to_path_buf())
}

/// Creates a cache folder, private to this user.
#[cfg(unix)]
fn create_cache_dir(dir: &Path) -> Result<()> {
    secure_cache_dir(dir)
}

/// Creates a cache folder in the user's profile, which Windows keeps private
/// to them.
#[cfg(not(unix))]
fn create_cache_dir(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).map_err(cache_error)
}

/// A label's cached total and unread count, as `Cache::count` reads them.
const COUNT_SQL: &str = "SELECT COUNT(*), COALESCE(SUM(m.unread),0) FROM messages m \
     JOIN message_labels l ON m.id=l.message_id WHERE l.label_id=?1";

/// How many messages a label holds in the cache, which its index answers
/// without reading a message.
const LABEL_COUNT_SQL: &str = "SELECT COUNT(*) FROM message_labels WHERE label_id=?1";

impl Cache {
    pub(crate) fn default_account_path(profile: &str, identity: &CacheIdentity) -> Result<PathBuf> {
        validate_profile_name(profile)?;
        Self::account_path(&cache_root()?, profile, identity)
    }

    fn account_path(root: &Path, profile: &str, identity: &CacheIdentity) -> Result<PathBuf> {
        let path = Self::account_file(root, profile, identity)?;
        let accounts = root.join("accounts");
        for directory in [&accounts, &accounts.join(profile)] {
            create_cache_dir(directory)?;
        }
        Ok(path)
    }

    /// Where an account's cache is under `root`, creating nothing.
    fn account_file(root: &Path, profile: &str, identity: &CacheIdentity) -> Result<PathBuf> {
        validate_profile_name(profile)?;
        // A fixed-size lowercase name also avoids path separators and collisions
        // caused by case-insensitive filesystems. Verify the full identity in SQLite.
        let digest = Sha256::digest(identity.serialized()?.as_bytes());
        Ok(root
            .join("accounts")
            .join(profile)
            .join(format!("{digest:x}.db")))
    }

    /// Where an account's cache is, if one was ever made, creating nothing.
    pub(crate) fn existing_account_path(
        profile: &str,
        identity: &CacheIdentity,
    ) -> Result<PathBuf> {
        Self::account_file(&cache_root_path()?, profile, identity)
    }

    /// An account's cache opened only to read it, for a program that keeps
    /// no mail on disk itself: nothing is created, migrated, bound or
    /// tightened. `None` when there is none for the account, or it was never
    /// bound to one; one that needs migrating, which only a writer may do,
    /// or that belongs to another account, is an error.
    pub(crate) fn open_existing_for_account(
        path: &Path,
        identity: &CacheIdentity,
    ) -> Result<Option<Cache>> {
        let metadata = match std::fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(cache_error(error)),
        };
        if !metadata.is_file() {
            return Err(Error::Cache("cache database must be a regular file".into()));
        }
        let (sqlite_path, flags) = read_only_target(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_URI
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        let conn = Connection::open_with_flags(&sqlite_path, flags).map_err(cache_error)?;
        conn.busy_timeout(Duration::from_secs(30))
            .map_err(cache_error)?;
        conn.set_prepared_statement_cache_capacity(KEPT_STATEMENTS);
        if needs_migration(&conn)? {
            return Err(Error::Cache(
                "the local index needs `ruston-cli sync` or `ruston-cli index` to update it".into(),
            ));
        }
        match account_owner(&conn)? {
            Some(owner) if owner == identity.serialized()? => Ok(Some(Cache { conn })),
            Some(_) => Err(Error::Cache("cache belongs to a different account".into())),
            None => Ok(None),
        }
    }

    /// Open a database only for its authenticated account. The binding is
    /// immutable: another account, or unbound existing data, is rejected.
    pub(crate) fn open_for_account(path: &Path, identity: &CacheIdentity) -> Result<Cache> {
        let cache = Self::open(path)?;
        let expected = identity.serialized()?;
        // A binding never changes once made, so finding this account's needs
        // no lock, and opening a bound cache never waits for a writer.
        if account_owner(&cache.conn)?.is_some_and(|owner| owner == expected) {
            return Ok(cache);
        }
        let tx = Transaction::new_unchecked(&cache.conn, TransactionBehavior::Immediate)
            .map_err(cache_error)?;
        match account_owner(&tx)? {
            Some(owner) if owner == expected => {}
            Some(_) => return Err(Error::Cache("cache belongs to a different account".into())),
            None => {
                let has_data: bool = tx
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM meta UNION ALL SELECT 1 FROM messages
                     UNION ALL SELECT 1 FROM message_labels UNION ALL SELECT 1 FROM msg_fts)",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(cache_error)?;
                if has_data {
                    return Err(Error::Cache(
                        "cache has data without a verified account identity; rebuild it".into(),
                    ));
                }
                tx.execute(
                    "INSERT INTO meta(key, value) VALUES('account_identity_v1', ?1)",
                    [expected],
                )
                .map_err(cache_error)?;
            }
        }
        tx.commit().map_err(cache_error)?;
        Ok(cache)
    }

    /// Opens the cache database at `path`, creating it when it is not there
    /// and bringing its schema up to date.
    /// This low-level API does not verify ownership; Client operations do so
    /// before returning a cache for use.
    /// On Unix, the parent must be a private directory; existing database files
    /// are restricted to mode `0600` before SQLite opens them.
    pub fn open(path: &Path) -> Result<Cache> {
        let (sqlite_path, flags) = writable_target(path)?;
        let conn = Connection::open_with_flags(&sqlite_path, flags).map_err(cache_error)?;
        // Another CLI process may hold the write lock during sync or migration.
        conn.busy_timeout(Duration::from_secs(30))
            .map_err(cache_error)?;
        conn.set_prepared_statement_cache_capacity(KEPT_STATEMENTS);
        // Deleted and replaced rows, decrypted bodies among them, are
        // overwritten instead of lingering in the file's free space.
        conn.execute_batch("PRAGMA secure_delete=ON")
            .map_err(cache_error)?;
        // A cache that needs no migration opens without the write lock, so a
        // command that only reads never waits for another process's sync.
        if needs_migration_to_write(&conn)? {
            migrate(&conn)?;
        }
        Ok(Cache { conn })
    }

    /// The cursor sync goes on from, or `None` before a sync has stored one.
    pub fn last_event_id(&self) -> Result<Option<String>> {
        read_cursor(&self.conn)
    }

    /// Claim an uninitialized cursor without replacing another sync's cursor.
    /// Returns whether this call stored `id`: `false` means another sync had
    /// already stored one.
    pub(crate) fn initialize_cursor(&self, id: &str) -> Result<bool> {
        Ok(self
            .conn
            .execute(
                "INSERT OR IGNORE INTO meta(key, value) VALUES(?1, ?2)",
                [CURSOR_KEY, id],
            )
            .map_err(cache_error)?
            == 1)
    }

    /// A write transaction, held only while the cursor is still `expected`,
    /// the one a request was made under: a sync that moved it since may have
    /// deleted what the request fetched, so writing it would bring that mail
    /// back. `changed` says what was under way when it moved.
    fn transaction_at_cursor(
        &self,
        expected: Option<&str>,
        changed: &'static str,
    ) -> Result<Transaction<'_>> {
        // Claim the SQLite write lock before checking; other processes cannot
        // advance the cursor between this check and the commit.
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)
            .map_err(cache_error)?;
        if read_cursor(&tx)?.as_deref() != expected {
            return Err(Error::cache_changed(changed));
        }
        Ok(tx)
    }

    /// Whether a body of the message `id` is indexed as text, as this
    /// version indexes one; a body an older version indexed is not.
    pub(crate) fn has_indexed_body(&self, id: &str) -> Result<bool> {
        self.conn
            .prepare_cached(
                "SELECT EXISTS(SELECT 1 FROM msg_fts_rows WHERE message_id=?1 AND as_text=1)",
            )
            .and_then(|mut statement| statement.query_row([id], |row| row.get(0)))
            .map_err(cache_error)
    }

    /// Commit a downloaded page only if sync has not changed since its request.
    pub(crate) fn store_metadata_page(
        &self,
        expected_cursor: Option<&str>,
        messages: &[MessageMetadata],
    ) -> Result<()> {
        let tx = self
            .transaction_at_cursor(expected_cursor, "sync cursor changed during cache download")?;
        for m in messages {
            write_message(&tx, m)?;
        }
        tx.commit().map_err(cache_error)
    }

    /// Commit a downloaded body, or drop the body of a message that is gone
    /// or cannot be opened, only while the cursor captured before listing the
    /// page still matches. Both outcomes update metadata and the body index
    /// atomically without changing the cursor.
    pub(crate) fn store_index_result(
        &self,
        expected_cursor: Option<&str>,
        m: &MessageMetadata,
        body: Option<&str>,
    ) -> Result<()> {
        let tx = self
            .transaction_at_cursor(expected_cursor, "sync cursor changed during cache download")?;
        replace_index(&tx, m, body)?;
        tx.commit().map_err(cache_error)
    }

    /// Apply a server batch and its cursor only if the cursor we fetched from
    /// still owns the cache. A stale process leaves both data and cursor intact.
    pub(crate) fn apply_event_batch(
        &self,
        previous_cursor: &str,
        batch: &EventBatch,
    ) -> Result<BatchCounts> {
        let tx = self.transaction_at_cursor(
            Some(previous_cursor),
            "sync cursor changed during incremental sync",
        )?;
        let mut counts = BatchCounts::default();
        for event in &batch.messages {
            apply_message_event(&tx, event, &mut counts)?;
        }
        write_cursor(&tx, &batch.event_id)?;
        tx.commit().map_err(cache_error)?;
        Ok(counts)
    }

    /// A page of the messages cached in a label, newest first: only the
    /// unread ones when `unread_only`.
    pub fn list(
        &self,
        label_id: &str,
        unread_only: bool,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<MessageMetadata>> {
        let mut statement = self
            .conn
            .prepare_cached(&list_sql(unread_only))
            .map_err(cache_error)?;
        read_metadata(&mut statement, rusqlite::params![label_id, limit, offset])
    }

    /// How many messages are cached in a label, and how many of those are
    /// unread.
    pub fn count(&self, label_id: &str) -> Result<(i64, i64)> {
        self.conn
            .prepare_cached(COUNT_SQL)
            .and_then(|mut statement| {
                statement.query_row([label_id], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
                })
            })
            .map_err(cache_error)
    }

    /// How many messages [`Self::list`] has to show of a label: all of them,
    /// or the unread ones. All of them are counted on the label's index
    /// alone; telling the unread means reading each message's row.
    pub(crate) fn listed_count(&self, label_id: &str, unread_only: bool) -> Result<i64> {
        if unread_only {
            return Ok(self.count(label_id)?.1);
        }
        self.conn
            .prepare_cached(LABEL_COUNT_SQL)
            .and_then(|mut statement| statement.query_row([label_id], |row| row.get(0)))
            .map_err(cache_error)
    }

    /// Start a full metadata rebuild without changing the visible cache.
    pub(crate) fn begin_rebuild(&self) -> Result<()> {
        self.conn
            .execute_batch(
                "CREATE TEMP TABLE IF NOT EXISTS resync_messages(
                     id TEXT PRIMARY KEY, time INTEGER, unread INTEGER, json TEXT NOT NULL);
                 CREATE TEMP TABLE IF NOT EXISTS resync_labels(
                     message_id TEXT, label_id TEXT, time INTEGER, PRIMARY KEY(message_id, label_id));
                 DELETE FROM resync_labels;
                 DELETE FROM resync_messages;",
            )
            .map_err(cache_error)
    }

    /// Stage one API page; a failed page leaves the visible cache untouched.
    pub(crate) fn stage_rebuild_page(&self, messages: &[MessageMetadata]) -> Result<()> {
        let tx = self.conn.unchecked_transaction().map_err(cache_error)?;
        for m in messages {
            write_metadata(&tx, &STAGED_SQL, m)?;
        }
        tx.commit().map_err(cache_error)
    }

    /// Replace metadata, labels, index, and cursor in one transaction, once
    /// the staged messages number `expected_total`, when the listing kept
    /// one total; returns how many there are.
    pub(crate) fn finish_rebuild(
        &self,
        previous_cursor: &str,
        new_cursor: &str,
        expected_total: Option<u32>,
    ) -> Result<usize> {
        let tx =
            self.transaction_at_cursor(Some(previous_cursor), "sync cursor changed during resync")?;
        let staged: i64 = tx
            .query_row("SELECT COUNT(*) FROM resync_messages", [], |row| row.get(0))
            .map_err(cache_error)?;
        if expected_total.is_some_and(|total| staged != i64::from(total)) {
            return Err(Error::cache_changed(
                "incomplete mailbox listing during resync",
            ));
        }

        // The index starts over. Deleting its rows one by one under FTS5's
        // secure-delete took seconds per thousand bodies, all of it holding
        // the write lock; dropping the table frees its pages, which
        // secure_delete overwrites, at once.
        tx.execute_batch(&format!(
            "DROP TABLE msg_fts_index;
             CREATE VIRTUAL TABLE msg_fts_index USING fts5({FTS_COLUMNS});
             DELETE FROM msg_fts_rows;
             DELETE FROM message_labels;
             DELETE FROM messages;
             INSERT INTO messages(id, time, unread, json)
             SELECT id, time, unread, json FROM resync_messages;
             INSERT INTO message_labels(message_id, label_id, time)
             SELECT message_id, label_id, time FROM resync_labels;",
        ))
        .map_err(cache_error)?;
        ensure_fts_secure_delete(&tx)?;
        write_cursor(&tx, new_cursor)?;
        tx.commit().map_err(cache_error)?;
        Ok(usize::try_from(staged).unwrap_or_default())
    }

    /// The messages whose indexed subject, body or people hold every word
    /// of `query`, best match first, at most `limit` of them. Nothing typed
    /// finds nothing.
    pub fn search(&self, query: &str, limit: u32) -> Result<Vec<MessageMetadata>> {
        self.matches(query, limit, limit, |_| true)
    }

    /// Like [`Self::search`], keeping only the messages `keep` accepts and
    /// reading no further once `limit` of them are kept. The matches are read
    /// one at a time, best first, so a search narrowed by what the index
    /// cannot tell, such as a sender or a date, holds no more than it returns.
    pub fn search_where(
        &self,
        query: &str,
        limit: u32,
        keep: impl FnMut(&MessageMetadata) -> bool,
    ) -> Result<Vec<MessageMetadata>> {
        // How many matches `keep` passes over is unknown: all are ranked.
        self.matches(query, u32::MAX, limit, keep)
    }

    /// The first `limit` of the `ranked` best matches of `query` that `keep`
    /// accepts. One whose metadata no longer reads is left out, as
    /// [`Self::list`] leaves it.
    fn matches(
        &self,
        query: &str,
        ranked: u32,
        limit: u32,
        mut keep: impl FnMut(&MessageMetadata) -> bool,
    ) -> Result<Vec<MessageMetadata>> {
        let Some(expression) = match_expression(query) else {
            return Ok(Vec::new());
        };
        let mut statement = self.conn.prepare_cached(SEARCH_SQL).map_err(cache_error)?;
        let mut rows = statement
            .query(rusqlite::params![expression, ranked])
            .map_err(cache_error)?;
        let mut found = Vec::new();
        while found.len() < limit as usize
            && let Some(row) = rows.next().map_err(cache_error)?
        {
            let json: String = row.get(0).map_err(cache_error)?;
            if let Ok(message) = serde_json::from_str::<MessageMetadata>(&json)
                && keep(&message)
            {
                found.push(message);
            }
        }
        Ok(found)
    }
}

/// Writes that leave the sync cursor unchecked, for setting up tests. The
/// library writes through sync, the rebuild, `store_metadata_page` and
/// `store_index_result`, which refuse data fetched under a cursor that
/// another sync has moved since.
#[cfg(test)]
impl Cache {
    /// Sets the cursor sync goes on from.
    pub(crate) fn set_last_event_id(&self, id: &str) -> Result<()> {
        write_cursor(&self.conn, id)
    }

    /// Insert or replace cached metadata and labels, keeping searchable headers
    /// current for a body that has already been indexed.
    pub(crate) fn upsert_message(&self, m: &MessageMetadata) -> Result<()> {
        let tx = self.conn.unchecked_transaction().map_err(cache_error)?;
        write_message(&tx, m)?;
        tx.commit().map_err(cache_error)?;
        Ok(())
    }

    /// Writes a message's flags and labels, leaving its indexed headers and
    /// body as they are.
    pub(crate) fn upsert_message_flags(&self, m: &MessageMetadata) -> Result<()> {
        let tx = self.conn.unchecked_transaction().map_err(cache_error)?;
        upsert_metadata(&tx, m)?;
        tx.commit().map_err(cache_error)?;
        Ok(())
    }

    /// Remove a message, its labels, and its active full-text index entry.
    pub(crate) fn delete_message(&self, id: &str) -> Result<()> {
        let tx = self.conn.unchecked_transaction().map_err(cache_error)?;
        remove_message(&tx, id)?;
        tx.commit().map_err(cache_error)?;
        Ok(())
    }

    /// Drops a message's indexed body, so a changed one is indexed again.
    pub(crate) fn invalidate_index(&self, id: &str) -> Result<()> {
        self.conn
            .execute(DELETE_INDEX_SQL, [id])
            .map_err(cache_error)?;
        Ok(())
    }

    /// Drop all cached messages and indexed bodies.
    pub(crate) fn clear(&self) -> Result<()> {
        let tx = self.conn.unchecked_transaction().map_err(cache_error)?;
        // Bulk resets do not need to materialize the compatibility view's bodies.
        tx.execute_batch(
            "DELETE FROM msg_fts_index; DELETE FROM msg_fts_rows;
             DELETE FROM message_labels; DELETE FROM messages;",
        )
        .map_err(cache_error)?;
        tx.commit().map_err(cache_error)?;
        Ok(())
    }

    /// Adds a message's decrypted body to the local search index.
    pub(crate) fn index_message(&self, m: &MessageMetadata, body: &str) -> Result<()> {
        let tx = self.conn.unchecked_transaction().map_err(cache_error)?;
        replace_index(&tx, m, Some(body))?;
        tx.commit().map_err(cache_error)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
