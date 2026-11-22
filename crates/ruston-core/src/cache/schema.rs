//! The cache's tables and indexes: the version this build writes, what an
//! older cache needs to reach it, and the migration that brings it there.

use super::cache_error;
use crate::error::{Error, Result};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior};

pub(super) fn ensure_label_time_index(conn: &Connection) -> Result<()> {
    // Serialize first-open migration across processes. A failed backfill leaves
    // the old schema and cache contents intact.
    let tx =
        Transaction::new_unchecked(conn, TransactionBehavior::Immediate).map_err(cache_error)?;
    let has_time = {
        let mut columns = tx
            .prepare("PRAGMA table_info(message_labels)")
            .map_err(cache_error)?;
        let names = columns
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(cache_error)?;
        let mut has_time = false;
        for name in names {
            has_time |= name.map_err(cache_error)? == "time";
        }
        has_time
    };
    if !has_time {
        tx.execute_batch(
            "ALTER TABLE message_labels ADD COLUMN time INTEGER;
             UPDATE message_labels SET time=(
                 SELECT time FROM messages WHERE id=message_labels.message_id);",
        )
        .map_err(cache_error)?;
    }
    // The time index starts with label_id, so it serves every lookup the old
    // label-only index did, which only cost writes and secure deletes.
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
         ON message_labels(label_id, time DESC, message_id);
         DROP INDEX IF EXISTS idx_ml_label;",
    )
    .map_err(cache_error)?;
    tx.commit().map_err(cache_error)
}

/// The full-text table's columns: the message ID, left out of the index, and
/// the text searched.
pub(super) const FTS_COLUMNS: &str = "id UNINDEXED, subject, body, participants";

/// The schema this version writes, kept in SQLite's `user_version`; a cache
/// from before it was kept reads 0. Opening an older cache brings it up to
/// date, and one a newer version wrote is refused: this version cannot know
/// what that one expects of it.
pub(super) const SCHEMA_VERSION: i64 = 1;

/// Whether opening has to migrate the cache: it is older than
/// [`SCHEMA_VERSION`], or an older version put back the label-only index the
/// migration drops. Only reads, so it never waits for a writer.
pub(super) fn needs_migration(conn: &Connection) -> Result<bool> {
    let version = schema_version(conn)?;
    if version < SCHEMA_VERSION {
        return Ok(true);
    }
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='index' AND name='idx_ml_label')",
        [],
        |row| row.get(0),
    )
    .map_err(cache_error)
}

/// Whether opening to write has to migrate the cache: [`needs_migration`]
/// says so, or its index rows cannot yet say which bodies were indexed as
/// text, which only writing bodies needs, so a cache opened to read stays
/// searchable without it.
pub(super) fn needs_migration_to_write(conn: &Connection) -> Result<bool> {
    Ok(needs_migration(conn)? || !has_as_text_column(conn)?)
}

/// Whether the index rows say which bodies were indexed as text.
pub(super) fn has_as_text_column(conn: &Connection) -> Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('msg_fts_rows') WHERE name='as_text')",
        [],
        |row| row.get(0),
    )
    .map_err(cache_error)
}

/// Marks which indexed bodies hold text alone, as this version indexes an
/// HTML body. Those indexed before held the HTML itself, with every address
/// its links led to, and an older version still writes rows without the mark;
/// both read 0, so `index` reads those bodies again, and until then they stay
/// searchable. The schema version stays, so older versions keep opening it.
pub(super) fn ensure_as_text_column(conn: &Connection) -> Result<()> {
    let tx =
        Transaction::new_unchecked(conn, TransactionBehavior::Immediate).map_err(cache_error)?;
    if !has_as_text_column(&tx)? {
        tx.execute_batch("ALTER TABLE msg_fts_rows ADD COLUMN as_text INTEGER NOT NULL DEFAULT 0")
            .map_err(cache_error)?;
    }
    tx.commit().map_err(cache_error)
}

/// The cache's schema version, refusing one newer than this version's.
pub(super) fn schema_version(conn: &Connection) -> Result<i64> {
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(cache_error)?;
    if version > SCHEMA_VERSION {
        return Err(Error::Cache(format!(
            "cache was written by a newer version of Ruston Mail (schema {version}); update Ruston Mail to use it"
        )));
    }
    Ok(version)
}

/// Brings the cache to [`SCHEMA_VERSION`]. Each step looks at what it finds
/// and holds the write lock only while it works, so processes opening the
/// cache at once take turns, and a step that fails leaves what it found.
pub(super) fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT);
         CREATE TABLE IF NOT EXISTS messages(id TEXT PRIMARY KEY, time INTEGER, unread INTEGER, json TEXT);
         CREATE TABLE IF NOT EXISTS message_labels(message_id TEXT, label_id TEXT, time INTEGER, PRIMARY KEY(message_id, label_id));
         CREATE VIRTUAL TABLE IF NOT EXISTS msg_fts USING fts5({FTS_COLUMNS});",
    ))
    .map_err(cache_error)?;
    ensure_label_time_index(conn)?;
    ensure_fts_rowid_index(conn)?;
    ensure_as_text_column(conn)?;
    ensure_fts_secure_delete(conn)?;
    // A newer version may have migrated it further meanwhile.
    let tx =
        Transaction::new_unchecked(conn, TransactionBehavior::Immediate).map_err(cache_error)?;
    schema_version(&tx)?;
    tx.execute_batch(&format!("PRAGMA user_version={SCHEMA_VERSION}"))
        .map_err(cache_error)?;
    tx.commit().map_err(cache_error)
}

/// FTS5 keeps a deleted body's words in its index until segments merge;
/// secure-delete removes them with the row. FTS5 keeps the setting, so its
/// settings table is only read here, to write the setting once.
pub(super) fn ensure_fts_secure_delete(conn: &Connection) -> Result<()> {
    let enabled: Option<i64> = conn
        .query_row(
            "SELECT v FROM msg_fts_index_config WHERE k='secure-delete'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(cache_error)?;
    if enabled != Some(1) {
        conn.execute(
            "INSERT INTO msg_fts_index(msg_fts_index, rank) VALUES('secure-delete', 1)",
            [],
        )
        .map_err(cache_error)?;
    }
    Ok(())
}

pub(super) fn ensure_fts_rowid_index(conn: &Connection) -> Result<()> {
    let tx =
        Transaction::new_unchecked(conn, TransactionBehavior::Immediate).map_err(cache_error)?;
    let kind: String = tx
        .query_row(
            "SELECT type FROM sqlite_schema WHERE name='msg_fts'",
            [],
            |row| row.get(0),
        )
        .map_err(cache_error)?;
    if kind == "view" {
        return tx.commit().map_err(cache_error);
    }

    // Preserve existing FTS data and rowids. FTS5 does not allow table triggers,
    // so a compatibility view keeps the ID/rowid map in sync for every writer,
    // including a process using the previous schema. Do not modify shadow tables.
    tx.execute_batch(
        "DROP TRIGGER IF EXISTS messages_delete_fts;
         ALTER TABLE msg_fts RENAME TO msg_fts_index;
         CREATE TABLE msg_fts_rows(fts_rowid INTEGER PRIMARY KEY, message_id TEXT NOT NULL);
         CREATE INDEX idx_fts_message_id ON msg_fts_rows(message_id);
         INSERT INTO msg_fts_rows(fts_rowid, message_id) SELECT rowid, id FROM msg_fts_index;
         CREATE VIEW msg_fts AS
         SELECT rowid, id, subject, body, participants, rank, msg_fts_index AS msg_fts
         FROM msg_fts_index;
         CREATE TRIGGER msg_fts_insert INSTEAD OF INSERT ON msg_fts
         WHEN NEW.msg_fts IS NULL
         BEGIN
             INSERT INTO msg_fts_index(rowid, id, subject, body, participants)
             VALUES(NEW.rowid, NEW.id, NEW.subject, NEW.body, NEW.participants);
             INSERT INTO msg_fts_rows(fts_rowid, message_id) VALUES(last_insert_rowid(), NEW.id);
         END;
         CREATE TRIGGER msg_fts_command INSTEAD OF INSERT ON msg_fts
         WHEN NEW.msg_fts IS NOT NULL
         BEGIN
             INSERT INTO msg_fts_index(msg_fts_index, rank) VALUES(NEW.msg_fts, NEW.rank);
         END;
         CREATE TRIGGER msg_fts_update INSTEAD OF UPDATE ON msg_fts
         BEGIN
             UPDATE msg_fts_index
             SET rowid=NEW.rowid, id=NEW.id, subject=NEW.subject,
                 body=NEW.body, participants=NEW.participants
             WHERE rowid=OLD.rowid;
             UPDATE msg_fts_rows SET fts_rowid=NEW.rowid, message_id=NEW.id
             WHERE fts_rowid=OLD.rowid;
         END;
         CREATE TRIGGER msg_fts_delete INSTEAD OF DELETE ON msg_fts
         BEGIN
             DELETE FROM msg_fts_index WHERE rowid=OLD.rowid;
             DELETE FROM msg_fts_rows WHERE fts_rowid=OLD.rowid;
         END;
         CREATE TRIGGER messages_delete_fts AFTER DELETE ON messages
         BEGIN
             DELETE FROM msg_fts WHERE rowid IN (
                 SELECT fts_rowid FROM msg_fts_rows WHERE message_id=OLD.id);
         END;",
    )
    .map_err(cache_error)?;
    tx.commit().map_err(cache_error)
}
