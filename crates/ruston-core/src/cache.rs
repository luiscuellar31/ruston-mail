//! Local SQLite cache of message metadata + the sync cursor.

use crate::error::{Error, Result};
use crate::model::message::MessageMetadata;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior};
use std::path::{Path, PathBuf};

const FTS_ORPHAN_REPAIR_KEY: &str = "fts_orphans_repaired_v1";

fn map<E: std::fmt::Display>(e: E) -> Error {
    Error::Cache(e.to_string())
}

fn participants(m: &MessageMetadata) -> String {
    std::iter::once(m.sender.address.as_str())
        .chain(m.to_list.iter().map(|r| r.address.as_str()))
        .collect::<Vec<_>>()
        .join(" ")
}

fn orphan_repair_done(conn: &Connection) -> Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM meta WHERE key=?1)",
        [FTS_ORPHAN_REPAIR_KEY],
        |row| row.get::<_, i64>(0),
    )
    .map(|found| found != 0)
    .map_err(map)
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

/// A per-profile metadata cache.
pub struct Cache {
    conn: Connection,
}

impl Cache {
    /// Default cache path: `<cache_dir>/protonmail-cli/<profile>.db`.
    pub fn default_path(profile: &str) -> Result<PathBuf> {
        let dirs = directories::ProjectDirs::from("me", "Proton", "protonmail-cli")
            .ok_or_else(|| Error::Cache("cannot resolve cache dir".into()))?;
        let dir = dirs.cache_dir().to_path_buf();
        std::fs::create_dir_all(&dir).map_err(map)?;
        let p = if profile.is_empty() {
            "default"
        } else {
            profile
        };
        Ok(dir.join(format!("{p}.db")))
    }

    /// Open (creating if needed) the cache database at `path`, ensuring its schema.
    pub fn open(path: &Path) -> Result<Cache> {
        let conn = Connection::open(path).map_err(map)?;
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
        // Older caches may already contain indexed bodies for deleted messages.
        // Repair once; the trigger also protects deletes made by older clients.
        if !orphan_repair_done(&conn)? {
            let tx =
                Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).map_err(map)?;
            if !orphan_repair_done(&tx)? {
                tx.execute(
                    "DELETE FROM msg_fts WHERE NOT EXISTS
                     (SELECT 1 FROM messages WHERE messages.id=msg_fts.id)",
                    [],
                )
                .map_err(map)?;
                tx.execute(
                    "INSERT INTO meta(key, value) VALUES(?1, 'done')",
                    [FTS_ORPHAN_REPAIR_KEY],
                )
                .map_err(map)?;
            }
            tx.commit().map_err(map)?;
        }
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

    /// Insert or replace cached metadata and labels, keeping searchable headers
    /// current for a body that has already been indexed.
    pub fn upsert_message(&self, m: &MessageMetadata) -> Result<()> {
        let tx = self.conn.unchecked_transaction().map_err(map)?;
        upsert_metadata(&tx, m)?;
        tx.execute(
            "UPDATE msg_fts SET subject=?2, participants=?3 WHERE id=?1",
            rusqlite::params![m.id, m.subject, participants(m)],
        )
        .map_err(map)?;
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
        // The messages_delete_fts trigger removes the indexed body in this transaction.
        let deleted = tx
            .execute("DELETE FROM messages WHERE id=?1", [id])
            .map_err(map)?;
        if deleted == 0 {
            // A stray index row can still exist without cached metadata.
            tx.execute("DELETE FROM msg_fts WHERE id=?1", [id])
                .map_err(map)?;
        }
        tx.execute("DELETE FROM message_labels WHERE message_id=?1", [id])
            .map_err(map)?;
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
    use crate::model::message::Recipient;

    fn meta(id: &str, label: &str, unread: i64, time: i64) -> MessageMetadata {
        MessageMetadata {
            id: id.into(),
            label_ids: vec![label.into()],
            unread,
            time,
            ..Default::default()
        }
    }

    #[test]
    fn upsert_list_delete_roundtrip() {
        let tmp = std::env::temp_dir().join(format!("ptest-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&tmp);
        let c = Cache::open(&tmp).unwrap();

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
        let _ = std::fs::remove_file(&tmp);
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
        let tmp = std::env::temp_dir().join(format!("pfts-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&tmp);
        let c = Cache::open(&tmp).unwrap();
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
        let _ = std::fs::remove_file(&tmp);
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
    fn opening_an_old_cache_repairs_orphans_and_keeps_valid_index_entries() {
        let path =
            std::env::temp_dir().join(format!("ruston-fts-repair-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let c = Cache::open(&path).unwrap();
            c.index_message(&meta("orphan", "0", 0, 10), "oldprivatebody")
                .unwrap();
            c.index_message(&meta("keep", "0", 0, 20), "currentbody")
                .unwrap();
            c.conn
                .execute_batch(
                    "DROP TRIGGER messages_delete_fts;
                     DELETE FROM messages WHERE id='orphan';
                     DELETE FROM message_labels WHERE message_id='orphan';",
                )
                .unwrap();
            c.conn
                .execute("DELETE FROM meta WHERE key=?1", [FTS_ORPHAN_REPAIR_KEY])
                .unwrap();
        }

        let c = Cache::open(&path).unwrap();
        let orphan_rows: i64 = c
            .conn
            .query_row(
                "SELECT COUNT(*) FROM msg_fts WHERE id='orphan'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(orphan_rows, 0);
        assert_eq!(c.search("currentbody", 10).unwrap()[0].id, "keep");

        // A client that still deletes metadata directly gets the same cleanup.
        c.conn
            .execute("DELETE FROM messages WHERE id='keep'", [])
            .unwrap();
        let remaining: i64 = c
            .conn
            .query_row("SELECT COUNT(*) FROM msg_fts WHERE id='keep'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(remaining, 0);
        drop(c);
        std::fs::remove_file(path).unwrap();
    }
}
