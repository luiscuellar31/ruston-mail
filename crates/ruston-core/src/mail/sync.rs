//! The local cache: kept current from Proton's events, filled a folder at a
//! time, indexed for full-text search, and read without Proton.

use super::{Client, read::FullMessage};
use crate::api;
use crate::api::messages::{ListQuery, is_last_page};
use crate::cache::{BatchCounts, Cache, CacheIdentity};
use crate::error::{Error, Result};
use crate::model::enums::{message_flag, resolve_folder, response_code};
use crate::model::message::MessageMetadata;
use crate::session::{KeyringStore, Paths, SecretStore, Session, validate_profile_name};
use crate::transport::Doer;
use futures::{FutureExt, StreamExt, stream};
use std::borrow::{Borrow, Cow};
use std::future::Future;
use std::path::PathBuf;
use std::time::Duration;

const RESYNC_PAGE_SIZE: u32 = 100;
/// How long a resync waits before asking again for a page that failed in a
/// way that may pass, once for each try: so one bad answer among hundreds of
/// pages does not start it over.
const RESYNC_PAGE_RETRIES: [Duration; 2] = [Duration::from_millis(500), Duration::from_secs(2)];
/// How many bodies `index` downloads at once, as conversation reads do.
const INDEX_BODY_FETCH_CONCURRENCY: usize = 3;

/// What one sync changed in the cache.
#[derive(Debug, Clone)]
pub struct SyncReport {
    /// How many messages it added.
    pub created: usize,
    /// How many messages it changed, flag and label changes included.
    pub updated: usize,
    /// Of `updated`, messages whose flags or labels alone changed, so their
    /// indexed bodies are still current.
    pub flags_updated: usize,
    /// How many messages it removed.
    pub deleted: usize,
    /// The last event the cache now reflects, where the next sync starts.
    pub event_id: String,
    /// Whether this was the cache's first sync, which only noted where
    /// Proton's events stand and changed no mail.
    pub initialized: bool,
    /// Number of messages rebuilt after a server-requested full resync.
    pub rebuilt: Option<usize>,
}

impl SyncReport {
    /// A first sync, which only stored the cursor `event_id`.
    fn initialized(event_id: String) -> Self {
        Self {
            initialized: true,
            ..Self::from_counts(BatchCounts::default(), event_id, None)
        }
    }

    /// A sync that applied `counts` and left the cursor at `event_id`, after
    /// rebuilding the cache when the server asked for it.
    fn from_counts(counts: BatchCounts, event_id: String, rebuilt: Option<usize>) -> Self {
        Self {
            created: counts.created,
            updated: counts.updated,
            flags_updated: counts.flags_updated,
            deleted: counts.deleted,
            event_id,
            initialized: false,
            rebuilt,
        }
    }
}

/// What a resync leaves: the cache it rebuilt, handed back to its caller, the
/// cursor it now holds, and how many messages it lists.
struct Rebuilt<C> {
    cache: C,
    cursor: String,
    count: usize,
}

/// Relists every message into `cache`. Like the other cache operations, it
/// takes the cache by value, so no reference to it lives across a request.
///
/// Mail that comes, goes or changes meanwhile does not stop it: the cursor
/// is taken first, so the next sync applies those changes as events. Only
/// what was there all along must be listed, and mail deleted meanwhile
/// moves later messages up into pages already listed, shrinking the total,
/// so a smaller total lists those pages again.
async fn rebuild_cache<D: Doer, C: Borrow<Cache>>(
    http: &D,
    cache: C,
    previous_cursor: &str,
    page_size: u32,
) -> Result<Rebuilt<C>> {
    let cursor = api::events::get_latest_event_id(http).await?;
    cache.borrow().begin_rebuild()?;
    let mut last_total = None;
    let mut changed = false;
    let mut page = 0;
    let total = loop {
        let query = ListQuery {
            page: Some(page),
            page_size: Some(page_size),
            ..Default::default()
        };
        let (total, messages) = resync_page(http, &query).await?;
        let listed_before = u64::from(page) * u64::from(page_size);
        if messages.is_empty() && listed_before < u64::from(total) {
            return Err(Error::cache_changed(
                "incomplete mailbox listing during resync",
            ));
        }
        cache.borrow().stage_rebuild_page(&messages)?;
        let shrunk = last_total.map_or(0, |before: u32| before.saturating_sub(total));
        changed |= last_total.is_some_and(|before| before != total);
        last_total = Some(total);
        if shrunk > 0 {
            page = page.saturating_sub(shrunk.div_ceil(page_size));
            continue;
        }
        if listed_before + u64::from(page_size) >= u64::from(total) {
            break total;
        }
        page += 1;
    };
    // A total that changed cannot be checked against what was staged.
    let expected = (!changed).then_some(total);
    let count = cache
        .borrow()
        .finish_rebuild(previous_cursor, &cursor, expected)?;
    Ok(Rebuilt {
        cache,
        cursor,
        count,
    })
}

/// One page of a resync's listing, asked for again after each of
/// [`RESYNC_PAGE_RETRIES`] while it fails in a way that may pass, Proton
/// busy or down or the connection; any other failure, or the last, ends it.
async fn resync_page<D: Doer>(http: &D, query: &ListQuery) -> Result<(u32, Vec<MessageMetadata>)> {
    let mut waits = RESYNC_PAGE_RETRIES.iter();
    loop {
        match api::messages::list_messages(http, query).await {
            Err(error) if error.may_pass() => match waits.next() {
                Some(wait) => tokio::time::sleep(*wait).await,
                None => return Err(error),
            },
            listed => return listed,
        }
    }
}

/// Applies Proton's events since the cached cursor. The cache comes by value
/// so the future stays `Send`: SQLite connections are not `Sync`.
async fn sync_cache<D: Doer, C: Borrow<Cache>>(http: &D, mut cache: C) -> Result<SyncReport> {
    let saved = cache.borrow().last_event_id()?;
    let mut cursor = match saved {
        Some(cursor) => cursor,
        None => {
            // A first sync only notes where the events stand; the next one
            // applies whatever happens after.
            let latest = api::events::get_latest_event_id(http).await?;
            if cache.borrow().initialize_cursor(&latest)? {
                tracing::info!(target: "ruston_core::sync", event_id = %latest, "sync initialized cursor");
                return Ok(SyncReport::initialized(latest));
            }
            cache.borrow().last_event_id()?.ok_or_else(|| {
                Error::cache_changed("sync cursor disappeared during initialization")
            })?
        }
    };

    let mut counts = BatchCounts::default();
    let mut rebuilt = None;
    loop {
        let batch = api::events::get_events(http, &cursor).await?;
        if batch.refresh {
            if rebuilt.is_some() {
                return Err(Error::cache_changed("server requested another resync"));
            }
            let resynced = rebuild_cache(http, cache, &cursor, RESYNC_PAGE_SIZE).await?;
            tracing::warn!(target: "ruston_core::sync", rebuilt = resynced.count, "server requested full resync — cache rebuilt");
            cache = resynced.cache;
            cursor = resynced.cursor;
            rebuilt = Some(resynced.count);
            // What earlier batches changed is part of the rebuild, which
            // `rebuilt` reports.
            counts = BatchCounts::default();
            continue;
        }
        counts += cache.borrow().apply_event_batch(&cursor, &batch)?;
        cursor = batch.event_id;
        if !batch.more {
            break;
        }
    }
    tracing::info!(target: "ruston_core::sync", created = counts.created, updated = counts.updated, deleted = counts.deleted, event_id = %cursor, "sync complete");
    Ok(SyncReport::from_counts(counts, cursor, rebuilt))
}

/// Pins the event cursor before a cache's first listing. Rows stored without
/// one were never reconciled: the first sync only set the cursor to the newest
/// event, so a message deleted since the listing stayed searchable.
/// The cache passes through by value, so no reference to it lives across the
/// request and the callers' futures stay `Send`.
async fn pin_cursor<D: Doer, C: Borrow<Cache>>(http: &D, cache: C) -> Result<C> {
    if cache.borrow().last_event_id()?.is_none() {
        let latest = api::events::get_latest_event_id(http).await?;
        // Another process may have pinned one meanwhile; either predates the
        // listing.
        cache.borrow().initialize_cursor(&latest)?;
    }
    Ok(cache)
}

/// The cursor a page's rows are stored under, which `pin_cursor` set.
fn page_cursor(cache: &Cache) -> Result<String> {
    cache
        .last_event_id()?
        .ok_or_else(|| Error::cache_changed("sync cursor disappeared during cache download"))
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
    let cache = pin_cursor(http, cache).await?;
    let mut cached = 0usize;
    for page in 0..max_pages {
        let query = ListQuery::folder_page(label.clone(), page, page_size);
        let cursor = page_cursor(cache.borrow())?;
        let (_total, messages) = api::messages::list_messages(http, &query).await?;
        if messages.is_empty() {
            break;
        }
        cache
            .borrow()
            .store_metadata_page(Some(&cursor), &messages)?;
        cached += messages.len();
        if is_last_page(messages.len(), page_size) {
            break;
        }
    }
    tracing::info!(target: "ruston_core::sync", folder, cached, "cache_folder backfill");
    Ok(cached)
}

/// What the local index keeps of a message's body: an HTML body's text,
/// without the addresses its links lead to, or a plain one as it is.
fn searchable_body(full: &FullMessage) -> Cow<'_, str> {
    if crate::html::is_html_mime(&full.mime_type) {
        Cow::Owned(crate::html::searchable_text(&full.body))
    } else {
        Cow::Borrowed(&full.body)
    }
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
    let cache = pin_cursor(http, cache).await?;
    let mut indexed = 0usize;
    for page in 0..max_pages {
        let query = ListQuery::folder_page(label.clone(), page, page_size);
        // Keep the page cursor for every body, including failed reads.
        let cursor = page_cursor(cache.borrow())?;
        let (_total, messages) = api::messages::list_messages(http, &query).await?;
        if messages.is_empty() {
            break;
        }
        let listed = messages.len();
        // Mail sent or received keeps its body, so one indexed before stays
        // and only its metadata is brought up to date; a draft may have been
        // edited, so it is read again.
        let mut kept = Vec::new();
        let mut to_read = Vec::with_capacity(listed);
        for meta in messages {
            if !is_draft(&meta) && cache.borrow().has_indexed_body(&meta.id)? {
                kept.push(meta);
            } else {
                to_read.push(meta);
            }
        }
        if !kept.is_empty() {
            cache.borrow().store_metadata_page(Some(&cursor), &kept)?;
            indexed += kept.len();
        }
        // A few bodies download at once, and are saved in listing order, so
        // a stop leaves exactly the messages before it saved.
        let mut reads = stream::iter(to_read)
            .map(|meta| read_message(meta.id.clone()).map(move |read| (meta, read)))
            .buffered(INDEX_BODY_FETCH_CONCURRENCY);
        while let Some((meta, read)) = reads.next().await {
            match read {
                Ok(full) => {
                    cache.borrow().store_index_result(
                        Some(&cursor),
                        &full.meta,
                        Some(&searchable_body(&full)),
                    )?;
                    indexed += 1;
                }
                // The message is gone or its body cannot be opened, so an
                // older body must not stay searchable.
                Err(error) if is_definitive_read_failure(&error) => {
                    cache
                        .borrow()
                        .store_index_result(Some(&cursor), &meta, None)?;
                }
                // Anything else says nothing about the body, and would most
                // likely fail the next read too: keep what is indexed and stop.
                Err(error) => {
                    tracing::info!(target: "ruston_core::sync", folder, indexed, error = %error, "local search indexing stopped");
                    return Err(error);
                }
            }
        }
        if is_last_page(listed, page_size) {
            break;
        }
    }
    tracing::info!(target: "ruston_core::sync", folder, indexed, "local search index built");
    Ok(indexed)
}

/// Whether `meta` is a draft, whose body may still change: Proton marks the
/// mail it sent or received, and a draft is neither.
fn is_draft(meta: &MessageMetadata) -> bool {
    !message_flag::has(meta.flags, message_flag::RECEIVED | message_flag::SENT)
}

/// Whether reading a body failed because of the message itself: it is gone,
/// or its body cannot be decrypted or parsed, so reading it again would fail
/// the same way. A lost connection, an expired session or rate limiting says
/// nothing about the body.
fn is_definitive_read_failure(error: &Error) -> bool {
    match error {
        // Proton's own clients take a 422 for a full message as "gone".
        Error::Api(api) => {
            matches!(
                api.code,
                response_code::MESSAGE_DOES_NOT_EXIST | response_code::INVALID_ID
            ) || (api.http_status == 422 && api.code != response_code::HUMAN_VERIFICATION)
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
        | Error::SessionAddressMismatch
        | Error::MailboxPasswordRequired
        | Error::SecurityKeyRequired
        | Error::UserKeysLocked
        | Error::Io(_)
        | Error::Cache(_)
        | Error::CacheChanged(_)
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
    let total = cache.listed_count(&label, unread)?;
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
    /// Opens the cache of the account saved under `profile`, which the
    /// saved session names, without asking Proton.
    pub fn open(profile: &str) -> Result<Self> {
        validate_profile_name(profile)?;
        let store = KeyringStore::new(profile.to_owned());
        Self::open_saved(&Paths::system()?, &store, profile, |identity| {
            Cache::default_account_path(profile, identity)
        })
    }

    fn open_saved(
        paths: &Paths,
        store: &dyn SecretStore,
        profile: &str,
        path: impl FnOnce(&CacheIdentity) -> Result<PathBuf>,
    ) -> Result<Self> {
        let loaded = Session::load(paths, profile, store)?.ok_or(Error::Unauthorized)?;
        let identity = CacheIdentity::new(&loaded.session.base_url, &loaded.session.user_id)?;
        let cache = Cache::open_for_account(&path(&identity)?, &identity)?;
        Ok(Self { cache })
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

    /// Brings the local cache up to date from Proton's events since its last
    /// sync, and reports what changed.
    pub async fn sync(&self) -> Result<SyncReport> {
        sync_cache(self.http(), self.open_cache()?).await
    }

    /// Lists a folder into the cache, `page_size` messages at a time and at
    /// most `max_pages` pages, and gives how many it stored.
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

    /// Builds the full-text index of a folder: lists it, `page_size` messages
    /// at a time and at most `max_pages` pages, and indexes each message with
    /// its decrypted body. A body indexed before is kept rather than
    /// downloaded again, unless it is a draft's, which may have changed; the
    /// count includes the bodies kept.
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

    /// Searches the full-text index on this device, which holds the decrypted
    /// bodies, so the query never reaches Proton: up to `limit` messages
    /// holding every word of `query`, best match first.
    pub fn search_local(&self, query: &str, limit: u32) -> Result<Vec<MessageMetadata>> {
        self.open_cache()?.search(query, limit)
    }

    /// Full-text search of the local index without changing it, for a
    /// program that keeps no mail on disk itself: nothing is created,
    /// upgraded or written, and SQLite runs on the blocking pool. `None`
    /// when this device holds no index for the account.
    pub async fn search_local_read_only(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Option<Vec<MessageMetadata>>> {
        let path = Cache::existing_account_path(&self.profile, &self.cache_identity)?;
        let (identity, query) = (self.cache_identity.clone(), query.to_owned());
        super::off_thread(move || {
            Cache::open_existing_for_account(&path, &identity)?
                .map(|cache| cache.search(&query, limit))
                .transpose()
        })
        .await
    }
}

#[cfg(test)]
mod tests;
