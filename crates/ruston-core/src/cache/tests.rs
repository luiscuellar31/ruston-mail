use super::*;
use crate::api::events::MessageEvent;
use crate::model::message::Recipient;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TEMP_DIR: AtomicUsize = AtomicUsize::new(0);

#[test]
fn the_cache_identity_format_never_changes() {
    let identity = CacheIdentity::new("https://mail.proton.me/api", "user-1").unwrap();
    assert_eq!(
        identity.serialized().unwrap(),
        r#"{"base_url":"https://mail.proton.me/api","user_id":"user-1"}"#
    );

    let root = temp_cache_dir();
    let path = Cache::account_path(&root, "ruston", &identity).unwrap();
    assert_eq!(
        path.strip_prefix(&root).unwrap(),
        Path::new("accounts/ruston")
            .join("2919895bff4c1e5d64a2863dde2837a36bb1d2bcf370d93e16e7f79fd6c08f37.db")
    );
    std::fs::remove_dir_all(root).unwrap();
}

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
    let builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    let mut builder = builder;
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
fn deleted_and_reindexed_bodies_leave_no_plaintext_in_the_file() {
    let dir = temp_cache_dir();
    let path = dir.join("cache.db");
    let cache = Cache::open(&path).unwrap();
    // Enough mail that the removed bodies share pages with kept ones.
    for index in 0..50 {
        cache
            .index_message(
                &meta(&format!("kept-{index}"), "0", 0, index),
                &"filler body ".repeat(20),
            )
            .unwrap();
    }
    cache
        .index_message(&meta("deleted", "0", 0, 100), "zebracanary deleted body")
        .unwrap();
    cache
        .index_message(&meta("edited", "0", 0, 101), "quokkacanary first draft")
        .unwrap();
    cache.delete_message("deleted").unwrap();
    cache
        .index_message(&meta("edited", "0", 0, 101), "replacement")
        .unwrap();
    drop(cache);

    let file = std::fs::read(&path).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    for word in ["zebracanary", "quokkacanary"] {
        assert!(
            !file
                .windows(word.len())
                .any(|window| window == word.as_bytes()),
            "{word} is still in the cache file"
        );
    }
}

#[test]
fn reopening_keeps_search_secure_delete_without_rewriting_it() {
    let dir = temp_cache_dir();
    let path = dir.join("cache.db");
    drop(Cache::open(&path).unwrap());
    let cache = Cache::open(&path).unwrap();
    let enabled: i64 = cache
        .conn
        .query_row(
            "SELECT v FROM msg_fts_index_config WHERE k='secure-delete'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(enabled, 1);
    assert_eq!(cache.conn.total_changes(), 0, "reopening wrote nothing");
    drop(cache);
    std::fs::remove_dir_all(&dir).unwrap();
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
    let other_server = CacheIdentity::new("https://other.example.test/api", "account-a").unwrap();
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
    // A flag change leaves the indexed body current.
    assert_eq!(
        fast.apply_event_batch("old", &newer).unwrap(),
        BatchCounts {
            updated: 1,
            flags_updated: 1,
            ..BatchCounts::default()
        }
    );
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
    assert_eq!(
        slow.apply_event_batch("new", &updated).unwrap(),
        BatchCounts {
            updated: 1,
            ..BatchCounts::default()
        }
    );
    assert!(slow.search("privatebody", 10).unwrap().is_empty());

    drop(slow);
    drop(fast);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn events_change_only_what_their_action_and_message_allow() {
    let cache = Cache::open(Path::new(":memory:")).unwrap();
    cache.set_last_event_id("old").unwrap();
    cache
        .index_message(&meta("indexed", "0", 0, 1), "privatebody")
        .unwrap();
    cache.upsert_message(&meta("flagged", "0", 0, 2)).unwrap();
    let event = |id: &str, code, message| MessageEvent {
        id: id.into(),
        action: code,
        message,
    };
    let batch = event_batch(
        "new",
        vec![
            event("created", action::CREATE, Some(meta("created", "0", 1, 3))),
            event("missing", action::CREATE, None),
            // An update drops the indexed body even without its message.
            event("indexed", action::UPDATE, None),
            event("flagged", action::UPDATE_FLAGS, None),
            event("unknown", 9, Some(meta("unknown", "0", 1, 4))),
        ],
    );

    assert_eq!(
        cache.apply_event_batch("old", &batch).unwrap(),
        BatchCounts {
            created: 1,
            ..BatchCounts::default()
        }
    );
    let listed: Vec<(String, i64)> = cache
        .list("0", false, 10, 0)
        .unwrap()
        .into_iter()
        .map(|m| (m.id, m.unread))
        .collect();
    assert_eq!(
        listed,
        [
            ("created".to_owned(), 1),
            ("flagged".to_owned(), 0),
            ("indexed".to_owned(), 0)
        ]
    );
    assert!(cache.search("privatebody", 10).unwrap().is_empty());
    // The cursor moves past every event, the unknown one included.
    assert_eq!(cache.last_event_id().unwrap().as_deref(), Some("new"));
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

    for unread_only in [false, true] {
        let plan: Vec<String> = cache
            .conn
            .prepare(&format!("EXPLAIN QUERY PLAN {}", list_sql(unread_only)))
            .unwrap()
            .query_map(rusqlite::params!["0", 10, 0], |row| row.get(3))
            .unwrap()
            .map(|row| row.unwrap())
            .collect();
        assert!(plan.iter().any(|step| step.contains("idx_ml_label_time")));
        assert!(!plan.iter().any(|step| step.contains("TEMP B-TREE")));
    }

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
fn labels_are_indexed_once_in_time_order() {
    let indexes = |cache: &Cache| -> Vec<String> {
        cache
            .conn
            .prepare(
                "SELECT name FROM sqlite_master WHERE type='index'
                 AND tbl_name='message_labels' AND sql IS NOT NULL ORDER BY name",
            )
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|row| row.unwrap())
            .collect()
    };
    let dir = temp_cache_dir();
    let path = dir.join("cache.db");

    let cache = Cache::open(&path).unwrap();
    assert_eq!(indexes(&cache), ["idx_ml_label_time"]);
    // Earlier versions kept a label-only index beside it.
    cache
        .conn
        .execute_batch("CREATE INDEX idx_ml_label ON message_labels(label_id)")
        .unwrap();
    drop(cache);

    let reopened = Cache::open(&path).unwrap();
    assert_eq!(indexes(&reopened), ["idx_ml_label_time"]);
    let plan: Vec<String> = reopened
        .conn
        .prepare(&format!("EXPLAIN QUERY PLAN {COUNT_SQL}"))
        .unwrap()
        .query_map(["0"], |row| row.get(3))
        .unwrap()
        .map(|row| row.unwrap())
        .collect();
    assert!(
        plan.iter().any(|step| step.contains("idx_ml_label_time")),
        "{plan:?}"
    );
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

/// A cache on disk with `count` indexed bodies of `words` more words
/// each, its cursor at "old" and a rebuild of the same messages staged.
fn staged_resync(path: &Path, count: i64, words: i64) -> Cache {
    let cache = Cache::open(path).unwrap();
    cache.set_last_event_id("old").unwrap();
    let messages: Vec<_> = (0..count)
        .map(|index| meta(&format!("m{index}"), "0", 0, index))
        .collect();
    for (index, message) in (0..).zip(&messages) {
        let filler: Vec<_> = (0..words)
            .map(|word| format!("w{}", (index * 31 + word * 7) % 5000))
            .collect();
        let body = format!("ibexcanary body {} {}", message.id, filler.join(" "));
        cache.index_message(message, &body).unwrap();
    }
    cache.begin_rebuild().unwrap();
    cache.stage_rebuild_page(&messages).unwrap();
    cache
}

#[test]
fn a_resync_leaves_no_indexed_body_in_the_file_and_search_working() {
    let dir = temp_cache_dir();
    let path = dir.join("cache.db");
    let cache = staged_resync(&path, 50, 0);

    assert_eq!(cache.finish_rebuild("old", "fresh", 50).unwrap(), 50);
    assert!(cache.search("ibexcanary", 10).unwrap().is_empty());
    assert_eq!(cache.list("0", false, 100, 0).unwrap().len(), 50);
    // Bodies indexed again are found, and stay wiped once deleted.
    cache
        .index_message(&meta("m7", "0", 0, 7), "newbody")
        .unwrap();
    assert_eq!(cache.search("newbody", 10).unwrap()[0].id, "m7");
    assert_fts_rowid_map_matches(&cache.conn);
    let secure_delete: i64 = cache
        .conn
        .query_row(
            "SELECT v FROM msg_fts_index_config WHERE k='secure-delete'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(secure_delete, 1);
    drop(cache);

    let file = std::fs::read(&path).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(
        !file
            .windows("ibexcanary".len())
            .any(|window| window == b"ibexcanary"),
        "a body indexed before the resync is still in the cache file"
    );
}

#[test]
#[ignore = "manual benchmark of a resync over a full index"]
fn measure_resync_index_reset() {
    for count in [1000, 2000, 4000] {
        let dir = temp_cache_dir();
        // About the words of a short email each.
        let cache = staged_resync(&dir.join("cache.db"), count, 300);
        let start = std::time::Instant::now();
        cache.finish_rebuild("old", "fresh", count as u32).unwrap();
        eprintln!(
            "Cache::finish_rebuild: {count} indexed messages in {:?}",
            start.elapsed()
        );
        drop(cache);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[test]
fn unchanged_headers_leave_the_indexed_body_alone() {
    let cache = Cache::open(Path::new(":memory:")).unwrap();
    let mut message = meta("m1", "0", 1, 10);
    message.subject = "Invoice".into();
    cache.index_message(&message, "privatebody").unwrap();
    // Every rewrite of a full-text row also updates its row map.
    cache
        .conn
        .execute_batch(
            "CREATE TEMP TABLE rewrites(message_id TEXT);
             CREATE TEMP TRIGGER count_rewrites AFTER UPDATE ON msg_fts_rows
             BEGIN INSERT INTO rewrites VALUES(NEW.message_id); END;",
        )
        .unwrap();
    let rewrites = || -> i64 {
        cache
            .conn
            .query_row("SELECT count(*) FROM rewrites", [], |row| row.get(0))
            .unwrap()
    };

    // Marking it read changes no header, so the body is not rewritten.
    message.unread = 0;
    cache.upsert_message(&message).unwrap();
    assert_eq!(rewrites(), 0);

    // A new subject still reaches the index, with the body kept.
    message.subject = "Receipt".into();
    cache.upsert_message(&message).unwrap();
    assert_eq!(rewrites(), 1);
    assert_eq!(cache.search("receipt", 10).unwrap()[0].id, "m1");
    assert_eq!(cache.search("privatebody", 10).unwrap()[0].id, "m1");
    assert!(cache.search("invoice", 10).unwrap().is_empty());
}

#[test]
fn fts_id_maintenance_does_not_scan_unrelated_rows() {
    use rusqlite::StatementStatus;

    let mut measurements = Vec::new();
    for count in [128, 2048] {
        let cache = Cache::open(Path::new(":memory:")).unwrap();
        let tx = cache.conn.unchecked_transaction().unwrap();
        for index in 0..count {
            tx.execute(
                "INSERT INTO msg_fts(id, subject, body, participants) VALUES(?1, '', 'body', '')",
                [format!("m{index}")],
            )
            .unwrap();
        }
        tx.commit().unwrap();
        let mut delete = cache.conn.prepare(DELETE_INDEX_SQL).unwrap();
        delete.execute(["missing"]).unwrap();
        let mut update = cache.conn.prepare(UPDATE_INDEX_SQL).unwrap();
        update
            .execute(rusqlite::params!["missing", "subject", "sender"])
            .unwrap();
        measurements.push((
            delete.get_status(StatementStatus::VmStep),
            update.get_status(StatementStatus::VmStep),
        ));
        for sql in [DELETE_INDEX_SQL, UPDATE_INDEX_SQL] {
            let mut explain = cache
                .conn
                .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                .unwrap();
            let parameters = explain.parameter_count();
            let plan: Vec<String> = explain
                .query_map(
                    rusqlite::params_from_iter((0..parameters).map(|_| "missing")),
                    |row| row.get(3),
                )
                .unwrap()
                .map(|row| row.unwrap())
                .collect();
            assert!(
                plan.iter()
                    .any(|step| step.contains("USING COVERING INDEX idx_fts_message_id")),
                "{plan:?}"
            );
            assert!(
                plan.iter()
                    .any(|step| step.contains("msg_fts_index VIRTUAL TABLE INDEX")
                        && step.ends_with('=')),
                "{plan:?}"
            );
        }
    }
    eprintln!("FTS maintenance VM steps (128 / 2048 rows): {measurements:?}");
    assert!(measurements[1].0 <= measurements[0].0 * 2);
    assert!(measurements[1].1 <= measurements[0].1 * 2);
}

fn create_legacy_fts_cache(path: &Path) -> Connection {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT);
         INSERT INTO meta VALUES('last_event_id', 'old-cursor');
         INSERT INTO meta VALUES('account_identity_v1', 'original-account-binding');
         CREATE TABLE messages(id TEXT PRIMARY KEY, time INTEGER, unread INTEGER, json TEXT);
         CREATE TABLE message_labels(message_id TEXT, label_id TEXT, time INTEGER,
             PRIMARY KEY(message_id, label_id));
         CREATE VIRTUAL TABLE msg_fts USING fts5(id UNINDEXED, subject, body, participants);
         CREATE TRIGGER messages_delete_fts AFTER DELETE ON messages
         BEGIN DELETE FROM msg_fts WHERE id=OLD.id; END;
         INSERT INTO msg_fts(rowid,id,subject,body,participants) VALUES
             (7,'m1','oldsubject','privatebody','sender'),
             (11,'m1','oldsubject','duplicatebody','sender'),
             (17,'orphan','','orphanbody',''),
             (21,'m2','','otherbody','');",
    )
    .unwrap();
    upsert_metadata(&conn, &meta("m1", "0", 1, 42)).unwrap();
    upsert_metadata(&conn, &meta("m2", "5", 0, 43)).unwrap();
    conn
}

fn assert_fts_rowid_map_matches(conn: &Connection) {
    let mismatches: i64 = conn.query_row(
        "SELECT
         (SELECT COUNT(*) FROM (
             SELECT rowid,id FROM msg_fts_index EXCEPT SELECT fts_rowid,message_id FROM msg_fts_rows)) +
         (SELECT COUNT(*) FROM (
             SELECT fts_rowid,message_id FROM msg_fts_rows EXCEPT SELECT rowid,id FROM msg_fts_index))",
        [], |row| row.get(0),
    ).unwrap();
    assert_eq!(mismatches, 0);
    conn.execute("INSERT INTO msg_fts(msg_fts) VALUES('integrity-check')", [])
        .unwrap();
}

#[test]
fn fts_migration_preserves_rows_search_cursor_and_binding_on_reopen() {
    let dir = temp_cache_dir();
    let path = dir.join("cache.db");
    let legacy = create_legacy_fts_cache(&path);
    let before: Vec<(i64, String, String)> = legacy
        .prepare("SELECT rowid,id,body FROM msg_fts ORDER BY rowid")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .map(|row| row.unwrap())
        .collect();
    let cache = Cache::open(&path).unwrap();
    let after: Vec<(i64, String, String)> = cache
        .conn
        .prepare("SELECT rowid,id,body FROM msg_fts ORDER BY rowid")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .map(|row| row.unwrap())
        .collect();
    assert_eq!(before, after);
    assert_eq!(cache.search("privatebody", 10).unwrap()[0].id, "m1");
    assert_eq!(cache.list("0", true, 10, 0).unwrap()[0].time, 42);
    assert_eq!(
        cache.last_event_id().unwrap().as_deref(),
        Some("old-cursor")
    );
    let binding: String = cache
        .conn
        .query_row(
            "SELECT value FROM meta WHERE key='account_identity_v1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(binding, "original-account-binding");
    assert_fts_rowid_map_matches(&cache.conn);
    drop(cache);
    let cache = Cache::open(&path).unwrap();
    assert_eq!(cache.search("duplicatebody", 10).unwrap()[0].id, "m1");
    assert_fts_rowid_map_matches(&cache.conn);
    drop(cache);
    drop(legacy);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn failed_fts_migration_rolls_back_and_can_be_retried() {
    let dir = temp_cache_dir();
    let path = dir.join("cache.db");
    let legacy = create_legacy_fts_cache(&path);
    // Force failure after the rename and trigger removal inside migration.
    legacy
        .execute_batch("CREATE VIEW msg_fts_rows AS SELECT 1")
        .unwrap();
    assert!(Cache::open(&path).is_err());
    let kind: String = legacy
        .query_row(
            "SELECT type FROM sqlite_schema WHERE name='msg_fts'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(kind, "table");
    let new_objects: i64 = legacy.query_row("SELECT COUNT(*) FROM sqlite_schema WHERE name IN ('msg_fts_index','idx_fts_message_id','msg_fts_insert')", [], |row| row.get(0)).unwrap();
    assert_eq!(new_objects, 0);
    let old_trigger: String = legacy
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE name='messages_delete_fts'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(old_trigger.contains("WHERE id=OLD.id"));
    assert_eq!(
        legacy
            .query_row("SELECT body FROM msg_fts WHERE rowid=7", [], |row| row
                .get::<_, String>(
                0
            ))
            .unwrap(),
        "privatebody"
    );
    assert_eq!(
        legacy
            .query_row(
                "SELECT value FROM meta WHERE key='last_event_id'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "old-cursor"
    );
    legacy.execute_batch("DROP VIEW msg_fts_rows").unwrap();
    let cache = Cache::open(&path).unwrap();
    assert_eq!(cache.search("privatebody", 10).unwrap()[0].id, "m1");
    assert_fts_rowid_map_matches(&cache.conn);
    drop(cache);
    drop(legacy);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn legacy_writes_and_new_writes_share_the_rowid_map() {
    let dir = temp_cache_dir();
    let path = dir.join("cache.db");
    // This connection predates migration and keeps using the original SQL.
    let legacy = create_legacy_fts_cache(&path);
    let cache = Cache::open(&path).unwrap();
    legacy
        .execute(
            "UPDATE msg_fts SET subject='legacysubject' WHERE id='m1'",
            [],
        )
        .unwrap();
    assert_eq!(cache.search("legacysubject", 10).unwrap().len(), 2);
    cache.invalidate_index("m1").unwrap();
    assert!(cache.search("privatebody", 10).unwrap().is_empty());
    assert!(cache.search("duplicatebody", 10).unwrap().is_empty());
    legacy
        .execute(
            "INSERT INTO msg_fts(id,subject,body,participants) VALUES('m1','','legacybody','')",
            [],
        )
        .unwrap();
    assert_fts_rowid_map_matches(&cache.conn);
    assert_eq!(cache.search("legacybody", 10).unwrap()[0].id, "m1");
    cache
        .index_message(&meta("m1", "0", 1, 42), "newbody")
        .unwrap();
    let old_reader: String = legacy
        .query_row(
            "SELECT id FROM msg_fts WHERE msg_fts MATCH 'newbody' ORDER BY rank LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(old_reader, "m1");
    assert_fts_rowid_map_matches(&legacy);
    legacy
        .execute("DELETE FROM messages WHERE id='m1'", [])
        .unwrap();
    assert!(cache.search("newbody", 10).unwrap().is_empty());
    cache.delete_message("orphan").unwrap();
    assert_fts_rowid_map_matches(&cache.conn);
    legacy.execute("DELETE FROM msg_fts", []).unwrap();
    assert_fts_rowid_map_matches(&cache.conn);
    cache
        .index_message(&meta("m2", "5", 0, 43), "reindexedbody")
        .unwrap();
    cache.clear().unwrap();
    assert_eq!(
        cache
            .conn
            .query_row("SELECT COUNT(*) FROM msg_fts_rows", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_fts_rowid_map_matches(&cache.conn);
    drop(cache);
    drop(legacy);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn failed_rowid_map_write_restores_metadata_body_and_mapping() {
    let cache = Cache::open(Path::new(":memory:")).unwrap();
    cache.set_last_event_id("old").unwrap();
    let original = meta("m1", "0", 0, 42);
    cache.index_message(&original, "privatebody").unwrap();
    cache
        .conn
        .execute_batch(
            "CREATE TRIGGER reject_map_insert BEFORE INSERT ON msg_fts_rows
         BEGIN SELECT RAISE(ABORT, 'reject rowid map'); END;",
        )
        .unwrap();
    // The new FTS row is inserted before the trigger tries to insert its map.
    // Both that row and the prior deletion must roll back with the metadata.
    assert!(
        cache
            .store_index_result(Some("old"), &meta("m1", "5", 1, 43), Some("newbody"))
            .is_err()
    );
    assert_eq!(cache.search("privatebody", 10).unwrap()[0].time, 42);
    assert!(cache.search("newbody", 10).unwrap().is_empty());
    assert_eq!(cache.count("0").unwrap(), (1, 0));
    assert_eq!(cache.count("5").unwrap(), (0, 0));
    assert_eq!(cache.last_event_id().unwrap().as_deref(), Some("old"));
    assert_fts_rowid_map_matches(&cache.conn);
}

#[test]
fn concurrent_fts_migrations_and_writers_preserve_the_map() {
    let dir = temp_cache_dir();
    let path = dir.join("cache.db");
    let legacy = create_legacy_fts_cache(&path);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let workers: Vec<_> = (0..2)
        .map(|index| {
            let path = path.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                let cache = Cache::open(&path).unwrap();
                cache
                    .index_message(&meta(&format!("new{index}"), "0", 0, index), "newbody")
                    .unwrap();
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }
    let cache = Cache::open(&path).unwrap();
    assert_eq!(cache.search("newbody", 10).unwrap().len(), 2);
    assert_eq!(cache.search("privatebody", 10).unwrap()[0].id, "m1");
    assert_fts_rowid_map_matches(&cache.conn);
    drop(cache);
    drop(legacy);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
#[ignore = "manual scaling benchmark with fictional in-memory mail"]
fn measure_fts_index_scaling() {
    for count in [1000, 2000, 4000] {
        let cache = Cache::open(Path::new(":memory:")).unwrap();
        let start = std::time::Instant::now();
        for index in 0..count {
            cache
                .index_message(
                    &meta(&format!("m{index}"), "0", 0, index),
                    "fictional message body",
                )
                .unwrap();
        }
        eprintln!(
            "Cache::index_message: {count} messages in {:?}",
            start.elapsed()
        );
        assert_eq!(
            cache.search("fictional", count as u32).unwrap().len(),
            count as usize
        );
    }
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
