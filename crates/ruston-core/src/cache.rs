//! Local SQLite cache of message metadata, indexed bodies, and the sync cursor.

use crate::api::events::{EventBatch, action};
use crate::error::{Error, Result};
use crate::model::message::MessageMetadata;
use crate::session::validate_profile_name;
use rusqlite::{Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior};
use std::path::{Path, PathBuf};

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
            "INSERT OR IGNORE INTO message_labels(message_id, label_id) VALUES(?1, ?2)",
            rusqlite::params![m.id, label],
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

/// A per-profile metadata cache.
pub struct Cache {
    conn: Connection,
}

impl Cache {
    /// Default cache path: `<cache_dir>/ruston-mail/<profile>.db`.
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

    /// Open (creating if needed) the cache database at `path`, ensuring its schema.
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
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT);
             CREATE TABLE IF NOT EXISTS messages(id TEXT PRIMARY KEY, time INTEGER, unread INTEGER, json TEXT);
             CREATE TABLE IF NOT EXISTS message_labels(message_id TEXT, label_id TEXT, PRIMARY KEY(message_id, label_id));
             CREATE INDEX IF NOT EXISTS idx_ml_label ON message_labels(label_id);
             CREATE VIRTUAL TABLE IF NOT EXISTS msg_fts USING fts5(id UNINDEXED, subject, body, participants);
             CREATE TRIGGER IF NOT EXISTS messages_delete_fts AFTER DELETE ON messages
             BEGIN DELETE FROM msg_fts WHERE id=OLD.id; END;",
        )
        .map_err(map)?;
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
             WHERE l.label_id=?1 {} ORDER BY m.time DESC LIMIT ?2 OFFSET ?3",
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
                     message_id TEXT, label_id TEXT, PRIMARY KEY(message_id, label_id));
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
                    "INSERT OR IGNORE INTO resync_labels(message_id, label_id) VALUES(?1, ?2)",
                    rusqlite::params![m.id, label],
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
             INSERT INTO message_labels(message_id, label_id)
             SELECT message_id, label_id FROM resync_labels;",
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
        upsert_metadata(&tx, m)?;
        tx.execute("DELETE FROM msg_fts WHERE id=?1", [&m.id])
            .map_err(map)?;
        tx.execute(
            "INSERT INTO msg_fts(id, subject, body, participants) VALUES(?1, ?2, ?3, ?4)",
            rusqlite::params![m.id, m.subject, body, participants(m)],
        )
        .map_err(map)?;
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
            "ruston-cache-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
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
