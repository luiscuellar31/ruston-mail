//! Incremental sync (event stream) + local-cache reads.

use super::{Client, read::FullMessage};
use crate::api;
use crate::api::messages::ListQuery;
use crate::cache::Cache;
use crate::error::{Error, Result};
use crate::model::enums::resolve_folder;
use crate::model::message::MessageMetadata;
use crate::transport::Doer;
use std::borrow::Borrow;
use std::future::Future;

const RESYNC_PAGE_SIZE: u32 = 100;

/// Outcome of a sync run.
#[derive(Debug, Clone)]
pub struct SyncReport {
    /// Number of messages created.
    pub created: usize,
    /// Number of messages updated.
    pub updated: usize,
    /// Number of messages deleted.
    pub deleted: usize,
    /// The event cursor after applying this sync.
    pub event_id: String,
    /// First sync — the cursor was initialized (no deltas applied).
    pub initialized: bool,
    /// Number of messages rebuilt after a server-requested full resync.
    pub rebuilt: Option<usize>,
}

async fn rebuild_cache<D: Doer>(
    http: &D,
    cache: &Cache,
    previous_cursor: &str,
    page_size: u32,
) -> Result<(String, usize)> {
    let cursor = api::events::get_latest_event_id(http).await?;
    cache.begin_rebuild()?;
    let mut expected_total = None;
    let mut page = 0;
    loop {
        let query = ListQuery {
            page: Some(page),
            page_size: Some(page_size),
            ..Default::default()
        };
        let (total, messages) = api::messages::list_messages(http, &query).await?;
        if expected_total.is_some_and(|expected| expected != total) {
            return Err(Error::Cache("mailbox changed during resync; retry".into()));
        }
        expected_total = Some(total);
        if messages.is_empty() && (u64::from(page) * u64::from(page_size)) < u64::from(total) {
            return Err(Error::Cache(
                "incomplete mailbox listing during resync; retry".into(),
            ));
        }
        cache.stage_rebuild_page(&messages)?;
        if (u64::from(page) + 1) * u64::from(page_size) >= u64::from(total) {
            break;
        }
        page += 1;
    }
    let rebuilt = cache.finish_rebuild(previous_cursor, &cursor, expected_total.unwrap_or(0))?;
    Ok((cursor, rebuilt))
}

async fn sync_cache<D: Doer>(http: &D, cache: &Cache) -> Result<SyncReport> {
    let mut id = match cache.last_event_id()? {
        Some(i) => i,
        None => {
            // Bootstrap: record the current cursor; deltas flow from here.
            let latest = api::events::get_latest_event_id(http).await?;
            if cache.initialize_cursor(&latest)? {
                tracing::info!(target: "ruston_core::sync", event_id = %latest, "sync initialized cursor");
                return Ok(SyncReport {
                    created: 0,
                    updated: 0,
                    deleted: 0,
                    event_id: latest,
                    initialized: true,
                    rebuilt: None,
                });
            }
            cache.last_event_id()?.ok_or_else(|| {
                Error::Cache("sync cursor disappeared during initialization; retry".into())
            })?
        }
    };

    let (mut created, mut updated, mut deleted) = (0, 0, 0);
    let mut rebuilt = None;
    loop {
        let batch = api::events::get_events(http, &id).await?;
        if batch.refresh {
            if rebuilt.is_some() {
                return Err(Error::Cache(
                    "server requested another resync; retry".into(),
                ));
            }
            let (cursor, count) = rebuild_cache(http, cache, &id, RESYNC_PAGE_SIZE).await?;
            tracing::warn!(target: "ruston_core::sync", rebuilt = count, "server requested full resync — cache rebuilt");
            id = cursor;
            rebuilt = Some(count);
            created = 0;
            updated = 0;
            deleted = 0;
            continue;
        }
        let (batch_created, batch_updated, batch_deleted) = cache.apply_event_batch(&id, &batch)?;
        created += batch_created;
        updated += batch_updated;
        deleted += batch_deleted;
        id = batch.event_id;
        if !batch.more {
            break;
        }
    }
    tracing::info!(target: "ruston_core::sync", created, updated, deleted, event_id = %id, "sync complete");
    Ok(SyncReport {
        created,
        updated,
        deleted,
        event_id: id,
        initialized: false,
        rebuilt,
    })
}

// Client passes an owned Cache to keep its future Send: SQLite connections are
// not Sync. Tests can borrow a cache to apply events while requests are paused.
async fn backfill_cache<D: Doer>(
    http: &D,
    cache: impl Borrow<Cache>,
    folder: &str,
    max_pages: u32,
    page_size: u32,
) -> Result<usize> {
    let label = resolve_folder(folder);
    let mut n = 0usize;
    for page in 0..max_pages {
        let q = ListQuery {
            label_id: Some(label.clone()),
            page: Some(page),
            page_size: Some(page_size),
            ..Default::default()
        };
        let cursor = cache.borrow().last_event_id()?;
        let (_total, msgs) = api::messages::list_messages(http, &q).await?;
        if msgs.is_empty() {
            break;
        }
        cache
            .borrow()
            .store_metadata_page(cursor.as_deref(), &msgs)?;
        n += msgs.len();
        if (msgs.len() as u32) < page_size {
            break;
        }
    }
    tracing::info!(target: "ruston_core::sync", folder, cached = n, "cache_folder backfill");
    Ok(n)
}

async fn index_cache<D, F, Fut>(
    http: &D,
    cache: impl Borrow<Cache>,
    folder: &str,
    max_pages: u32,
    page_size: u32,
    mut read_message: F,
) -> Result<usize>
where
    D: Doer,
    F: FnMut(String) -> Fut,
    Fut: Future<Output = Result<FullMessage>>,
{
    let label = resolve_folder(folder);
    let mut n = 0usize;
    for page in 0..max_pages {
        let q = ListQuery {
            label_id: Some(label.clone()),
            page: Some(page),
            page_size: Some(page_size),
            ..Default::default()
        };
        // Keep the page cursor for every body, including failed reads.
        let cursor = cache.borrow().last_event_id()?;
        let (_total, msgs) = api::messages::list_messages(http, &q).await?;
        if msgs.is_empty() {
            break;
        }
        for meta in &msgs {
            match read_message(meta.id.clone()).await {
                Ok(full) => {
                    cache.borrow().store_index_result(
                        cursor.as_deref(),
                        &full.meta,
                        Some(&full.body),
                    )?;
                    n += 1;
                }
                Err(_) => {
                    // A failed refresh must not keep an older body searchable.
                    cache
                        .borrow()
                        .store_index_result(cursor.as_deref(), meta, None)?;
                }
            }
        }
        if (msgs.len() as u32) < page_size {
            break;
        }
    }
    tracing::info!(target: "ruston_core::sync", folder, indexed = n, "local search index built");
    Ok(n)
}

impl Client {
    pub(crate) fn open_cache(&self) -> Result<Cache> {
        let path = Cache::default_account_path(&self.profile, &self.cache_identity)?;
        Cache::open_for_account(&path, &self.cache_identity)
    }

    /// Apply incremental events from the cached cursor into the local cache.
    pub async fn sync(&self) -> Result<SyncReport> {
        let cache = self.open_cache()?;
        sync_cache(self.http(), &cache).await
    }

    /// Backfill a folder into the cache by paging the API (bounded by `max_pages`).
    /// Returns a retry error if sync changes the cursor before a page is saved;
    /// previously committed pages remain cached.
    pub async fn cache_folder(
        &self,
        folder: &str,
        max_pages: u32,
        page_size: u32,
    ) -> Result<usize> {
        let cache = self.open_cache()?;
        backfill_cache(self.http(), cache, folder, max_pages, page_size).await
    }

    /// Read messages for a folder from the local cache (offline).
    pub fn cached_messages(
        &self,
        folder: &str,
        unread: bool,
        limit: u32,
    ) -> Result<Vec<MessageMetadata>> {
        self.open_cache()?
            .list(&resolve_folder(folder), unread, limit, 0)
    }

    /// Cached (total, unread) counts for a folder.
    pub fn cached_count(&self, folder: &str) -> Result<(i64, i64)> {
        self.open_cache()?.count(&resolve_folder(folder))
    }

    /// Build the local full-text search index: page a folder, decrypt each
    /// message body, and index it (bounded by `max_pages`).
    /// Returns a retry error if sync changes the page's cursor before a result
    /// is saved; previously committed messages remain indexed.
    pub async fn index_folder(
        &self,
        folder: &str,
        max_pages: u32,
        page_size: u32,
    ) -> Result<usize> {
        let cache = self.open_cache()?;
        index_cache(
            self.http(),
            cache,
            folder,
            max_pages,
            page_size,
            |id| async move { self.read_message(&id).await },
        )
        .await
    }

    /// Full-text search the local index (decrypted bodies; private + offline).
    pub fn search_local(&self, query: &str, limit: u32) -> Result<Vec<MessageMetadata>> {
        self.open_cache()?.search(query, limit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::events::{EventBatch, MessageEvent, action};
    use crate::crypto::Verdict;
    use crate::transport::{HttpClient, Request, Response};
    use serde::de::DeserializeOwned;
    use std::path::Path;
    use tokio::sync::Semaphore;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn message(id: &str) -> serde_json::Value {
        serde_json::json!({
            "ID": id,
            "LabelIDs": ["0"],
            "Sender": {"Address": "sender@example.test"}
        })
    }

    fn cached_message(id: &str) -> MessageMetadata {
        MessageMetadata {
            id: id.into(),
            label_ids: vec!["0".into()],
            ..Default::default()
        }
    }

    struct ListingDoer {
        pages: Vec<Vec<MessageMetadata>>,
        gate: Semaphore,
    }

    #[async_trait::async_trait]
    impl Doer for ListingDoer {
        async fn do_raw(&self, _req: Request) -> Result<Response> {
            unreachable!("listing uses decode")
        }

        async fn decode<T: DeserializeOwned>(&self, req: Request) -> Result<T> {
            assert_eq!(req.path, "/mail/v4/messages");
            let page: usize = req
                .query
                .iter()
                .find(|(key, _)| key == "Page")
                .unwrap()
                .1
                .parse()
                .unwrap();
            let permit = self.gate.acquire().await.unwrap();
            permit.forget();
            Ok(serde_json::from_value(serde_json::json!({
                "Total": self.pages.iter().map(Vec::len).sum::<usize>(),
                "Messages": self.pages.get(page).cloned().unwrap_or_default(),
            }))?)
        }
    }

    fn full_message(meta: MessageMetadata) -> FullMessage {
        FullMessage {
            meta,
            body: "downloadedbody".into(),
            mime_type: "text/plain".into(),
            verdict: Verdict::Unsigned,
            attachments: Vec::new(),
        }
    }

    #[test]
    fn folder_cache_futures_remain_send() {
        fn require_send(_: impl Future + Send) {}
        // Type-check the public futures without polling or signing in.
        let _check = |client: &Client| {
            require_send(client.cache_folder("inbox", 1, 10));
            require_send(client.index_folder("inbox", 1, 10));
        };
    }

    fn apply_competing_event(cache: &Cache, event_action: u8) {
        let mut updated = cached_message("m1");
        updated.subject = "newsubject".into();
        updated.label_ids = vec!["5".into()];
        updated.unread = 1;
        cache
            .apply_event_batch(
                "old",
                &EventBatch {
                    event_id: "new".into(),
                    more: false,
                    refresh: false,
                    messages: vec![MessageEvent {
                        id: "m1".into(),
                        action: event_action,
                        message: (event_action != action::DELETE).then_some(updated.clone()),
                    }],
                    counts: Vec::new(),
                },
            )
            .unwrap();
        if event_action != action::DELETE {
            // A newer index may have finished too; a failed stale read must not
            // invalidate it or restore the older headers/labels.
            cache.index_message(&updated, "newbody").unwrap();
        }
    }

    fn assert_competing_event_preserved(cache: &Cache, event_action: u8) {
        assert_eq!(cache.last_event_id().unwrap().as_deref(), Some("new"));
        assert!(cache.list("0", false, 10, 0).unwrap().is_empty());
        assert!(cache.search("downloadedbody", 10).unwrap().is_empty());
        assert!(cache.search("oldbody", 10).unwrap().is_empty());
        if event_action == action::DELETE {
            assert!(cache.list("5", false, 10, 0).unwrap().is_empty());
        } else {
            let current = cache.search("newbody", 10).unwrap();
            assert_eq!(current.len(), 1);
            assert_eq!(current[0].subject, "newsubject");
            assert_eq!(current[0].unread, 1);
            assert_eq!(current[0].label_ids, ["5"]);
        }
    }

    #[tokio::test]
    async fn index_rejects_stale_listing_and_body_results() {
        for event_action in [action::DELETE, action::UPDATE, action::UPDATE_FLAGS] {
            for during_listing in [true, false] {
                for failed_read in [false, true] {
                    let cache = Cache::open(Path::new(":memory:")).unwrap();
                    cache.set_last_event_id("old").unwrap();
                    cache
                        .index_message(&cached_message("m1"), "oldbody")
                        .unwrap();
                    let http = ListingDoer {
                        pages: vec![vec![cached_message("m1")]],
                        gate: Semaphore::new(usize::from(!during_listing)),
                    };
                    let body_gate = Semaphore::new(usize::from(during_listing));
                    let index = index_cache(&http, &cache, "inbox", 1, 10, |id| {
                        let body_gate = &body_gate;
                        async move {
                            let permit = body_gate.acquire().await.unwrap();
                            permit.forget();
                            if failed_read {
                                Err(Error::Other("download failed".into()))
                            } else {
                                Ok(full_message(cached_message(&id)))
                            }
                        }
                    });
                    futures::pin_mut!(index);
                    // Deterministically pause either the listing or body request;
                    // no timing sleeps or live account are needed.
                    assert!(futures::poll!(&mut index).is_pending());
                    apply_competing_event(&cache, event_action);
                    if during_listing {
                        http.gate.add_permits(1);
                    } else {
                        body_gate.add_permits(1);
                    }
                    assert!(
                        matches!(index.await, Err(Error::Cache(message)) if message.contains("retry"))
                    );
                    assert_competing_event_preserved(&cache, event_action);
                }
            }
        }
    }

    #[tokio::test]
    async fn index_keeps_the_page_cursor_after_an_earlier_body_commits() {
        let cache = Cache::open(Path::new(":memory:")).unwrap();
        cache.set_last_event_id("old").unwrap();
        let http = ListingDoer {
            pages: vec![vec![cached_message("m0"), cached_message("m1")]],
            gate: Semaphore::new(1),
        };
        let body_gate = Semaphore::new(0);
        let index = index_cache(&http, &cache, "inbox", 1, 10, |id| {
            let body_gate = &body_gate;
            async move {
                if id == "m1" {
                    let permit = body_gate.acquire().await.unwrap();
                    permit.forget();
                }
                Ok(full_message(cached_message(&id)))
            }
        });
        futures::pin_mut!(index);
        assert!(futures::poll!(&mut index).is_pending());
        assert_eq!(cache.search("downloadedbody", 10).unwrap()[0].id, "m0");
        apply_competing_event(&cache, action::DELETE);
        body_gate.add_permits(1);
        assert!(matches!(index.await, Err(Error::Cache(message)) if message.contains("retry")));
        let remaining = cache.list("0", false, 10, 0).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, "m0");
        assert_eq!(cache.search("downloadedbody", 10).unwrap()[0].id, "m0");
        assert_eq!(cache.last_event_id().unwrap().as_deref(), Some("new"));
    }

    #[tokio::test]
    async fn backfill_rejects_stale_page_without_inserting_any_rows() {
        for event_action in [action::DELETE, action::UPDATE, action::UPDATE_FLAGS] {
            let cache = Cache::open(Path::new(":memory:")).unwrap();
            cache.set_last_event_id("old").unwrap();
            cache
                .index_message(&cached_message("m1"), "oldbody")
                .unwrap();
            let http = ListingDoer {
                pages: vec![vec![cached_message("m2"), cached_message("m1")]],
                gate: Semaphore::new(0),
            };
            let backfill = backfill_cache(&http, &cache, "inbox", 1, 10);
            futures::pin_mut!(backfill);
            assert!(futures::poll!(&mut backfill).is_pending());
            apply_competing_event(&cache, event_action);
            http.gate.add_permits(1);
            assert!(
                matches!(backfill.await, Err(Error::Cache(message)) if message.contains("retry"))
            );
            assert_competing_event_preserved(&cache, event_action);
        }
    }

    #[tokio::test]
    async fn backfill_and_index_keep_paging_without_cursor_changes() {
        for cursor in [None, Some("old")] {
            let cache = Cache::open(Path::new(":memory:")).unwrap();
            if let Some(cursor) = cursor {
                cache.set_last_event_id(cursor).unwrap();
            }
            let http = ListingDoer {
                pages: vec![vec![cached_message("m1")], vec![cached_message("m2")]],
                gate: Semaphore::new(6),
            };
            assert_eq!(
                backfill_cache(&http, &cache, "inbox", 3, 1).await.unwrap(),
                2
            );
            assert_eq!(cache.count("0").unwrap(), (2, 0));
            assert_eq!(
                index_cache(&http, &cache, "inbox", 3, 1, |id| async move {
                    Ok(full_message(cached_message(&id)))
                })
                .await
                .unwrap(),
                2
            );
            assert_eq!(cache.search("downloadedbody", 10).unwrap().len(), 2);
            assert_eq!(cache.last_event_id().unwrap().as_deref(), cursor);
        }
    }

    #[tokio::test]
    async fn failed_index_read_atomically_refreshes_metadata_and_invalidates_body() {
        let cache = Cache::open(Path::new(":memory:")).unwrap();
        cache.set_last_event_id("old").unwrap();
        cache
            .index_message(&cached_message("m1"), "oldbody")
            .unwrap();
        let mut updated = cached_message("m1");
        updated.unread = 1;
        let http = ListingDoer {
            pages: vec![vec![updated]],
            gate: Semaphore::new(1),
        };
        assert_eq!(
            index_cache(&http, &cache, "inbox", 1, 10, |_| async {
                Err(Error::Other("download failed".into()))
            })
            .await
            .unwrap(),
            0
        );
        assert!(cache.search("oldbody", 10).unwrap().is_empty());
        assert_eq!(cache.list("0", false, 10, 0).unwrap()[0].unread, 1);
        assert_eq!(cache.last_event_id().unwrap().as_deref(), Some("old"));
    }

    #[tokio::test]
    async fn server_refresh_rebuilds_all_metadata_before_advancing_cursor() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/core/v4/events/old"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Code": 1000, "EventID": "ignored", "Refresh": 1, "More": 0,
                "Messages": [{"ID": "ghost", "Action": 1, "Message": message("ghost")}]
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/core/v4/events/latest"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Code": 1000, "EventID": "fresh"
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/mail/v4/messages"))
            .and(query_param("Page", "0"))
            .and(query_param("PageSize", "100"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Code": 1000, "Total": 2,
                "Messages": [message("m1"), message("m2")]
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/core/v4/events/fresh"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Code": 1000, "EventID": "caught-up", "More": 0,
                "Messages": [{"ID": "m3", "Action": 1, "Message": message("m3")}]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let cache = Cache::open(Path::new(":memory:")).unwrap();
        cache.set_last_event_id("old").unwrap();
        cache
            .index_message(&cached_message("stale"), "oldprivatebody")
            .unwrap();
        let report = sync_cache(&HttpClient::new(server.uri(), "Other"), &cache)
            .await
            .unwrap();

        let mut ids: Vec<String> = cache
            .list("0", false, 10, 0)
            .unwrap()
            .into_iter()
            .map(|m| m.id)
            .collect();
        ids.sort();
        assert_eq!(ids, ["m1", "m2", "m3"]);
        assert!(cache.search("oldprivatebody", 10).unwrap().is_empty());
        assert_eq!(cache.last_event_id().unwrap().as_deref(), Some("caught-up"));
        assert_eq!(report.rebuilt, Some(2));
        assert_eq!(report.event_id, "caught-up");
        assert_eq!((report.created, report.updated, report.deleted), (1, 0, 0));
    }

    #[tokio::test]
    async fn rebuild_fetches_every_page() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/core/v4/events/latest"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Code": 1000, "EventID": "fresh"
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/mail/v4/messages"))
            .and(query_param("Page", "0"))
            .and(query_param("PageSize", "2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Code": 1000, "Total": 3,
                "Messages": [message("m1"), message("m2")]
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/mail/v4/messages"))
            .and(query_param("Page", "1"))
            .and(query_param("PageSize", "2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Code": 1000, "Total": 3, "Messages": [message("m3")]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let cache = Cache::open(Path::new(":memory:")).unwrap();
        cache.set_last_event_id("old").unwrap();
        let (cursor, count) =
            rebuild_cache(&HttpClient::new(server.uri(), "Other"), &cache, "old", 2)
                .await
                .unwrap();
        assert_eq!(cursor, "fresh");
        assert_eq!(count, 3);
        assert_eq!(cache.count("0").unwrap(), (3, 0));
        assert_eq!(cache.last_event_id().unwrap().as_deref(), Some("fresh"));
    }

    #[tokio::test]
    async fn interrupted_rebuild_preserves_offline_cache_and_cursor() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/core/v4/events/latest"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Code": 1000, "EventID": "fresh"
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/mail/v4/messages"))
            .and(query_param("Page", "0"))
            .and(query_param("PageSize", "2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Code": 1000, "Total": 3,
                "Messages": [message("m1"), message("m2")]
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/mail/v4/messages"))
            .and(query_param("Page", "1"))
            .and(query_param("PageSize", "2"))
            .respond_with(ResponseTemplate::new(503).set_body_json(serde_json::json!({
                "Code": 503, "Error": "unavailable"
            })))
            .expect(1)
            .mount(&server)
            .await;

        let cache = Cache::open(Path::new(":memory:")).unwrap();
        cache.set_last_event_id("old").unwrap();
        cache
            .index_message(&cached_message("keep"), "oldprivatebody")
            .unwrap();
        assert!(
            rebuild_cache(&HttpClient::new(server.uri(), "Other"), &cache, "old", 2)
                .await
                .is_err()
        );
        assert_eq!(cache.last_event_id().unwrap().as_deref(), Some("old"));
        assert_eq!(cache.list("0", false, 10, 0).unwrap()[0].id, "keep");
        assert_eq!(cache.search("oldprivatebody", 10).unwrap()[0].id, "keep");
    }
}
