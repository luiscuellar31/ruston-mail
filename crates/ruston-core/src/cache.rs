//! Local SQLite cache of message metadata, indexed bodies, and the sync cursor.

use crate::api::events::{EventBatch, action};
use crate::error::{Error, Result};
use crate::model::message::MessageMetadata;
use crate::session::validate_profile_name;
use rusqlite::{Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;

fn map<E: std::fmt::Display>(e: E) -> Error {
    Error::Cache(e.to_string())
}

#[cfg(unix)]
fn secure_cache_dir(dir: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::create_dir_all(dir).map_err(map)?;
    let metadata = std::fs::symlink_metadata(dir).map_err(map)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(Error::Cache("cache directory must not be a symlink".into()));
    }
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(map)?;
    if std::fs::symlink_metadata(dir)
        .map_err(map)?
        .permissions()
        .mode()
        & 0o777
        != 0o700
    {
        return Err(Error::Cache("cache directory is not private".into()));
    }
    Ok(())
}

#[cfg(unix)]
fn secure_cache_file(path: &Path) -> Result<()> {
    use std::fs::{OpenOptions, Permissions};
    use std::io::ErrorKind;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let dir = std::fs::symlink_metadata(parent).map_err(map)?;
    if !dir.is_dir() || dir.file_type().is_symlink() || dir.permissions().mode() & 0o777 != 0o700 {
        return Err(Error::Cache(format!(
            "cache directory {} must be private (mode 0700) and not a symlink",
            parent.display()
        )));
    }

    let mut options = OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    match options.open(path) {
        Ok(file) => file
            .set_permissions(Permissions::from_mode(0o600))
            .map_err(map)?,
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            let metadata = std::fs::symlink_metadata(path).map_err(map)?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(Error::Cache("cache database must be a regular file".into()));
            }
            let file = OpenOptions::new().read(true).open(path).map_err(map)?;
            let opened = file.metadata().map_err(map)?;
            if metadata.dev() != opened.dev() || metadata.ino() != opened.ino() {
                return Err(Error::Cache("cache database changed while opening".into()));
            }
            file.set_permissions(Permissions::from_mode(0o600))
                .map_err(map)?;
        }
        Err(error) => return Err(map(error)),
    }
    let metadata = std::fs::symlink_metadata(path).map_err(map)?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o777 != 0o600 {
        return Err(Error::Cache("cache database is not private".into()));
    }
    Ok(())
}

fn participants(m: &MessageMetadata) -> String {
    std::iter::once(m.sender.address.as_str())
        .chain(m.to_list.iter().map(|r| r.address.as_str()))
        .collect::<Vec<_>>()
        .join(" ")
}

fn upsert_metadata(conn: &Connection, m: &MessageMetadata) -> Result<()> {
    let json = serde_json::to_string(m)?;
    conn.execute(
        "INSERT INTO messages(id, time, unread, json) VALUES(?1, ?2, ?3, ?4)
         ON CONFLICT(id) DO UPDATE SET
         time=excluded.time, unread=excluded.unread, json=excluded.json",
        rusqlite::params![m.id, m.time, m.unread, json],
    )
    .map_err(map)?;
    conn.execute("DELETE FROM message_labels WHERE message_id=?1", [&m.id])
        .map_err(map)?;
    for label in &m.label_ids {
        conn.execute(
            "INSERT OR IGNORE INTO message_labels(message_id, label_id, time) VALUES(?1, ?2, ?3)",
            rusqlite::params![m.id, label, m.time],
        )
        .map_err(map)?;
    }
    Ok(())
}

fn upsert_message(conn: &Connection, m: &MessageMetadata) -> Result<()> {
    upsert_metadata(conn, m)?;
    conn.execute(
        "UPDATE msg_fts SET subject=?2, participants=?3 WHERE id=?1",
        rusqlite::params![m.id, m.subject, participants(m)],
    )
    .map_err(map)?;
    Ok(())
}

fn replace_index(conn: &Connection, m: &MessageMetadata, body: Option<&str>) -> Result<()> {
    upsert_metadata(conn, m)?;
    conn.execute("DELETE FROM msg_fts WHERE id=?1", [&m.id])
        .map_err(map)?;
    if let Some(body) = body {
        conn.execute(
            "INSERT INTO msg_fts(id, subject, body, participants) VALUES(?1, ?2, ?3, ?4)",
            rusqlite::params![m.id, m.subject, body, participants(m)],
        )
        .map_err(map)?;
    }
    Ok(())
}

fn delete_message(conn: &Connection, id: &str) -> Result<()> {
    // The messages_delete_fts trigger removes the indexed body in this transaction.
    let deleted = conn
        .execute("DELETE FROM messages WHERE id=?1", [id])
        .map_err(map)?;
    if deleted == 0 {
        // A stray index row can still exist without cached metadata.
        conn.execute("DELETE FROM msg_fts WHERE id=?1", [id])
            .map_err(map)?;
    }
    conn.execute("DELETE FROM message_labels WHERE message_id=?1", [id])
        .map_err(map)?;
    Ok(())
}

fn ensure_label_time_index(conn: &Connection) -> Result<()> {
    // Serialize first-open migration across processes. A failed backfill leaves
    // the old schema and cache contents intact.
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate).map_err(map)?;
    let has_time = {
        let mut columns = tx
            .prepare("PRAGMA table_info(message_labels)")
            .map_err(map)?;
        let names = columns
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(map)?;
        let mut has_time = false;
        for name in names {
            has_time |= name.map_err(map)? == "time";
        }
        has_time
    };
    if !has_time {
        tx.execute_batch(
            "ALTER TABLE message_labels ADD COLUMN time INTEGER;
             UPDATE message_labels SET time=(
                 SELECT time FROM messages WHERE id=message_labels.message_id);",
        )
        .map_err(map)?;
    }
    tx.execute_batch(
        "CREATE TRIGGER IF NOT EXISTS message_labels_insert_time
         AFTER INSERT ON message_labels WHEN NEW.time IS NULL
         BEGIN
             UPDATE message_labels SET time=(
                 SELECT time FROM messages WHERE id=NEW.message_id)
             WHERE message_id=NEW.message_id AND label_id=NEW.label_id;
         END;
         CREATE TRIGGER IF NOT EXISTS messages_update_label_time
         AFTER UPDATE OF time ON messages WHEN OLD.time IS NOT NEW.time
         BEGIN
             UPDATE message_labels SET time=NEW.time WHERE message_id=NEW.id;
         END;
         CREATE INDEX IF NOT EXISTS idx_ml_label_time
         ON message_labels(label_id, time DESC, message_id);",
    )
    .map_err(map)?;
    tx.commit().map_err(map)
}

/// Authenticated account identity, independent of session tokens and addresses.
#[derive(Serialize)]
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

    fn serialized(&self) -> Result<String> {
        Ok(serde_json::to_string(self)?)
    }
}

/// A local metadata cache. Client operations use an account-specific database.
pub struct Cache {
    conn: Connection,
}

impl Cache {
    /// Legacy cache path: `<cache_dir>/ruston-mail/<profile>.db`.
    /// Client operations no longer use this path because its owner is unknown.
    pub fn default_path(profile: &str) -> Result<PathBuf> {
        validate_profile_name(profile)?;
        let dirs = directories::ProjectDirs::from("", "", crate::STORAGE_NAME)
            .ok_or_else(|| Error::Cache("cannot resolve cache dir".into()))?;
        let dir = dirs.cache_dir().to_path_buf();
        #[cfg(unix)]
        secure_cache_dir(&dir)?;
        #[cfg(not(unix))]
        std::fs::create_dir_all(&dir).map_err(map)?;
        Ok(dir.join(format!("{profile}.db")))
    }

    pub(crate) fn default_account_path(profile: &str, identity: &CacheIdentity) -> Result<PathBuf> {
        let legacy_path = Self::default_path(profile)?;
        Self::account_path(legacy_path.parent().unwrap(), profile, identity)
    }

    fn account_path(root: &Path, profile: &str, identity: &CacheIdentity) -> Result<PathBuf> {
        validate_profile_name(profile)?;
        let accounts = root.join("accounts");
        let dir = accounts.join(profile);
        for directory in [&accounts, &dir] {
            #[cfg(unix)]
            secure_cache_dir(directory)?;
            #[cfg(not(unix))]
            std::fs::create_dir_all(directory).map_err(map)?;
        }
        // A fixed-size lowercase name also avoids path separators and collisions
        // caused by case-insensitive filesystems. Verify the full identity in SQLite.
        let digest = Sha256::digest(identity.serialized()?.as_bytes());
        Ok(dir.join(format!("{digest:x}.db")))
    }

    /// Open a database only for its authenticated account. The binding is
    /// immutable: another account, or unbound existing data, is rejected.
    pub(crate) fn open_for_account(path: &Path, identity: &CacheIdentity) -> Result<Cache> {
        let cache = Self::open(path)?;
        let tx =
            Transaction::new_unchecked(&cache.conn, TransactionBehavior::Immediate).map_err(map)?;
        let expected = identity.serialized()?;
        let owner: Option<String> = tx
            .query_row(
                "SELECT value FROM meta WHERE key='account_identity_v1'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(map)?;
        match owner {
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
                    .map_err(map)?;
                if has_data {
                    return Err(Error::Cache(
                        "cache has data without a verified account identity; rebuild it".into(),
                    ));
                }
                tx.execute(
                    "INSERT INTO meta(key, value) VALUES('account_identity_v1', ?1)",
                    [expected],
                )
                .map_err(map)?;
            }
        }
        tx.commit().map_err(map)?;
        Ok(cache)
    }

    /// Open (creating if needed) the cache database at `path`, ensuring its schema.
    /// This low-level API does not verify ownership; Client operations do so
    /// before returning a cache for use.
    /// On Unix, the parent must be a private directory; existing database files
    /// are restricted to mode `0600` before SQLite opens them.
    pub fn open(path: &Path) -> Result<Cache> {
        #[cfg(unix)]
        let sqlite_path = if path == Path::new(":memory:") {
            path.to_path_buf()
        } else {
            secure_cache_file(path)?;
            // NOFOLLOW also rejects symlinks in ancestor paths on macOS.
            std::fs::canonicalize(path).map_err(map)?
        };
        #[cfg(not(unix))]
        let sqlite_path = path.to_path_buf();
        #[cfg(unix)]
        let flags = OpenFlags::default() | OpenFlags::SQLITE_OPEN_NOFOLLOW;
        #[cfg(not(unix))]
        let flags = OpenFlags::default();
        let conn = Connection::open_with_flags(&sqlite_path, flags).map_err(map)?;
        // Another CLI process may hold the write lock during sync or migration.
        conn.busy_timeout(Duration::from_secs(30)).map_err(map)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT);
             CREATE TABLE IF NOT EXISTS messages(id TEXT PRIMARY KEY, time INTEGER, unread INTEGER, json TEXT);
             CREATE TABLE IF NOT EXISTS message_labels(message_id TEXT, label_id TEXT, time INTEGER, PRIMARY KEY(message_id, label_id));
             CREATE INDEX IF NOT EXISTS idx_ml_label ON message_labels(label_id);
             CREATE VIRTUAL TABLE IF NOT EXISTS msg_fts USING fts5(id UNINDEXED, subject, body, participants);
             CREATE TRIGGER IF NOT EXISTS messages_delete_fts AFTER DELETE ON messages
             BEGIN DELETE FROM msg_fts WHERE id=OLD.id; END;",
        )
        .map_err(map)?;
        ensure_label_time_index(&conn)?;
        Ok(Cache { conn })
    }

    /// The stored sync cursor, or `None` if not yet initialized.
    pub fn last_event_id(&self) -> Result<Option<String>> {
        let r = self.conn.query_row(
            "SELECT value FROM meta WHERE key='last_event_id'",
            [],
            |row| row.get::<_, String>(0),
        );
        match r {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(map(e)),
        }
    }

    /// Store the sync cursor.
    pub fn set_last_event_id(&self, id: &str) -> Result<()> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO meta(key, value) VALUES('last_event_id', ?1)",
                [id],
            )
            .map_err(map)?;
        Ok(())
    }

    /// Claim an uninitialized cursor without replacing another sync's cursor.
    pub(crate) fn initialize_cursor(&self, id: &str) -> Result<bool> {
        Ok(self
            .conn
            .execute(
                "INSERT OR IGNORE INTO meta(key, value) VALUES('last_event_id', ?1)",
                [id],
            )
            .map_err(map)?
            == 1)
    }

    fn transaction_at_cursor(&self, expected: Option<&str>) -> Result<Transaction<'_>> {
        // Claim the SQLite write lock before checking; other processes cannot
        // advance the cursor between this check and the commit.
        let tx =
            Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate).map_err(map)?;
        let current: Option<String> = tx
            .query_row(
                "SELECT value FROM meta WHERE key='last_event_id'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(map)?;
        if current.as_deref() != expected {
            return Err(Error::Cache(
                "sync cursor changed during cache download; retry".into(),
            ));
        }
        Ok(tx)
    }

    /// Commit a downloaded page only if sync has not changed since its request.
    pub(crate) fn store_metadata_page(
        &self,
        expected_cursor: Option<&str>,
        messages: &[MessageMetadata],
    ) -> Result<()> {
        let tx = self.transaction_at_cursor(expected_cursor)?;
        for m in messages {
            upsert_message(&tx, m)?;
        }
        tx.commit().map_err(map)
    }

    /// Commit a downloaded body, or invalidate a failed refresh, only while the
    /// cursor captured before listing the page still matches. Both outcomes
    /// update metadata and the body index atomically without changing the cursor.
    pub(crate) fn store_index_result(
        &self,
        expected_cursor: Option<&str>,
        m: &MessageMetadata,
        body: Option<&str>,
    ) -> Result<()> {
        let tx = self.transaction_at_cursor(expected_cursor)?;
        replace_index(&tx, m, body)?;
        tx.commit().map_err(map)
    }

    /// Apply a server batch and its cursor only if the cursor we fetched from
    /// still owns the cache. A stale process leaves both data and cursor intact.
    pub(crate) fn apply_event_batch(
        &self,
        previous_cursor: &str,
        batch: &EventBatch,
    ) -> Result<(usize, usize, usize)> {
        let tx =
            Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate).map_err(map)?;
        let current_cursor: Option<String> = tx
            .query_row(
                "SELECT value FROM meta WHERE key='last_event_id'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(map)?;
        if current_cursor.as_deref() != Some(previous_cursor) {
            return Err(Error::Cache(
                "sync cursor changed during incremental sync; retry".into(),
            ));
        }

        let (mut created, mut updated, mut deleted) = (0, 0, 0);
        for ev in &batch.messages {
            match ev.action {
                action::DELETE => {
                    delete_message(&tx, &ev.id)?;
                    deleted += 1;
                }
                action::CREATE | action::UPDATE => {
                    if ev.action == action::UPDATE {
                        // Events carry metadata, not the decrypted body.
                        tx.execute("DELETE FROM msg_fts WHERE id=?1", [&ev.id])
                            .map_err(map)?;
                    }
                    if let Some(m) = &ev.message {
                        upsert_message(&tx, m)?;
                        if ev.action == action::CREATE {
                            created += 1;
                        } else {
                            updated += 1;
                        }
                    }
                }
                action::UPDATE_FLAGS => {
                    if let Some(m) = &ev.message {
                        upsert_metadata(&tx, m)?;
                        updated += 1;
                    }
                }
                _ => {}
            }
        }
        tx.execute(
            "INSERT OR REPLACE INTO meta(key, value) VALUES('last_event_id', ?1)",
            [&batch.event_id],
        )
        .map_err(map)?;
        tx.commit().map_err(map)?;
        Ok((created, updated, deleted))
    }

    /// Insert or replace cached metadata and labels, keeping searchable headers
    /// current for a body that has already been indexed.
    pub fn upsert_message(&self, m: &MessageMetadata) -> Result<()> {
        let tx = self.conn.unchecked_transaction().map_err(map)?;
        upsert_message(&tx, m)?;
        tx.commit().map_err(map)?;
        Ok(())
    }

    /// Apply a flags/labels-only event without changing indexed headers or body.
    pub fn upsert_message_flags(&self, m: &MessageMetadata) -> Result<()> {
        let tx = self.conn.unchecked_transaction().map_err(map)?;
        upsert_metadata(&tx, m)?;
        tx.commit().map_err(map)?;
        Ok(())
    }

    /// Remove a message, its labels, and its active full-text index entry.
    pub fn delete_message(&self, id: &str) -> Result<()> {
        let tx = self.conn.unchecked_transaction().map_err(map)?;
        delete_message(&tx, id)?;
        tx.commit().map_err(map)?;
        Ok(())
    }

    /// Drop an indexed body when a full update may have changed its content.
    pub fn invalidate_index(&self, id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM msg_fts WHERE id=?1", [id])
            .map_err(map)?;
        Ok(())
    }

    /// Cached messages in a label, newest first.
    pub fn list(
        &self,
        label_id: &str,
        unread_only: bool,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<MessageMetadata>> {
        let sql = format!(
            "SELECT m.json FROM messages m JOIN message_labels l ON m.id=l.message_id \
             WHERE l.label_id=?1 {} ORDER BY l.time DESC LIMIT ?2 OFFSET ?3",
            if unread_only { "AND m.unread=1" } else { "" }
        );
        let mut stmt = self.conn.prepare(&sql).map_err(map)?;
        let rows = stmt
            .query_map(rusqlite::params![label_id, limit, offset], |row| {
                row.get::<_, String>(0)
            })
            .map_err(map)?;
        let mut out = Vec::new();
        for r in rows {
            let json = r.map_err(map)?;
            if let Ok(m) = serde_json::from_str::<MessageMetadata>(&json) {
                out.push(m);
            }
        }
        Ok(out)
    }

    /// (total, unread) cached for a label.
    pub fn count(&self, label_id: &str) -> Result<(i64, i64)> {
        self.conn
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(m.unread),0) FROM messages m \
                 JOIN message_labels l ON m.id=l.message_id WHERE l.label_id=?1",
                [label_id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .map_err(map)
    }

    /// Drop all cached messages and indexed bodies.
    pub fn clear(&self) -> Result<()> {
        let tx = self.conn.unchecked_transaction().map_err(map)?;
        tx.execute_batch("DELETE FROM msg_fts; DELETE FROM message_labels; DELETE FROM messages;")
            .map_err(map)?;
        tx.commit().map_err(map)?;
        Ok(())
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
            .map_err(map)
    }

    /// Stage one API page; a failed page leaves the visible cache untouched.
    pub(crate) fn stage_rebuild_page(&self, messages: &[MessageMetadata]) -> Result<()> {
        let tx = self.conn.unchecked_transaction().map_err(map)?;
        for m in messages {
            let json = serde_json::to_string(m)?;
            tx.execute(
                "INSERT INTO resync_messages(id, time, unread, json) VALUES(?1, ?2, ?3, ?4)
                 ON CONFLICT(id) DO UPDATE SET
                 time=excluded.time, unread=excluded.unread, json=excluded.json",
                rusqlite::params![m.id, m.time, m.unread, json],
            )
            .map_err(map)?;
            tx.execute("DELETE FROM resync_labels WHERE message_id=?1", [&m.id])
                .map_err(map)?;
            for label in &m.label_ids {
                tx.execute(
                    "INSERT OR IGNORE INTO resync_labels(message_id, label_id, time) VALUES(?1, ?2, ?3)",
                    rusqlite::params![m.id, label, m.time],
                )
                .map_err(map)?;
            }
        }
        tx.commit().map_err(map)
    }

    /// Replace metadata, labels, index, and cursor in one transaction.
    pub(crate) fn finish_rebuild(
        &self,
        previous_cursor: &str,
        new_cursor: &str,
        expected_total: u32,
    ) -> Result<usize> {
        let tx =
            Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate).map_err(map)?;
        let current_cursor: Option<String> = tx
            .query_row(
                "SELECT value FROM meta WHERE key='last_event_id'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(map)?;
        if current_cursor.as_deref() != Some(previous_cursor) {
            return Err(Error::Cache(
                "sync cursor changed during resync; retry".into(),
            ));
        }
        let staged: i64 = tx
            .query_row("SELECT COUNT(*) FROM resync_messages", [], |row| row.get(0))
            .map_err(map)?;
        if staged != i64::from(expected_total) {
            return Err(Error::Cache(
                "incomplete mailbox listing during resync; retry".into(),
            ));
        }

        tx.execute_batch(
            "DELETE FROM msg_fts;
             DELETE FROM message_labels;
             DELETE FROM messages;
             INSERT INTO messages(id, time, unread, json)
             SELECT id, time, unread, json FROM resync_messages;
             INSERT INTO message_labels(message_id, label_id, time)
             SELECT message_id, label_id, time FROM resync_labels;",
        )
        .map_err(map)?;
        tx.execute(
            "INSERT OR REPLACE INTO meta(key, value) VALUES('last_event_id', ?1)",
            [new_cursor],
        )
        .map_err(map)?;
        tx.commit().map_err(map)?;
        Ok(expected_total as usize)
    }

    /// Index a message's decrypted body for local full-text search.
    pub fn index_message(&self, m: &MessageMetadata, body: &str) -> Result<()> {
        let tx = self.conn.unchecked_transaction().map_err(map)?;
        replace_index(&tx, m, Some(body))?;
        tx.commit().map_err(map)?;
        Ok(())
    }

    /// Full-text search the local index. Tokens are AND-ed; punctuation-safe.
    pub fn search(&self, query: &str, limit: u32) -> Result<Vec<MessageMetadata>> {
        let expr = query
            .split_whitespace()
            .map(|t| format!("\"{}\"", t.replace('"', "")))
            .collect::<Vec<_>>()
            .join(" ");
        if expr.is_empty() {
            return Ok(Vec::new());
        }
        let mut stmt = self
            .conn
            .prepare(
                "SELECT m.json FROM msg_fts JOIN messages m ON m.id=msg_fts.id \
                 WHERE msg_fts MATCH ?1 ORDER BY rank LIMIT ?2",
            )
            .map_err(map)?;
        let rows = stmt
            .query_map(rusqlite::params![expr, limit], |row| {
                row.get::<_, String>(0)
            })
            .map_err(map)?;
        let mut out = Vec::new();
        for r in rows {
            if let Ok(m) = serde_json::from_str::<MessageMetadata>(&r.map_err(map)?) {
                out.push(m);
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::events::MessageEvent;
    use crate::model::message::Recipient;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_TEMP_DIR: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn cache_path_rejects_profile_with_parent_component() {
        assert!(matches!(
            Cache::default_path("../outside"),
            Err(Error::Session(_))
        ));
    }

    fn meta(id: &str, label: &str, unread: i64, time: i64) -> MessageMetadata {
        MessageMetadata {
            id: id.into(),
            label_ids: vec![label.into()],
            unread,
            time,
            ..Default::default()
        }
    }

    fn event_batch(event_id: &str, messages: Vec<MessageEvent>) -> EventBatch {
        EventBatch {
            event_id: event_id.into(),
            more: false,
            refresh: false,
            messages,
            counts: Vec::new(),
        }
    }

    fn temp_cache_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ruston-cache-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&dir).unwrap();
        #[cfg(unix)]
        secure_cache_dir(&dir).unwrap();
        dir
    }

    #[test]
    fn first_sync_claims_cursor_only_once() {
        let cache = Cache::open(Path::new(":memory:")).unwrap();
        assert!(cache.initialize_cursor("first").unwrap());
        assert!(!cache.initialize_cursor("stale").unwrap());
        assert_eq!(cache.last_event_id().unwrap().as_deref(), Some("first"));
    }

    #[test]
    fn account_switch_does_not_expose_previous_mail() {
        let dir = temp_cache_dir();
        let identity_a = CacheIdentity::new("https://mail.example.test/api", "account-a").unwrap();
        let identity_b = CacheIdentity::new("https://mail.example.test/api", "account-b").unwrap();
        let path_a = Cache::account_path(&dir, "shared-profile", &identity_a).unwrap();
        let path_b = Cache::account_path(&dir, "shared-profile", &identity_b).unwrap();
        let account_a = Cache::open_for_account(&path_a, &identity_a).unwrap();
        account_a
            .index_message(&meta("a", "0", 0, 1), "accountaprivatebody")
            .unwrap();
        account_a.set_last_event_id("account-a-cursor").unwrap();

        let account_b = Cache::open_for_account(&path_b, &identity_b).unwrap();
        assert!(
            account_b
                .search("accountaprivatebody", 10)
                .unwrap()
                .is_empty()
        );
        assert!(account_b.list("0", false, 10, 0).unwrap().is_empty());
        assert_eq!(account_b.count("0").unwrap(), (0, 0));
        assert!(account_b.last_event_id().unwrap().is_none());

        // An old indexing process can finish after B opens the same profile.
        account_a
            .index_message(&meta("late-a", "0", 0, 2), "accountaprivatebody")
            .unwrap();
        account_b
            .index_message(&meta("b", "0", 1, 3), "accountbprivatebody")
            .unwrap();
        account_b.set_last_event_id("account-b-cursor").unwrap();
        let late_batch = event_batch(
            "late-account-a-cursor",
            vec![MessageEvent {
                id: "a".into(),
                action: action::UPDATE_FLAGS,
                message: Some(meta("a", "0", 1, 1)),
            }],
        );
        account_a
            .apply_event_batch("account-a-cursor", &late_batch)
            .unwrap();
        assert_eq!(
            account_b.last_event_id().unwrap().as_deref(),
            Some("account-b-cursor")
        );
        assert!(
            account_b
                .search("accountaprivatebody", 10)
                .unwrap()
                .is_empty()
        );
        assert!(
            account_a
                .search("accountbprivatebody", 10)
                .unwrap()
                .is_empty()
        );
        drop((account_a, account_b));

        // A new session for the same account retains its index and cursor.
        let reopened = Cache::open_for_account(&path_a, &identity_a).unwrap();
        assert_eq!(reopened.search("accountaprivatebody", 10).unwrap().len(), 2);
        assert_eq!(
            reopened.last_event_id().unwrap().as_deref(),
            Some("late-account-a-cursor")
        );
        drop(reopened);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn account_cache_paths_separate_servers_profiles_and_case_sensitive_ids() {
        let dir = temp_cache_dir();
        let identity = CacheIdentity::new("https://mail.example.test/api", "account-a").unwrap();
        let other_server =
            CacheIdentity::new("https://other.example.test/api", "account-a").unwrap();
        let other_case = CacheIdentity::new("https://mail.example.test/api", "Account-a").unwrap();
        let original = Cache::account_path(&dir, "profile", &identity).unwrap();
        for different in [
            Cache::account_path(&dir, "other-profile", &identity).unwrap(),
            Cache::account_path(&dir, "profile", &other_server).unwrap(),
            Cache::account_path(&dir, "profile", &other_case).unwrap(),
        ] {
            assert_ne!(original, different);
        }
        assert_eq!(
            original,
            Cache::account_path(&dir, "profile", &identity).unwrap()
        );

        let unusual =
            CacheIdentity::new("https://mail.example.test/api", "../../outside\\user").unwrap();
        let safe_path = Cache::account_path(&dir, "profile", &unusual).unwrap();
        assert_eq!(safe_path.parent().unwrap(), dir.join("accounts/profile"));
        assert_eq!(safe_path.file_stem().unwrap().len(), 64);
        assert!(Cache::account_path(&dir, "../outside", &identity).is_err());
        assert!(CacheIdentity::new("", "account-a").is_err());
        assert!(CacheIdentity::new("https://mail.example.test/api", " ").is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn account_binding_rejects_another_owner_and_legacy_data() {
        let dir = temp_cache_dir();
        let identity_a = CacheIdentity::new("https://mail.example.test/api", "account-a").unwrap();
        let identity_b = CacheIdentity::new("https://mail.example.test/api", "account-b").unwrap();
        let path = dir.join("bound.db");
        let account_a = Cache::open_for_account(&path, &identity_a).unwrap();
        account_a
            .index_message(&meta("a", "0", 0, 1), "privatebody")
            .unwrap();
        assert!(matches!(
            Cache::open_for_account(&path, &identity_b),
            Err(Error::Cache(_))
        ));
        assert_eq!(account_a.search("privatebody", 10).unwrap().len(), 1);
        account_a.clear().unwrap();
        assert!(Cache::open_for_account(&path, &identity_b).is_err());
        assert!(Cache::open_for_account(&path, &identity_a).is_ok());
        drop(account_a);

        let legacy_path = dir.join("legacy.db");
        let legacy = Cache::open(&legacy_path).unwrap();
        legacy
            .index_message(&meta("unknown", "0", 0, 1), "legacybody")
            .unwrap();
        legacy.set_last_event_id("unknown-cursor").unwrap();
        assert!(matches!(
            Cache::open_for_account(&legacy_path, &identity_b),
            Err(Error::Cache(_))
        ));
        // Refusing an unsafe cache neither claims nor destroys its contents.
        assert_eq!(legacy.search("legacybody", 10).unwrap().len(), 1);
        let owner_count: i64 = legacy
            .conn
            .query_row(
                "SELECT COUNT(*) FROM meta WHERE key='account_identity_v1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(owner_count, 0);
        drop(legacy);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn concurrent_account_binding_has_only_one_owner() {
        let dir = temp_cache_dir();
        let path = dir.join("cache.db");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let workers: Vec<_> = ["account-a", "account-b"]
            .into_iter()
            .map(|id| {
                let path = path.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let identity = CacheIdentity::new("https://mail.example.test/api", id).unwrap();
                    barrier.wait();
                    Cache::open_for_account(&path, &identity).is_ok()
                })
            })
            .collect();
        let successes = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .filter(|success| *success)
            .count();
        assert_eq!(successes, 1);
        let cache = Cache::open(&path).unwrap();
        assert_eq!(
            cache
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM meta WHERE key='account_identity_v1'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
        drop(cache);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn downloaded_body_cannot_restore_a_deleted_message() {
        let dir = temp_cache_dir();
        let path = dir.join("cache.db");
        let indexer = Cache::open(&path).unwrap();
        let syncer = Cache::open(&path).unwrap();
        indexer.set_last_event_id("before-download").unwrap();
        let old = meta("m1", "0", 0, 1);
        indexer.index_message(&old, "privatebody").unwrap();
        let cursor = indexer.last_event_id().unwrap();

        // The indexer has a response in flight when another connection deletes it.
        syncer
            .apply_event_batch(
                "before-download",
                &event_batch(
                    "after-delete",
                    vec![MessageEvent {
                        id: old.id.clone(),
                        action: action::DELETE,
                        message: None,
                    }],
                ),
            )
            .unwrap();
        assert!(
            indexer
                .store_index_result(cursor.as_deref(), &old, Some("privatebody"))
                .is_err()
        );

        assert_eq!(
            indexer.last_event_id().unwrap().as_deref(),
            Some("after-delete")
        );
        assert!(indexer.list("0", false, 10, 0).unwrap().is_empty());
        assert!(indexer.search("privatebody", 10).unwrap().is_empty());
        drop(indexer);
        drop(syncer);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cursor_initialization_rejects_downloads_started_without_a_cursor() {
        let cache = Cache::open(Path::new(":memory:")).unwrap();
        let old_cursor = cache.last_event_id().unwrap();
        assert!(cache.initialize_cursor("initialized").unwrap());
        let message = meta("m1", "0", 0, 1);

        assert!(
            cache
                .store_metadata_page(old_cursor.as_deref(), std::slice::from_ref(&message))
                .is_err()
        );
        for body in [None, Some("privatebody")] {
            assert!(
                cache
                    .store_index_result(old_cursor.as_deref(), &message, body)
                    .is_err()
            );
        }
        assert_eq!(cache.count("0").unwrap(), (0, 0));
        assert!(cache.search("privatebody", 10).unwrap().is_empty());
        assert_eq!(
            cache.last_event_id().unwrap().as_deref(),
            Some("initialized")
        );
    }

    #[test]
    fn failed_download_commits_roll_back_metadata_labels_and_bodies() {
        let cache = Cache::open(Path::new(":memory:")).unwrap();
        cache.set_last_event_id("old").unwrap();
        let original = meta("keep", "0", 0, 1);
        cache.index_message(&original, "privatebody").unwrap();
        cache
            .conn
            .execute_batch(
                "CREATE TRIGGER reject_bad BEFORE INSERT ON messages
             WHEN NEW.id='bad' BEGIN SELECT RAISE(ABORT, 'bad'); END;",
            )
            .unwrap();
        let updated = meta("keep", "5", 1, 2);
        assert!(
            cache
                .store_metadata_page(Some("old"), &[updated.clone(), meta("bad", "0", 0, 3)])
                .is_err()
        );
        assert_eq!(cache.list("0", false, 10, 0).unwrap()[0].time, 1);
        assert!(cache.list("5", false, 10, 0).unwrap().is_empty());

        cache
            .conn
            .execute_batch(
                "CREATE TRIGGER reject_changed_label BEFORE INSERT ON message_labels
             WHEN NEW.label_id='5' BEGIN SELECT RAISE(ABORT, 'bad label'); END;",
            )
            .unwrap();
        for body in [None, Some("downloadedbody")] {
            assert!(
                cache
                    .store_index_result(Some("old"), &updated, body)
                    .is_err()
            );
            assert_eq!(cache.search("privatebody", 10).unwrap()[0].time, 1);
            assert_eq!(cache.list("0", false, 10, 0).unwrap()[0].unread, 0);
            assert!(cache.list("5", false, 10, 0).unwrap().is_empty());
            assert!(cache.search("downloadedbody", 10).unwrap().is_empty());
        }
        assert_eq!(cache.last_event_id().unwrap().as_deref(), Some("old"));
    }

    #[test]
    fn stale_process_cannot_replace_a_newer_batch() {
        let dir = temp_cache_dir();
        let path = dir.join("cache.db");
        let fast = Cache::open(&path).unwrap();
        let slow = Cache::open(&path).unwrap();
        fast.set_last_event_id("old").unwrap();
        fast.index_message(&meta("m1", "0", 0, 1), "privatebody")
            .unwrap();

        let newer = event_batch(
            "new",
            vec![MessageEvent {
                id: "m1".into(),
                action: action::UPDATE_FLAGS,
                message: Some(meta("m1", "0", 1, 2)),
            }],
        );
        assert_eq!(fast.apply_event_batch("old", &newer).unwrap(), (0, 1, 0));
        let stale = event_batch(
            "older",
            vec![MessageEvent {
                id: "m1".into(),
                action: action::DELETE,
                message: None,
            }],
        );
        assert!(slow.apply_event_batch("old", &stale).is_err());
        assert_eq!(slow.last_event_id().unwrap().as_deref(), Some("new"));
        assert_eq!(slow.list("0", false, 10, 0).unwrap()[0].unread, 1);
        assert_eq!(slow.search("privatebody", 10).unwrap().len(), 1);

        let updated = event_batch(
            "next",
            vec![MessageEvent {
                id: "m1".into(),
                action: action::UPDATE,
                message: Some(meta("m1", "0", 1, 3)),
            }],
        );
        assert_eq!(slow.apply_event_batch("new", &updated).unwrap(), (0, 1, 0));
        assert!(slow.search("privatebody", 10).unwrap().is_empty());

        drop(slow);
        drop(fast);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn failed_batch_rolls_back_prior_changes_and_cursor() {
        let cache = Cache::open(Path::new(":memory:")).unwrap();
        cache.set_last_event_id("old").unwrap();
        cache
            .index_message(&meta("keep", "0", 0, 1), "privatebody")
            .unwrap();
        cache
            .conn
            .execute_batch(
                "CREATE TRIGGER reject_bad BEFORE INSERT ON messages
                 WHEN NEW.id='bad' BEGIN SELECT RAISE(ABORT, 'bad'); END;",
            )
            .unwrap();
        let batch = event_batch(
            "new",
            vec![
                MessageEvent {
                    id: "keep".into(),
                    action: action::DELETE,
                    message: None,
                },
                MessageEvent {
                    id: "bad".into(),
                    action: action::CREATE,
                    message: Some(meta("bad", "0", 0, 2)),
                },
            ],
        );
        assert!(cache.apply_event_batch("old", &batch).is_err());
        assert_eq!(cache.last_event_id().unwrap().as_deref(), Some("old"));
        assert_eq!(cache.list("0", false, 10, 0).unwrap()[0].id, "keep");
        assert_eq!(cache.search("privatebody", 10).unwrap().len(), 1);
    }

    #[test]
    fn upsert_list_delete_roundtrip() {
        let c = Cache::open(Path::new(":memory:")).unwrap();

        assert_eq!(c.last_event_id().unwrap(), None);
        c.set_last_event_id("evt-1").unwrap();
        assert_eq!(c.last_event_id().unwrap().as_deref(), Some("evt-1"));

        c.upsert_message(&meta("a", "0", 1, 100)).unwrap();
        c.upsert_message(&meta("b", "0", 0, 200)).unwrap();
        let inbox = c.list("0", false, 10, 0).unwrap();
        assert_eq!(inbox.len(), 2);
        assert_eq!(inbox[0].id, "b"); // newest first
        assert_eq!(c.list("0", true, 10, 0).unwrap().len(), 1); // only unread
        assert_eq!(c.count("0").unwrap(), (2, 1));

        c.delete_message("a").unwrap();
        assert_eq!(c.list("0", false, 10, 0).unwrap().len(), 1);
        c.clear().unwrap();
        assert_eq!(c.list("0", false, 10, 0).unwrap().len(), 0);
    }

    #[test]
    fn existing_cache_backfills_label_times_and_uses_ordered_index() {
        let dir = temp_cache_dir();
        let path = dir.join("cache.db");
        let legacy = Connection::open(&path).unwrap();
        legacy
            .execute_batch(
                "CREATE TABLE messages(id TEXT PRIMARY KEY, time INTEGER, unread INTEGER, json TEXT);
                 CREATE TABLE message_labels(message_id TEXT, label_id TEXT, PRIMARY KEY(message_id, label_id));",
            )
            .unwrap();
        for m in [meta("older", "0", 0, 10), meta("newer", "0", 0, 20)] {
            legacy
                .execute(
                    "INSERT INTO messages(id, time, unread, json) VALUES(?1, ?2, ?3, ?4)",
                    rusqlite::params![m.id, m.time, m.unread, serde_json::to_string(&m).unwrap()],
                )
                .unwrap();
            legacy
                .execute(
                    "INSERT INTO message_labels(message_id, label_id) VALUES(?1, '0')",
                    [&m.id],
                )
                .unwrap();
        }
        drop(legacy);

        let cache = Cache::open(&path).unwrap();
        let ids: Vec<_> = cache
            .list("0", false, 10, 0)
            .unwrap()
            .into_iter()
            .map(|m| m.id)
            .collect();
        assert_eq!(ids, ["newer", "older"]);
        let times: Vec<i64> = cache
            .conn
            .prepare("SELECT time FROM message_labels WHERE label_id='0' ORDER BY time DESC")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|row| row.unwrap())
            .collect();
        assert_eq!(times, [20, 10]);

        let plan: Vec<String> = cache
            .conn
            .prepare(
                "EXPLAIN QUERY PLAN SELECT m.json FROM messages m
                 JOIN message_labels l ON m.id=l.message_id
                 WHERE l.label_id=?1 ORDER BY l.time DESC LIMIT ?2 OFFSET ?3",
            )
            .unwrap()
            .query_map(rusqlite::params!["0", 10, 0], |row| row.get(3))
            .unwrap()
            .map(|row| row.unwrap())
            .collect();
        assert!(plan.iter().any(|step| step.contains("idx_ml_label_time")));
        assert!(!plan.iter().any(|step| step.contains("TEMP B-TREE")));

        let old_reader_ids: Vec<String> = cache
            .conn
            .prepare(
                "SELECT m.id FROM messages m JOIN message_labels l ON m.id=l.message_id
                 WHERE l.label_id='0' ORDER BY m.time DESC",
            )
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|row| row.unwrap())
            .collect();
        assert_eq!(old_reader_ids, ["newer", "older"]);
        drop(cache);
        let reopened = Cache::open(&path).unwrap();
        assert_eq!(reopened.list("0", false, 1, 0).unwrap()[0].id, "newer");
        drop(reopened);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn failed_label_time_backfill_keeps_legacy_schema_and_rows() {
        let dir = temp_cache_dir();
        let path = dir.join("cache.db");
        let legacy = Connection::open(&path).unwrap();
        legacy
            .execute_batch(
                "CREATE TABLE messages(id TEXT PRIMARY KEY, time INTEGER, unread INTEGER, json TEXT);
                 CREATE TABLE message_labels(message_id TEXT, label_id TEXT, PRIMARY KEY(message_id, label_id));
                 INSERT INTO messages VALUES('keep', 10, 0, '{}');
                 INSERT INTO message_labels VALUES('keep', '0');
                 CREATE TRIGGER reject_backfill BEFORE UPDATE ON message_labels
                 BEGIN SELECT RAISE(ABORT, 'reject backfill'); END;",
            )
            .unwrap();
        drop(legacy);

        assert!(Cache::open(&path).is_err());
        let legacy = Connection::open(&path).unwrap();
        let columns: Vec<String> = legacy
            .prepare("PRAGMA table_info(message_labels)")
            .unwrap()
            .query_map([], |row| row.get(1))
            .unwrap()
            .map(|row| row.unwrap())
            .collect();
        assert!(!columns.contains(&"time".to_string()));
        let count: i64 = legacy
            .query_row("SELECT COUNT(*) FROM message_labels", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);

        drop(legacy);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn label_times_follow_updates_rebuilds_and_legacy_writes() {
        let cache = Cache::open(Path::new(":memory:")).unwrap();
        cache.upsert_message(&meta("a", "0", 0, 10)).unwrap();
        cache.upsert_message(&meta("b", "0", 0, 20)).unwrap();
        cache.upsert_message_flags(&meta("a", "0", 1, 30)).unwrap();
        assert_eq!(cache.list("0", false, 1, 0).unwrap()[0].id, "a");

        // An older process does not know the new column; the trigger fills it.
        let legacy = meta("legacy", "0", 0, 40);
        cache
            .conn
            .execute(
                "INSERT INTO messages(id, time, unread, json) VALUES(?1, ?2, ?3, ?4)",
                rusqlite::params![
                    legacy.id,
                    legacy.time,
                    legacy.unread,
                    serde_json::to_string(&legacy).unwrap()
                ],
            )
            .unwrap();
        cache
            .conn
            .execute(
                "INSERT INTO message_labels(message_id, label_id) VALUES('legacy', '0')",
                [],
            )
            .unwrap();
        assert_eq!(cache.list("0", false, 1, 0).unwrap()[0].id, "legacy");
        cache
            .conn
            .execute("UPDATE messages SET time=5 WHERE id='legacy'", [])
            .unwrap();
        let time: i64 = cache
            .conn
            .query_row(
                "SELECT time FROM message_labels WHERE message_id='legacy'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(time, 5);

        cache.set_last_event_id("old").unwrap();
        cache.begin_rebuild().unwrap();
        cache
            .stage_rebuild_page(&[meta("rebuilt", "0", 0, 50)])
            .unwrap();
        cache.finish_rebuild("old", "new", 1).unwrap();
        assert_eq!(cache.list("0", false, 10, 0).unwrap()[0].id, "rebuilt");
        let time: i64 = cache
            .conn
            .query_row(
                "SELECT time FROM message_labels WHERE message_id='rebuilt'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(time, 50);
    }

    #[test]
    fn incomplete_rebuild_cannot_replace_cache_or_cursor() {
        let c = Cache::open(Path::new(":memory:")).unwrap();
        c.set_last_event_id("old").unwrap();
        c.index_message(&meta("keep", "0", 0, 1), "privatebody")
            .unwrap();
        c.begin_rebuild().unwrap();
        c.stage_rebuild_page(&[meta("new", "0", 0, 2)]).unwrap();

        assert!(c.finish_rebuild("old", "fresh", 2).is_err());
        assert_eq!(c.last_event_id().unwrap().as_deref(), Some("old"));
        assert_eq!(c.list("0", false, 10, 0).unwrap()[0].id, "keep");
        assert_eq!(c.search("privatebody", 10).unwrap()[0].id, "keep");
    }

    #[test]
    fn fts_index_and_search() {
        let c = Cache::open(Path::new(":memory:")).unwrap();
        c.index_message(&meta("m1", "5", 0, 10), "hello from ruston-cli rust client")
            .unwrap();
        c.index_message(&meta("m2", "5", 0, 20), "completely unrelated content here")
            .unwrap();
        let hits = c.search("ruston-cli", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "m1");
        assert!(c.search("nonexistentword", 10).unwrap().is_empty());
        // multi-token AND
        assert_eq!(c.search("hello rust", 10).unwrap().len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn disk_cache_restricts_new_and_existing_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = temp_cache_dir();
        let new_cache_dir = dir.join("new-cache");
        secure_cache_dir(&new_cache_dir).unwrap();
        assert_eq!(
            std::fs::metadata(&new_cache_dir)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        let path = dir.join("cache.db");

        let cache = Cache::open(&path).unwrap();
        cache
            .index_message(&meta("m1", "0", 0, 1), "privatebody")
            .unwrap();
        cache
            .conn
            .execute_batch("BEGIN IMMEDIATE; INSERT INTO meta VALUES('test', 'secret')")
            .unwrap();
        let journal = path.with_extension("db-journal");
        assert_eq!(
            std::fs::metadata(&journal).unwrap().permissions().mode() & 0o777,
            0o600
        );
        cache.conn.execute_batch("ROLLBACK").unwrap();
        drop(cache);
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let cache = Cache::open(&path).unwrap();
        assert_eq!(cache.search("privatebody", 1).unwrap().len(), 1);
        drop(cache);
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(Cache::open(&path).is_err());
        secure_cache_dir(&dir).unwrap();
        assert_eq!(
            std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        Cache::open(&path).unwrap();

        let link = dir.join("linked.db");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(Cache::open(&link).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn deleting_a_message_removes_its_decrypted_fts_entry() {
        let c = Cache::open(Path::new(":memory:")).unwrap();
        c.index_message(&meta("m1", "0", 0, 10), "privatebody")
            .unwrap();
        c.delete_message("m1").unwrap();

        let remaining: i64 = c
            .conn
            .query_row("SELECT COUNT(*) FROM msg_fts WHERE id='m1'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(remaining, 0);
        assert!(c.search("privatebody", 10).unwrap().is_empty());

        c.conn
            .execute(
                "INSERT INTO msg_fts(id, subject, body, participants)
                 VALUES('orphan', '', 'orphanbody', '')",
                [],
            )
            .unwrap();
        c.delete_message("orphan").unwrap();
        let orphan_rows: i64 = c
            .conn
            .query_row(
                "SELECT COUNT(*) FROM msg_fts WHERE id='orphan'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(orphan_rows, 0);
    }

    #[test]
    fn metadata_upsert_refreshes_searchable_headers_without_losing_body() {
        let c = Cache::open(Path::new(":memory:")).unwrap();
        c.conn
            .execute_batch("PRAGMA recursive_triggers=ON")
            .unwrap();
        let mut original = meta("m1", "0", 0, 10);
        original.subject = "oldsubject".into();
        original.sender = Recipient::new("oldsender@example.test");
        original.to_list = vec![Recipient::new("oldrecipient@example.test")];
        c.index_message(&original, "samebody").unwrap();

        let mut updated = original;
        updated.subject = "newsubject".into();
        updated.sender = Recipient::new("newsender@example.test");
        updated.to_list = vec![Recipient::new("newrecipient@example.test")];
        c.upsert_message(&updated).unwrap();

        for old in ["oldsubject", "oldsender", "oldrecipient"] {
            assert!(c.search(old, 10).unwrap().is_empty());
        }
        for current in ["newsubject", "newsender", "newrecipient", "samebody"] {
            assert_eq!(c.search(current, 10).unwrap()[0].id, "m1");
        }
    }

    #[test]
    fn invalidated_body_stays_out_of_search_until_reindexed() {
        let c = Cache::open(Path::new(":memory:")).unwrap();
        let m = meta("m1", "0", 0, 10);
        c.index_message(&m, "oldbody").unwrap();
        c.invalidate_index(&m.id).unwrap();
        assert!(c.search("oldbody", 10).unwrap().is_empty());

        c.index_message(&m, "newbody").unwrap();
        assert!(c.search("oldbody", 10).unwrap().is_empty());
        assert_eq!(c.search("newbody", 10).unwrap()[0].id, "m1");
    }

    #[test]
    fn deleting_metadata_directly_removes_the_indexed_body() {
        let c = Cache::open(Path::new(":memory:")).unwrap();
        c.index_message(&meta("m1", "0", 0, 10), "privatebody")
            .unwrap();
        c.conn
            .execute("DELETE FROM messages WHERE id='m1'", [])
            .unwrap();
        let remaining: i64 = c
            .conn
            .query_row("SELECT COUNT(*) FROM msg_fts WHERE id='m1'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(remaining, 0);
    }
}
