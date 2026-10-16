//! Incremental sync (event stream) + local-cache reads.

use super::{Client, read::FullMessage};
use crate::api;
use crate::api::messages::ListQuery;
use crate::cache::{Cache, CacheIdentity};
use crate::error::{Error, Result};
use crate::model::enums::resolve_folder;
use crate::model::message::MessageMetadata;
use crate::session::{KeyringStore, Paths, SecretStore, Session, validate_profile_name};
use crate::transport::Doer;
use std::borrow::Borrow;
use std::future::Future;
use std::path::PathBuf;

const RESYNC_PAGE_SIZE: u32 = 100;

/// Outcome of a sync run.
#[derive(Debug, Clone)]
pub struct SyncReport {
    /// Number of messages created.
    pub created: usize,
    /// Number of messages updated, flag and label changes included.
    pub updated: usize,
    /// Of `updated`, messages whose flags or labels alone changed, so their
    /// indexed bodies are still current.
    pub flags_updated: usize,
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
                    flags_updated: 0,
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

    let (mut created, mut updated, mut flags_updated, mut deleted) = (0, 0, 0, 0);
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
            flags_updated = 0;
            deleted = 0;
            continue;
        }
        let counts = cache.apply_event_batch(&id, &batch)?;
        created += counts.created;
        updated += counts.updated;
        flags_updated += counts.flags_updated;
        deleted += counts.deleted;
        id = batch.event_id;
        if !batch.more {
            break;
        }
    }
    tracing::info!(target: "ruston_core::sync", created, updated, deleted, event_id = %id, "sync complete");
    Ok(SyncReport {
        created,
        updated,
        flags_updated,
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
                // The message is gone or its body cannot be opened, so an
                // older body must not stay searchable.
                Err(error) if is_definitive_read_failure(&error) => {
                    cache
                        .borrow()
                        .store_index_result(cursor.as_deref(), meta, None)?;
                }
                // Anything else says nothing about the body, and would most
                // likely fail the next read too: keep what is indexed and stop.
                Err(error) => {
                    tracing::info!(target: "ruston_core::sync", folder, indexed = n, error = %error, "local search indexing stopped");
                    return Err(error);
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

/// Proton's code for a message that does not exist.
const MESSAGE_DOES_NOT_EXIST: i64 = 2501;
/// Proton's code for an ID it does not accept.
const INVALID_ID: i64 = 2061;
/// Proton's code for a request that needs human verification.
const HUMAN_VERIFICATION_REQUIRED: i64 = 9001;

/// Whether reading a body failed because of the message itself: it is gone,
/// or its body cannot be decrypted or parsed, so reading it again would fail
/// the same way. A lost connection, an expired session or rate limiting says
/// nothing about the body.
fn is_definitive_read_failure(error: &Error) -> bool {
    match error {
        // Proton's own clients take a 422 for a full message as "gone".
        Error::Api(api) => {
            matches!(api.code, MESSAGE_DOES_NOT_EXIST | INVALID_ID)
                || (api.http_status == 422 && api.code != HUMAN_VERIFICATION_REQUIRED)
        }
        Error::NotFound { .. }
        | Error::Crypto(_)
        | Error::Json(_)
        | Error::ResponseTooLarge { .. }
        // Only the decryption worker fails this way on a body read.
        | Error::Other(_) => true,
        Error::Unauthorized
        | Error::HumanVerification(_)
        | Error::Http(_)
        | Error::Session(_)
        | Error::Io(_)
        | Error::Cache(_)
        | Error::Srp(_) => false,
        // A body read never fails these ways. Should that change, stop rather
        // than drop bodies.
        Error::AttachmentBatchTooLarge { .. }
        | Error::AttachmentTooLarge { .. }
        | Error::AttachmentUnavailable { .. }
        | Error::AttachmentVerificationFailed
        | Error::SendUnconfirmed { .. }
        | Error::Ambiguous(_) => false,
    }
}

fn cached_page(
    cache: &Cache,
    folder: &str,
    unread: bool,
    page: u32,
    page_size: u32,
) -> Result<(u32, Vec<MessageMetadata>)> {
    let label = resolve_folder(folder);
    let (all, unread_only) = cache.count(&label)?;
    let total = if unread { unread_only } else { all };
    // A page past the end of what a u32 can address is past the cache too.
    let messages = cache.list(&label, unread, page_size, page.saturating_mul(page_size))?;
    Ok((u32::try_from(total).unwrap_or(u32::MAX), messages))
}

/// The local cache of the account signed in under a profile, opened without
/// signing in again, so reading cached mail and searching it work offline.
pub struct LocalCache {
    cache: Cache,
}

impl LocalCache {
    /// Opens the cache of the account saved under `profile`. Only a session
    /// saved before its account ID was recorded needs Proton: it is resumed
    /// once, which records the ID for next time.
    pub async fn open(profile: &str) -> Result<Self> {
        validate_profile_name(profile)?;
        let store = KeyringStore::new(profile.to_owned());
        let saved = Self::open_saved(&Paths::system()?, &store, profile, |identity| {
            Cache::default_account_path(profile, identity)
        })?;
        match saved {
            Some(cache) => Ok(cache),
            None => Ok(Self {
                cache: Client::resume(profile).await?.open_cache()?,
            }),
        }
    }

    /// `None` when the saved session does not name its account.
    fn open_saved(
        paths: &Paths,
        store: &dyn SecretStore,
        profile: &str,
        path: impl FnOnce(&CacheIdentity) -> Result<PathBuf>,
    ) -> Result<Option<Self>> {
        let loaded = Session::load(paths, profile, store)?.ok_or(Error::Unauthorized)?;
        let Some(user_id) = loaded.session.user_id.as_deref() else {
            return Ok(None);
        };
        let identity = CacheIdentity::new(&loaded.session.base_url, user_id)?;
        let cache = Cache::open_for_account(&path(&identity)?, &identity)?;
        Ok(Some(Self { cache }))
    }

    /// One page of a folder, with how many cached messages it holds: unread
    /// ones only when `unread` is set.
    pub fn messages_page(
        &self,
        folder: &str,
        unread: bool,
        page: u32,
        page_size: u32,
    ) -> Result<(u32, Vec<MessageMetadata>)> {
        cached_page(&self.cache, folder, unread, page, page_size)
    }

    /// Full-text search of the indexed bodies. Words are matched together.
    pub fn search(&self, query: &str, limit: u32) -> Result<Vec<MessageMetadata>> {
        self.cache.search(query, limit)
    }
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
    /// A message that is gone, or whose body cannot be decrypted, loses any
    /// body indexed before. Any other failed read, such as a lost connection
    /// or rate limiting, stops indexing with its error and leaves that
    /// message as it was. Returns a retry error if sync changes the page's
    /// cursor before a result is saved. Either way, previously committed
    /// messages remain indexed.
    pub async fn index_folder(
        &self,
        folder: &str,
        max_pages: u32,
        page_size: u32,
    ) -> Result<usize> {
        self.index_into(self.open_cache()?, folder, max_pages, page_size)
            .await
    }

    /// The index keeps bodies, not verdicts, so it reads them without looking
    /// up sender keys to check a signature nobody would see.
    async fn index_into(
        &self,
        cache: impl Borrow<Cache>,
        folder: &str,
        max_pages: u32,
        page_size: u32,
    ) -> Result<usize> {
        index_cache(
            self.http(),
            cache,
            folder,
            max_pages,
            page_size,
            |id| async move { self.read_message_unverified(&id).await },
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
            reply_tos: Vec::new(),
        }
    }

    #[tokio::test]
    async fn indexing_reads_bodies_without_looking_up_sender_keys() {
        use crate::cache::CacheIdentity;
        use crate::crypto::{AddressKeys, KeyStore, StoredKey};
        use crate::session::{MemoryStore, Paths};
        use proton_crypto::crypto::{
            DataEncoding, KeyGenerator, KeyGeneratorAlgorithm, KeyGeneratorSync, PGPProviderSync,
        };
        use secrecy::SecretString;
        use std::sync::Arc;

        let server = MockServer::start().await;
        let provider = crate::crypto::provider();
        let key = provider
            .new_key_generator()
            .with_user_id("Test", "me@proton.me")
            .with_algorithm(KeyGeneratorAlgorithm::ECC)
            .generate()
            .unwrap();
        let armored = String::from_utf8(
            provider
                .private_key_export(&key, "pass", DataEncoding::Armor)
                .unwrap()
                .as_ref()
                .to_vec(),
        )
        .unwrap();
        let address = AddressKeys {
            address_id: "address".into(),
            email: "me@proton.me".into(),
            keys: vec![StoredKey {
                armored,
                passphrase: SecretString::from("pass"),
                unlocked: None,
            }],
        };
        let body = crate::crypto::encrypt_self_draft(&provider, &address, "indexedbody").unwrap();
        Mock::given(method("GET"))
            .and(path("/mail/v4/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Code": 1000, "Total": 1, "Messages": [message("m1")]
            })))
            .mount(&server)
            .await;
        let mut full = message("m1");
        full["AddressID"] = "address".into();
        full["Body"] = body.into();
        full["MIMEType"] = "text/plain".into();
        Mock::given(method("GET"))
            .and(path("/mail/v4/messages/m1"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"Code": 1000, "Message": full})),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/core/v4/keys/all"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Code": 1000, "Address": { "Keys": [] }
            })))
            .expect(0)
            .mount(&server)
            .await;
        let client = Client {
            http: HttpClient::new(server.uri(), "Other"),
            keys: KeyStore {
                user_keys: Vec::new(),
                addresses: vec![address],
            },
            paths: Paths::with_base(std::env::temp_dir()),
            profile: "test".into(),
            session_uid: "test".into(),
            cache_identity: CacheIdentity::new(&server.uri(), "test").unwrap(),
            store: Arc::new(MemoryStore::default()),
            sender_cache: tokio::sync::Mutex::new(super::super::SenderKeyCache::default()),
        };
        let cache = Cache::open(Path::new(":memory:")).unwrap();

        assert_eq!(client.index_into(&cache, "inbox", 1, 10).await.unwrap(), 1);
        assert_eq!(cache.search("indexedbody", 10).unwrap()[0].id, "m1");
    }

    #[test]
    fn a_saved_account_s_cache_opens_without_contacting_proton() {
        use crate::session::{MemoryStore, Tokens};
        use secrecy::SecretString;

        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let base = std::env::temp_dir().join(format!(
            "ruston-local-cache-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&base);
        let cache_dir = base.join("cache");
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        // The cache refuses a database in a directory others can read.
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        builder.create(&cache_dir).unwrap();
        let paths = Paths::with_base(base.join("config"));
        let store = MemoryStore::new();
        let save = |user_id: Option<&str>| {
            let session = Session {
                uid: "UID1".into(),
                app_version: "Other".into(),
                base_url: "https://mail.proton.me/api".into(),
                password_mode: 1,
                user_agent: None,
                user_id: user_id.map(str::to_owned),
            };
            let tokens = Tokens {
                uid: "UID1".into(),
                access: SecretString::from("acc"),
                refresh: SecretString::from("ref"),
            };
            session
                .save(
                    &paths,
                    "default",
                    &store,
                    &tokens,
                    &SecretString::from("skp"),
                )
                .unwrap();
        };
        let open = || {
            LocalCache::open_saved(
                &paths,
                &store,
                "default",
                |_| Ok(cache_dir.join("cache.db")),
            )
        };

        // Signed out, there is no account to read.
        assert!(matches!(open(), Err(Error::Unauthorized)));
        // A session saved before its account ID was kept is resumed once.
        save(None);
        assert!(open().unwrap().is_none());
        // With the ID, the cache opens and reads without a single request.
        save(Some("user-1"));
        let cache = open().unwrap().unwrap();
        let (total, messages) = cache.messages_page("inbox", false, 0, 25).unwrap();
        assert_eq!((total, messages.len()), (0, 0));
        assert!(cache.search("anything", 10).unwrap().is_empty());
        drop(cache);
        // The database stays bound to the account that first opened it.
        save(Some("user-2"));
        assert!(matches!(open(), Err(Error::Cache(_))));

        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn a_cached_folder_is_read_page_by_page_with_its_real_total() {
        let cache = Cache::open(Path::new(":memory:")).unwrap();
        for index in 0..5_i64 {
            cache
                .upsert_message(&MessageMetadata {
                    unread: i64::from(index < 2),
                    time: index,
                    ..cached_message(&format!("m{index}"))
                })
                .unwrap();
        }
        let ids = |messages: Vec<MessageMetadata>| -> Vec<String> {
            messages.into_iter().map(|message| message.id).collect()
        };

        // Newest first; the total is the folder's, not the page's.
        let (total, first) = cached_page(&cache, "inbox", false, 0, 2).unwrap();
        assert_eq!((total, ids(first)), (5, vec!["m4".into(), "m3".into()]));
        let (total, second) = cached_page(&cache, "inbox", false, 1, 2).unwrap();
        assert_eq!((total, ids(second)), (5, vec!["m2".into(), "m1".into()]));
        let (total, unread) = cached_page(&cache, "inbox", true, 0, 25).unwrap();
        assert_eq!((total, ids(unread)), (2, vec!["m1".into(), "m0".into()]));
        let (total, beyond) = cached_page(&cache, "inbox", false, u32::MAX, 25).unwrap();
        assert_eq!((total, beyond.len()), (5, 0));
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
                                Err(Error::Crypto("body decrypt failed".into()))
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
    async fn an_unreadable_body_atomically_refreshes_metadata_and_invalidates_it() {
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
                Err(Error::Crypto("body decrypt failed".into()))
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
    async fn index_stops_at_an_outage_but_drops_a_missing_message_s_body() {
        let server = MockServer::start().await;
        let mut listed_m1 = message("m1");
        listed_m1["Unread"] = 1.into();
        Mock::given(method("GET"))
            .and(path("/mail/v4/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Code": 1000, "Total": 3,
                "Messages": [message("m0"), listed_m1, message("m2")]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/mail/v4/messages/m0"))
            .respond_with(ResponseTemplate::new(422).set_body_json(
                serde_json::json!({"Code": 2501, "Error": "Message does not exist"}),
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/mail/v4/messages/m1"))
            .respond_with(
                ResponseTemplate::new(503).set_body_json(serde_json::json!({"Code": 503})),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/mail/v4/messages/m2"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"Code": 1000, "Message": message("m2")})),
            )
            .mount(&server)
            .await;
        let http = HttpClient::new(server.uri(), "Other");
        let cache = Cache::open(Path::new(":memory:")).unwrap();
        cache.set_last_event_id("old").unwrap();
        cache
            .index_message(&cached_message("m0"), "goneoldbody")
            .unwrap();
        cache
            .index_message(&cached_message("m1"), "keptoldbody")
            .unwrap();

        let result = index_cache(&http, &cache, "inbox", 1, 10, |id| {
            let http = &http;
            async move {
                let message = api::messages::get_message(http, &id).await?;
                Ok(full_message(message.meta))
            }
        })
        .await;

        assert!(
            matches!(&result, Err(Error::Api(api)) if api.http_status == 503),
            "{result:?}"
        );
        // The message Proton no longer has loses its body.
        assert!(cache.search("goneoldbody", 10).unwrap().is_empty());
        // The outage says nothing about m1: its body and metadata stay.
        let kept = cache.search("keptoldbody", 10).unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!((kept[0].id.as_str(), kept[0].unread), ("m1", 0));
        // And indexing stopped there.
        let cached: Vec<_> = cache
            .list("0", false, 10, 0)
            .unwrap()
            .into_iter()
            .map(|message| message.id)
            .collect();
        assert!(!cached.contains(&"m2".to_owned()), "{cached:?}");
        assert_eq!(cache.last_event_id().unwrap().as_deref(), Some("old"));
    }

    #[test]
    fn only_failures_about_the_message_itself_drop_its_body() {
        let api = |http_status, code| {
            Error::Api(crate::error::ApiError {
                http_status,
                code,
                message: String::new(),
                raw_body: String::new(),
            })
        };
        let http = Error::from(
            reqwest::Client::new()
                .get("://invalid")
                .build()
                .unwrap_err(),
        );
        let challenge = crate::error::HvChallenge {
            token: String::new(),
            methods: Vec::new(),
            web_url: String::new(),
        };
        for (error, definitive) in [
            (api(422, 2501), true),
            (api(404, 2501), true),
            (api(422, 2061), true),
            (api(422, 2001), true),
            (
                Error::NotFound {
                    kind: "message".into(),
                },
                true,
            ),
            (Error::Crypto("body decrypt failed".into()), true),
            (
                Error::Json(serde_json::from_str::<()>("{").unwrap_err()),
                true,
            ),
            (Error::ResponseTooLarge { limit: 1 }, true),
            (Error::Other("decryption worker failed".into()), true),
            (api(422, 9001), false),
            (api(404, 0), false),
            (api(429, 2028), false),
            (api(503, 0), false),
            (Error::Unauthorized, false),
            (Error::HumanVerification(challenge), false),
            (http, false),
            (Error::Session("keyring get: locked".into()), false),
            (Error::Io(std::io::Error::other("disk full")), false),
        ] {
            assert_eq!(is_definitive_read_failure(&error), definitive, "{error:?}");
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
