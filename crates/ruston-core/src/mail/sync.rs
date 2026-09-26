//! Incremental sync (event stream) + local-cache reads.

use super::Client;
use crate::api;
use crate::api::events::action;
use crate::api::messages::ListQuery;
use crate::cache::Cache;
use crate::error::{Error, Result};
use crate::model::enums::resolve_folder;
use crate::model::message::MessageMetadata;
use crate::transport::Doer;

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
            cache.set_last_event_id(&latest)?;
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
        for ev in &batch.messages {
            match ev.action {
                action::DELETE => {
                    cache.delete_message(&ev.id)?;
                    deleted += 1;
                }
                action::CREATE | action::UPDATE => {
                    if ev.action == action::UPDATE {
                        // Events carry metadata, not the decrypted body. Do not
                        // leave an older body searchable after a full update.
                        cache.invalidate_index(&ev.id)?;
                    }
                    if let Some(m) = &ev.message {
                        cache.upsert_message(m)?;
                        if ev.action == action::CREATE {
                            created += 1;
                        } else {
                            updated += 1;
                        }
                    }
                }
                action::UPDATE_FLAGS => {
                    if let Some(m) = &ev.message {
                        cache.upsert_message_flags(m)?;
                        updated += 1;
                    }
                }
                _ => {}
            }
        }
        cache.set_last_event_id(&batch.event_id)?;
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

impl Client {
    pub(crate) fn open_cache(&self) -> Result<Cache> {
        let path = Cache::default_path(&self.profile)?;
        Cache::open(&path)
    }

    /// Apply incremental events from the cached cursor into the local cache.
    pub async fn sync(&self) -> Result<SyncReport> {
        let cache = self.open_cache()?;
        sync_cache(self.http(), &cache).await
    }

    /// Backfill a folder into the cache by paging the API (bounded by `max_pages`).
    pub async fn cache_folder(
        &self,
        folder: &str,
        max_pages: u32,
        page_size: u32,
    ) -> Result<usize> {
        let cache = self.open_cache()?;
        let label = resolve_folder(folder);
        let mut n = 0usize;
        for page in 0..max_pages {
            let q = ListQuery {
                label_id: Some(label.clone()),
                page: Some(page),
                page_size: Some(page_size),
                ..Default::default()
            };
            let (_total, msgs) = api::messages::list_messages(self.http(), &q).await?;
            if msgs.is_empty() {
                break;
            }
            for m in &msgs {
                cache.upsert_message(m)?;
                n += 1;
            }
            if (msgs.len() as u32) < page_size {
                break;
            }
        }
        tracing::info!(target: "ruston_core::sync", folder, cached = n, "cache_folder backfill");
        Ok(n)
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

    /// Build the local encrypted-search index: page a folder, decrypt each
    /// message body, and index it (bounded by `max_pages`).
    pub async fn index_folder(
        &self,
        folder: &str,
        max_pages: u32,
        page_size: u32,
    ) -> Result<usize> {
        let cache = self.open_cache()?;
        let label = resolve_folder(folder);
        let mut n = 0usize;
        for page in 0..max_pages {
            let q = ListQuery {
                label_id: Some(label.clone()),
                page: Some(page),
                page_size: Some(page_size),
                ..Default::default()
            };
            let (_total, msgs) = api::messages::list_messages(self.http(), &q).await?;
            if msgs.is_empty() {
                break;
            }
            for meta in &msgs {
                match self.read_message(&meta.id).await {
                    Ok(full) => {
                        cache.index_message(&full.meta, &full.body)?;
                        n += 1;
                    }
                    Err(_) => {
                        // A failed refresh must not keep an older body searchable.
                        cache.invalidate_index(&meta.id)?;
                        cache.upsert_message(meta)?;
                    }
                }
            }
            if (msgs.len() as u32) < page_size {
                break;
            }
        }
        tracing::info!(target: "ruston_core::sync", folder, indexed = n, "encrypted-search index built");
        Ok(n)
    }

    /// Full-text search the local index (decrypted bodies; private + offline).
    pub fn search_local(&self, query: &str, limit: u32) -> Result<Vec<MessageMetadata>> {
        self.open_cache()?.search(query, limit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::HttpClient;
    use std::path::Path;
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
