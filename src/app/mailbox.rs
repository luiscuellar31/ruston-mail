use std::collections::{HashMap, HashSet, VecDeque};

use super::reader::{ConversationReader, ReaderState};
use crate::mail::{
    ConversationDetail, ConversationPage, ConversationSummary, CustomKind, Folder, ListFilter,
    ListOrder, MailAction, MailFolder, MailboxCounts, MailboxError, SummaryKind,
};

/// Identifies an asynchronous mailbox request. Only the response matching the
/// request currently in flight is applied; anything else is stale.
pub type RequestId = u64;

/// The most rows one request to Proton acts on.
const ACTION_GROUP_LIMIT: usize = ruston_core::api::MAX_IDS_PER_REQUEST;

/// Recent readers are kept only for quick backtracking within this session.
const READER_CACHE_CAPACITY: usize = 8;
/// Folders left most recently keep their loaded listing in memory, besides
/// the open one.
const LISTING_CACHE_CAPACITY: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageRequest {
    pub id: RequestId,
    pub folder: Folder,
    /// Which of the folder's conversations it lists.
    pub filter: ListFilter,
    /// The order it lists them in.
    pub order: ListOrder,
    /// Zero-based server page.
    pub page: u32,
    pub page_size: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReaderRequest {
    pub id: RequestId,
    pub conversation_id: String,
    pub kind: SummaryKind,
    /// How many messages the row says it holds; a longer one may take longer.
    pub messages: u32,
}

/// An action sent to the backend for one row, remembered so only its own
/// response is applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionRequest {
    pub id: RequestId,
    pub row_id: String,
    pub kind: SummaryKind,
    /// The folder the action was started from, so a response that no longer
    /// describes what is on screen is dropped.
    pub folder: Folder,
    /// The folder Proton should read the row in. `None` for a row that only
    /// a search found, which can live anywhere the open folder is not.
    pub context: Option<Folder>,
    pub action: MailAction,
}

/// How an automatic read ended, and the action that can be sent after it.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ReadCompletion {
    /// Whether Proton took the read, which changes its counts, whether or
    /// not the row is still listed here.
    pub read_confirmed: bool,
    /// An explicit read that waited for this one, or, when this read failed
    /// and took that read down with it, the action waiting behind them.
    pub next_action: Option<ActionRequest>,
    pub error: Option<MailboxError>,
}

/// What an action shows as soon as it is asked for, and what can be sent.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ActionStart {
    /// The request to send now; `None` while it waits behind another action,
    /// or for an automatic read of the same row.
    pub send: Option<ActionRequest>,
    /// The row opened in place of one the action took out of the view.
    pub opened: Option<String>,
}

/// How an action ended, and the action that can be sent after it.
#[derive(Debug, PartialEq, Eq)]
pub struct ActionFinish {
    pub result: Result<(), MailboxError>,
    pub next_action: Option<ActionRequest>,
    /// The row opened in place of one a reload had brought back while its
    /// move was on the way.
    pub opened: Option<String>,
}

/// A move the user can take back: the rows it took, together, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoMove {
    /// The rows moved, in the order of the list, which Undo puts back.
    pub rows: Vec<MovedRow>,
    /// Where they were moved, which is what the offer says.
    pub to: Folder,
}

/// One row a move took out of a folder, and how to put it back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MovedRow {
    pub row_id: String,
    pub kind: SummaryKind,
    /// The folder the row was moved out of, and goes back to.
    pub from: Folder,
    /// The row as it was listed, so taking the move back can list it again
    /// even when it came from a page a reload does not read.
    pub row: Option<ConversationSummary>,
}

/// Which way the keyboard moves through the conversation list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Next,
    Previous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListStatus {
    /// First page of the selected folder in flight; nothing to show yet.
    Loading(RequestId),
    /// First page in flight while the previous list stays visible.
    Refreshing(RequestId),
    LoadingMore(RequestId),
    Loaded,
    Failed(MailboxError, ListAttempt),
}

impl ListStatus {
    /// The list request this status waits for, if one is in flight.
    pub fn request_in_flight(self) -> Option<RequestId> {
        match self {
            Self::Loading(request) | Self::Refreshing(request) | Self::LoadingMore(request) => {
                Some(request)
            }
            Self::Loaded | Self::Failed(..) => None,
        }
    }
}

/// What a list request that failed asked for, so trying again asks the same.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListAttempt {
    /// The folder's first page, on opening or refreshing it.
    Folder,
    /// The next page of the folder or of the search results.
    More,
    /// A server search.
    Search,
}

/// One server search: what was asked, and what came back. Results replace the
/// folder listing while they are shown, and never merge into it.
struct SearchState {
    query: String,
    rows: Vec<ConversationSummary>,
    row_ids: HashSet<String>,
    next_page: u32,
    has_more: bool,
    /// Found in this device's index, Proton being out of reach.
    on_device: bool,
}

/// Why a search Proton could not answer found nothing on this device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceIndex {
    /// This device holds no index for the account.
    Missing,
    /// The index could not be read.
    Unreadable,
}

/// What a folder's listing holds of the mailbox: kept while another folder
/// is open, and put back when it opens again.
#[derive(Default)]
struct Listing {
    conversations: Vec<ConversationSummary>,
    row_ids: HashSet<String>,
    last_dated_time: Option<i64>,
    next_page: u32,
    has_more: bool,
    groups: ConversationGroups,
}

/// The loaded part of one folder while another folder is open.
/// Memory-only, so Ruston never creates a second persistent mail store.
struct CachedListing {
    folder: FolderKey,
    listing: Listing,
    /// A confirmed mailbox change may have affected this folder. It can still
    /// be shown immediately, but must be refreshed before it is cached clean.
    dirty: bool,
}

struct SplitConversation {
    source: ConversationSummary,
    message_ids: Vec<String>,
}

impl SplitConversation {
    fn matches(&self, source: &ConversationSummary) -> bool {
        // Read/star flags and labels, the folder's among them, do not change
        // threading. Reinspection refreshes each message's own while these
        // rows remain visible.
        let mut current = source.clone();
        current.unread = self.source.unread;
        current.starred = self.source.starred;
        current.labels.clone_from(&self.source.labels);
        current == self.source
    }
}

/// Presentation decisions belong to the loaded folder, not to the session or
/// disk cache. The reverse index also identifies a row's conversation for UI
/// anchoring and keeps reconciliation proportional to the loaded rows.
#[derive(Default)]
struct ConversationGroups {
    splits: HashMap<String, SplitConversation>,
    parents: HashMap<String, String>,
    /// The conversations an inspection left as one, as each was listed then.
    /// Listed the same again, one has nothing new to be asked about.
    kept_whole: HashMap<String, ConversationSummary>,
}

impl ConversationGroups {
    fn forget(&mut self, id: &str) -> Option<SplitConversation> {
        let group = self.splits.remove(id)?;
        for message in &group.message_ids {
            if self.parents.get(message).map(String::as_str) == Some(id) {
                self.parents.remove(message);
            }
        }
        Some(group)
    }

    fn retain_rows(&mut self, rows: &HashSet<String>) {
        self.splits.retain(|id, group| {
            group
                .message_ids
                .iter()
                .any(|message| rows.contains(message) && self.parents.get(message) == Some(id))
        });
        self.parents
            .retain(|message, parent| rows.contains(message) && self.splits.contains_key(parent));
    }

    fn reusable(
        &mut self,
        source: &ConversationSummary,
        rows: &HashSet<String>,
    ) -> Option<&mut SplitConversation> {
        if !self.contains_members(&source.id, rows) {
            return None;
        }
        self.splits
            .get_mut(&source.id)
            .filter(|group| group.matches(source))
    }

    fn contains_members(&self, id: &str, rows: &HashSet<String>) -> bool {
        self.splits.get(id).is_some_and(|group| {
            group.message_ids.iter().all(|message| {
                rows.contains(message) && self.parents.get(message).map(String::as_str) == Some(id)
            })
        })
    }

    /// Replace the parent and any older copies of its members together. Fresh
    /// inspected metadata wins, including when a member was retained below a
    /// page boundary or already appeared in another response.
    fn apply_split(
        &mut self,
        rows: &mut Vec<ConversationSummary>,
        order: ListOrder,
        id: &str,
        mut incoming: Vec<ConversationSummary>,
    ) -> bool {
        deduplicate_rows(&mut incoming);
        if incoming.is_empty() {
            return false;
        }
        let source = rows
            .iter()
            .find(|row| row.id == id)
            .cloned()
            .or_else(|| self.splits.get(id).map(|group| group.source.clone()));
        let Some(source) = source else {
            return false;
        };
        let message_ids: Vec<_> = incoming.iter().map(|row| row.id.clone()).collect();
        let mut replaced: HashSet<&str> = message_ids.iter().map(String::as_str).collect();
        replaced.insert(id);
        if let Some(group) = self.splits.get(id) {
            replaced.extend(group.message_ids.iter().map(String::as_str));
        }
        let index = rows
            .iter()
            .position(|row| {
                row.id == id || self.parents.get(&row.id).is_some_and(|parent| parent == id)
            })
            .unwrap_or(rows.len());
        let insertion = rows[..index]
            .iter()
            .filter(|row| !replaced.contains(row.id.as_str()))
            .count();
        rows.retain(|row| !replaced.contains(row.id.as_str()));
        let _ = self.forget(id);
        for message in &message_ids {
            self.parents.insert(message.clone(), id.to_owned());
        }
        self.splits.insert(
            id.to_owned(),
            SplitConversation {
                source,
                message_ids,
            },
        );
        rows.splice(insertion..insertion, incoming);
        deduplicate_rows(rows);
        sort_by_date(rows, order);
        self.retain_rows(&collect_ids(rows));
        true
    }
}

/// List metadata that tells whether a loaded reader still describes its row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReaderStamp {
    kind: SummaryKind,
    time: Option<i64>,
    message_count: u32,
}

impl ReaderStamp {
    fn of(summary: &ConversationSummary) -> Self {
        Self {
            kind: summary.kind,
            time: summary.time,
            message_count: summary.message_count,
        }
    }
}

/// One memory-only reader, ordered least-recently used first in the cache.
struct CachedReader {
    stamp: ReaderStamp,
    reader: ConversationReader,
}

/// A folder's stable identity. Account folders are keyed by Proton's id, not
/// their display name, so a rename does not strand a valid cached listing.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum FolderKey {
    System(MailFolder),
    Custom(String),
}

impl FolderKey {
    fn of(folder: &Folder) -> Self {
        match folder {
            Folder::System(folder) => Self::System(*folder),
            Folder::Custom { id, .. } => Self::Custom(id.clone()),
        }
    }
}

/// A search sent to the backend, remembered so only its own answer is applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRequest {
    pub id: RequestId,
    pub query: String,
    pub page: u32,
    pub page_size: u32,
}

pub struct Mailbox {
    folder: Folder,
    /// Which of the folder's conversations the list shows. Only a listing of
    /// all of them, newest first, is kept when the folder, the filter or the
    /// order changes; rows read while it shows only unread ones stay until it
    /// is loaded again, and so do rows made unread among the read ones.
    filter: ListFilter,
    /// The order the list shows them in.
    order: ListOrder,
    status: ListStatus,
    conversations: Vec<ConversationSummary>,
    row_ids: HashSet<String>,
    /// Last dated row in display order, used to order the next page.
    last_dated_time: Option<i64>,
    /// Indices of visible rows into either `search.rows` or `conversations`.
    visible_cache: Vec<usize>,
    page_size: u32,
    next_page: u32,
    /// The page `load_more` asked for, which the next page continues from.
    more_page: u32,
    /// Rows of the loaded pages a move took off the screen, before Proton
    /// answered.
    moving_out: HashSet<String>,
    /// Rows Proton confirmed gone from the loaded pages since the last page
    /// arrived: Proton's later pages now start that many rows earlier.
    moved_out: HashSet<String>,
    has_more: bool,
    /// The folders left most recently, the one left longest ago first.
    cached_listings: Vec<CachedListing>,
    groups: ConversationGroups,
    /// Confirmed local mutations invalidate older inspection metadata.
    inspection_revision: u64,
    /// Whether the active listing must be revalidated before being considered
    /// a clean cache entry.
    active_dirty: bool,
    counts: Option<MailboxCounts>,
    counts_request: Option<RequestId>,
    reader: ReaderState,
    reader_stamp: Option<ReaderStamp>,
    cached_readers: Vec<CachedReader>,
    /// The conversation loading into the reader cache in the background,
    /// with its row's stamp when it was asked for: only its answer is kept.
    preloading: Option<(ReaderRequest, ReaderStamp)>,
    search_query: String,
    /// Results of the last server search, while they are on screen.
    search: Option<SearchState>,
    /// Why this device's index had nothing for the last search, if so.
    device_index: Option<DeviceIndex>,
    /// The search in flight, so a late answer to an abandoned one is ignored.
    search_request: Option<RequestId>,
    /// Enter was pressed while a folder page was loading. Searching then
    /// would make that page look stale, so the search waits for it.
    search_deferred: bool,
    /// The action on its way to Proton, or waiting for an automatic read of
    /// the same row.
    action_in_flight: Option<ActionRequest>,
    /// The actions sent in the same request as the one in flight, which Proton
    /// answers together with it; each settles in turn after it.
    sent_with: Vec<ActionRequest>,
    /// Actions asked for while another was on its way, in the order asked.
    /// Each already shows; they are sent one request at a time, so taking a
    /// move back always follows the move.
    waiting_actions: VecDeque<ActionRequest>,
    /// Actions started together with the one before them in the queue, as for
    /// rows chosen together, which go in the same request as it when they can.
    rides_with_previous: HashSet<RequestId>,
    /// The flag each action shown early replaced, by request, to put back if
    /// Proton refuses it.
    shown_flags: HashMap<RequestId, MailAction>,
    action_error: Option<MailboxError>,
    /// The last move, while it can still be taken back.
    undo: Option<UndoMove>,
    /// Automatic reads in flight, by row. Explicit read mutations on the same
    /// row wait here without blocking independent rows or other action kinds.
    automatic_reads: HashMap<String, RequestId>,
    /// Where rows moved without leaving the view now are. Starred and labels
    /// list mail wherever it sits, so a conversation moved to another folder
    /// stays in them, and only this says where it went.
    relocated: HashMap<String, Folder>,
    /// Rows a move back listed again before Proton confirmed it; a refused
    /// one takes its row away again.
    listed_again: HashSet<String>,
    /// Actions Proton confirmed since the folder's first page on its way was
    /// asked for. It may have read the page before applying them, so the
    /// page shows them again.
    confirmed_while_loading: Vec<ActionRequest>,
    /// The rows chosen together with Command or Shift, while there are two
    /// or more: the reader is closed then, and an action acts on all of them.
    selection: HashSet<String>,
    /// Where a Shift range starts: the row opened or Command-clicked last.
    selection_anchor: Option<String>,
    /// Where a Shift range ends, which Shift and an arrow move on.
    selection_end: Option<String>,
}

impl Mailbox {
    /// Opens `folder`, returning the first page to fetch. Counts are fetched
    /// under `counts_request`.
    pub fn open(
        folder: Folder,
        page_size: u32,
        page_request: RequestId,
        counts_request: RequestId,
    ) -> (Self, PageRequest) {
        let mailbox = Self {
            folder,
            filter: ListFilter::All,
            order: ListOrder::NewestFirst,
            status: ListStatus::Loading(page_request),
            conversations: Vec::new(),
            row_ids: HashSet::new(),
            last_dated_time: None,
            visible_cache: Vec::new(),
            page_size,
            next_page: 0,
            more_page: 0,
            moving_out: HashSet::new(),
            moved_out: HashSet::new(),
            has_more: false,
            cached_listings: Vec::new(),
            groups: ConversationGroups::default(),
            inspection_revision: 0,
            active_dirty: false,
            counts: None,
            counts_request: Some(counts_request),
            reader: ReaderState::Empty,
            reader_stamp: None,
            cached_readers: Vec::new(),
            preloading: None,
            search_query: String::new(),
            search: None,
            device_index: None,
            search_request: None,
            search_deferred: false,
            action_in_flight: None,
            sent_with: Vec::new(),
            waiting_actions: VecDeque::new(),
            rides_with_previous: HashSet::new(),
            shown_flags: HashMap::new(),
            action_error: None,
            undo: None,
            automatic_reads: HashMap::new(),
            relocated: HashMap::new(),
            listed_again: HashSet::new(),
            confirmed_while_loading: Vec::new(),
            selection: HashSet::new(),
            selection_anchor: None,
            selection_end: None,
        };
        let page = mailbox.page_request(page_request, 0);

        (mailbox, page)
    }

    pub fn folder(&self) -> &Folder {
        &self.folder
    }

    /// Which of the folder's conversations the list shows.
    pub fn filter(&self) -> ListFilter {
        self.filter
    }

    /// The order the list shows them in.
    pub fn order(&self) -> ListOrder {
        self.order
    }

    pub fn status(&self) -> ListStatus {
        self.status
    }

    /// The counts request in flight, if any.
    #[cfg(test)]
    pub fn counts_request(&self) -> Option<RequestId> {
        self.counts_request
    }

    #[cfg(test)]
    pub fn conversations(&self) -> &[ConversationSummary] {
        &self.conversations
    }

    /// Whether the index of row IDs holds the rows listed, each once, and
    /// nothing else.
    #[cfg(test)]
    pub fn indexes_its_rows(&self) -> bool {
        self.row_ids.len() == self.conversations.len()
            && self
                .conversations
                .iter()
                .all(|row| self.row_ids.contains(&row.id))
    }

    /// How many of the folders left behind stay loaded.
    #[cfg(test)]
    pub fn cached_folder_count(&self) -> usize {
        self.cached_listings.len()
    }

    fn source_rows(&self) -> &[ConversationSummary] {
        match &self.search {
            Some(search) => &search.rows,
            None => &self.conversations,
        }
    }

    /// Recomputes the cached visible row indices based on current search state and query.
    fn recompute_visible(&mut self) {
        self.visible_cache.clear();
        match &self.search {
            Some(search) => {
                self.visible_cache.extend(0..search.rows.len());
            }
            None => self.append_visible(0),
        }
        self.keep_chosen_rows_listed();
    }

    /// Keeps the rows chosen together that the list still shows; one left on
    /// its own is no longer a choice of several.
    fn keep_chosen_rows_listed(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        let listed: HashSet<String> = self
            .visible_conversations()
            .filter(|row| self.selection.contains(&row.id))
            .map(|row| row.id.clone())
            .collect();
        self.selection = if listed.len() < 2 {
            HashSet::new()
        } else {
            listed
        };
    }

    /// Extends visible indices when a page was appended without moving old rows.
    fn append_visible(&mut self, from: usize) {
        let query = self.search_query.trim();
        if query.is_empty() {
            self.visible_cache.extend(from..self.conversations.len());
        } else {
            let query = query.to_lowercase();
            self.visible_cache.extend(
                self.conversations[from..]
                    .iter()
                    .enumerate()
                    .filter_map(|(offset, row)| {
                        matches_search(row, &query).then_some(from + offset)
                    }),
            );
        }
    }

    /// Number of visible rows currently on screen.
    pub fn visible_count(&self) -> usize {
        self.visible_cache.len()
    }

    /// Whether there are no visible rows on screen.
    pub fn is_empty_visible(&self) -> bool {
        self.visible_cache.is_empty()
    }

    /// Accesses a visible row by its 0-based visible index.
    pub fn visible_row(&self, index: usize) -> Option<&ConversationSummary> {
        let &row_index = self.visible_cache.get(index)?;
        self.source_rows().get(row_index)
    }

    /// The rows on screen: search results when a search is showing, otherwise
    /// the loaded folder narrowed by what is typed in the search field.
    pub fn visible_conversations(
        &self,
    ) -> impl ExactSizeIterator<Item = &ConversationSummary> + DoubleEndedIterator {
        let rows = self.source_rows();
        self.visible_cache.iter().map(move |&idx| &rows[idx])
    }

    /// The query whose results are on screen, if any.
    pub fn shown_search_query(&self) -> Option<&str> {
        self.search.as_ref().map(|search| search.query.as_str())
    }

    pub fn search_query(&self) -> &str {
        &self.search_query
    }

    pub fn has_search_text(&self) -> bool {
        !self.search_query.trim().is_empty()
    }

    /// Starts a global backend search, or clears empty input.
    pub fn start_search(&mut self, request: RequestId) -> Option<SearchRequest> {
        let query = self.search_query.trim().to_owned();
        if query.is_empty() {
            self.clear_search();
            return None;
        }
        if self.search_request.is_none() && self.is_busy() {
            self.search_deferred = true;
            return None;
        }

        self.search_deferred = false;
        self.search_request = Some(request);
        self.device_index = None;
        self.status = ListStatus::Loading(request);
        // Only actions confirmed from now on can be missing from its answer.
        self.confirmed_while_loading.clear();

        Some(SearchRequest {
            id: request,
            query,
            page: 0,
            page_size: self.page_size,
        })
    }

    /// Whether a search waited for the folder page that has now settled.
    pub fn take_deferred_search(&mut self) -> bool {
        !self.is_busy() && std::mem::take(&mut self.search_deferred)
    }

    pub fn load_more_search(&mut self, request: RequestId) -> Option<SearchRequest> {
        if self.is_busy() {
            return None;
        }
        let search = self.search.as_ref().filter(|search| search.has_more)?;
        let next = SearchRequest {
            id: request,
            query: search.query.clone(),
            page: search.next_page,
            page_size: self.page_size,
        };
        self.search_request = Some(request);
        self.status = ListStatus::LoadingMore(request);
        Some(next)
    }

    /// Applies the answer to the search still in flight; anything else is a
    /// leftover from a query the user has moved on from.
    pub fn finish_search(
        &mut self,
        request: &SearchRequest,
        result: Result<ConversationPage, MailboxError>,
    ) -> Option<MailboxError> {
        if self.search_request != Some(request.id) {
            return None;
        }
        self.search_request = None;

        match result {
            Ok(page) => {
                let has_more = has_more_pages(
                    page.conversations.len(),
                    u64::from(request.page) + 1,
                    request.page_size,
                    page.total,
                );
                if request.page > 0 {
                    let search = self
                        .search
                        .as_mut()
                        .expect("later search page has first page");
                    let new: Vec<_> = page
                        .conversations
                        .into_iter()
                        .filter(|row| !search.row_ids.contains(&row.id))
                        .collect();
                    let from = search.rows.len();
                    search.row_ids.extend(new.iter().map(|row| row.id.clone()));
                    search.rows.extend(new);
                    search.next_page = request.page.saturating_add(1);
                    search.has_more = has_more;
                    self.visible_cache.extend(from..search.rows.len());
                } else {
                    let row_ids = collect_ids(&page.conversations);
                    self.search = Some(SearchState {
                        query: request.query.clone(),
                        rows: page.conversations,
                        row_ids,
                        next_page: 1,
                        has_more,
                        on_device: false,
                    });
                    self.recompute_visible();
                }
                // Proton may have read the results before applying these, so
                // what they showed stays: a row moved out leaves them again.
                for request in self.unseen_actions() {
                    self.record_action(&request.row_id, &request.action);
                }
                if request.page == 0 {
                    self.close_reader_if_hidden();
                }
                self.status = ListStatus::Loaded;
                None
            }
            Err(error) => {
                let attempt = if request.page > 0 {
                    ListAttempt::More
                } else {
                    ListAttempt::Search
                };
                self.status = ListStatus::Failed(error, attempt);
                Some(error)
            }
        }
    }

    /// Applies what this device's index found for a search Proton could not
    /// answer, or, when it found nothing to look in, the failure that
    /// Proton's silence was, saying why.
    pub fn finish_device_search(
        &mut self,
        request: &SearchRequest,
        found: Result<Option<ConversationPage>, MailboxError>,
    ) -> Option<MailboxError> {
        if self.search_request != Some(request.id) {
            return None;
        }
        let page = match found {
            Ok(Some(page)) => page,
            Ok(None) => return self.no_device_results(request, DeviceIndex::Missing),
            Err(_) => return self.no_device_results(request, DeviceIndex::Unreadable),
        };
        let error = self.finish_search(request, Ok(page));
        if let Some(search) = &mut self.search {
            search.on_device = true;
        }
        error
    }

    fn no_device_results(
        &mut self,
        request: &SearchRequest,
        why: DeviceIndex,
    ) -> Option<MailboxError> {
        self.device_index = Some(why);
        self.finish_search(request, Err(MailboxError::Connection))
    }

    /// Whether the results shown came from this device's index.
    pub fn shown_search_is_on_device(&self) -> bool {
        self.search.as_ref().is_some_and(|search| search.on_device)
    }

    /// Why this device's index had nothing to show for the last search,
    /// if it was looked in for nothing.
    pub fn device_index(&self) -> Option<DeviceIndex> {
        self.device_index
    }

    /// Goes back to the folder listing, leaving any results behind.
    pub fn clear_search(&mut self) {
        let leaving_results = self.search.is_some();
        self.search = None;
        self.device_index = None;
        self.search_deferred = false;
        if self.search_request.is_some() {
            self.status = ListStatus::Loaded;
        }
        self.search_request = None;
        // Only the search's own failure goes with it: a folder that failed to
        // load still says so, with its way to try again.
        let search_failed = match self.status {
            ListStatus::Failed(_, ListAttempt::Search) => true,
            ListStatus::Failed(_, ListAttempt::More) => leaving_results,
            _ => false,
        };
        if search_failed {
            self.status = ListStatus::Loaded;
        }
        self.recompute_visible();
        // Leaving results is a new list; only emptying what was typed keeps
        // the reader, as typing does.
        if leaving_results {
            self.close_reader_if_hidden();
        }
    }

    /// Closes the reader when what it holds is no longer on screen.
    fn close_reader_if_hidden(&mut self) {
        let hidden = self.selected_conversation().is_some_and(|selected| {
            !self
                .visible_conversations()
                .any(|conversation| conversation.id == selected)
        });
        if hidden {
            self.set_reader(ReaderState::Empty);
        }
    }

    pub fn set_search_query(&mut self, query: String) {
        self.search_query = query;
        // A waiting search was asked for the text before this change.
        self.search_deferred = false;
        // Emptying the field is how someone leaves the results behind.
        if self.search_query.trim().is_empty() {
            self.clear_search();
        } else {
            // What is typed narrows the list but leaves the open conversation
            // open, as a refresh does, so deleting a letter brings its row back.
            self.recompute_visible();
        }
    }

    pub fn counts(&self) -> Option<&MailboxCounts> {
        self.counts.as_ref()
    }

    /// Whether the visible search or folder listing has another page.
    pub fn has_more(&self) -> bool {
        self.search
            .as_ref()
            .map_or(self.has_more, |search| search.has_more)
    }

    /// Whether the folder's loaded pages list `row_id`, whatever a search
    /// shows meanwhile.
    pub fn lists(&self, row_id: &str) -> bool {
        self.row_ids.contains(row_id)
    }

    /// Whether the rows on screen hold this one, hidden by typing or not.
    pub fn has_row(&self, row_id: &str) -> bool {
        match &self.search {
            Some(search) => search.row_ids.contains(row_id),
            None => self.row_ids.contains(row_id),
        }
    }

    pub(super) fn has_inspection_target(&self, id: &str) -> bool {
        self.row_ids.contains(id) || self.groups.splits.contains_key(id)
    }

    /// The conversations listed as the messages they were split into, by ID.
    #[cfg(test)]
    pub fn split_parents(&self) -> Vec<&str> {
        let mut parents: Vec<&str> = self.groups.splits.keys().map(String::as_str).collect();
        parents.sort_unstable();
        parents
    }

    /// Each message listed for a split conversation, with that conversation,
    /// by the message's ID.
    #[cfg(test)]
    pub fn split_members(&self) -> Vec<(&str, &str)> {
        let mut members: Vec<(&str, &str)> = self
            .groups
            .parents
            .iter()
            .map(|(message, parent)| (message.as_str(), parent.as_str()))
            .collect();
        members.sort_unstable();
        members
    }

    /// How the split conversation `id` was listed last, which its messages
    /// stand for.
    #[cfg(test)]
    pub fn split_source(&self, id: &str) -> Option<&ConversationSummary> {
        self.groups.splits.get(id).map(|group| &group.source)
    }

    pub(super) fn inspection_revision(&self) -> u64 {
        self.inspection_revision
    }

    /// Whether asking Proton about `id` could change how it is listed: it is
    /// here, and not as it was when an inspection last left it as one.
    pub(super) fn worth_inspecting(&self, id: &str) -> bool {
        self.has_inspection_target(id)
            && self
                .groups
                .kept_whole
                .get(id)
                .is_none_or(|then| self.row(id) != Some(then))
    }

    /// Remembers that an inspection left `id` as the one conversation it is
    /// listed as now, and lets go of those no longer listed.
    pub(super) fn keep_whole(&mut self, id: &str) {
        let Some(row) = self.row(id).cloned() else {
            return;
        };
        let listed = &self.row_ids;
        self.groups
            .kept_whole
            .retain(|kept, _| listed.contains(kept));
        self.groups.kept_whole.insert(id.to_owned(), row);
    }

    pub fn row_conversation<'a>(&'a self, row: &'a ConversationSummary) -> Option<&'a str> {
        match row.kind {
            SummaryKind::Conversation => Some(&row.id),
            SummaryKind::Message => self.groups.parents.get(&row.id).map(String::as_str),
        }
    }

    /// Prefer the exact message; a grouped parent or its first visible member
    /// represents the same conversation while inspection updates presentation.
    pub fn visible_anchor_index(&self, id: &str, conversation: Option<&str>) -> Option<usize> {
        let mut representative = None;
        for (index, row) in self.visible_conversations().enumerate() {
            if row.id == id {
                return Some(index);
            }
            if representative.is_none()
                && conversation.is_some()
                && self.row_conversation(row) == conversation
            {
                representative = Some(index);
            }
        }
        representative
    }

    /// How many conversations are loaded, which is all a search can look at.
    pub fn loaded_count(&self) -> usize {
        self.conversations.len()
    }

    /// The visible row one step away from the selected one, or the first
    /// visible row when nothing is open yet. `None` at either end of the list.
    pub fn neighbour(&self, step: Step) -> Option<String> {
        // After several rows are chosen, the keys go on from the far end.
        let from = self.selected_conversation().or_else(|| {
            self.selection_end
                .as_deref()
                .filter(|_| !self.selection.is_empty())
        });
        let current =
            from.and_then(|from| self.visible_conversations().position(|row| row.id == from));

        let index = match (current, step) {
            (None, _) => 0,
            (Some(index), Step::Next) => index + 1,
            (Some(0), Step::Previous) => return None,
            (Some(index), Step::Previous) => index - 1,
        };

        self.visible_row(index).map(|row| row.id.clone())
    }

    /// Closes the reader without touching the list, as Escape does.
    pub fn close_reader(&mut self) {
        self.set_reader(ReaderState::Empty);
    }

    /// Whether a row is chosen: the open one, or one of several selected.
    pub fn is_chosen(&self, row_id: &str) -> bool {
        self.selection.contains(row_id) || self.selected_conversation() == Some(row_id)
    }

    /// Whether a row is one of several chosen together.
    pub fn selected_together(&self, row_id: &str) -> bool {
        self.selection.contains(row_id)
    }

    /// Where every row chosen together is known to be, when it is the same
    /// place, so none is offered a move into the folder it is already in.
    pub fn selection_location(&self) -> Option<&Folder> {
        let mut places = self.selection.iter().map(|row_id| self.location_of(row_id));
        let first = places.next()??;
        places.all(|place| place == Some(first)).then_some(first)
    }

    /// The rows chosen together, in the order of the list; none unless two or
    /// more are.
    pub fn selected_rows(&self) -> Vec<&ConversationSummary> {
        if self.selection.is_empty() {
            return Vec::new();
        }
        self.visible_conversations()
            .filter(|row| self.selection.contains(&row.id))
            .collect()
    }

    /// The rows an action acts on: those chosen together, in the order of the
    /// list, or else the open one.
    pub fn rows_acted_on(&self) -> Vec<String> {
        let chosen = self.selected_rows();
        if chosen.is_empty() {
            return self
                .selected_summary()
                .map(|row| row.id.clone())
                .into_iter()
                .collect();
        }
        chosen.into_iter().map(|row| row.id.clone()).collect()
    }

    /// Adds a row to those chosen, the open one counted among them, or takes
    /// it out, as a Command click does. Returns the row left alone, for the
    /// app to open.
    pub fn toggle_selected(&mut self, row_id: &str) -> Option<String> {
        let mut chosen = self.chosen_ids();
        if !chosen.remove(row_id) {
            chosen.insert(row_id.to_owned());
        }
        self.selection_anchor = Some(row_id.to_owned());
        self.selection_end = Some(row_id.to_owned());
        self.choose(chosen)
    }

    /// Chooses every row from where the range starts through `row_id`, as a
    /// Shift click does. Returns the row left alone, for the app to open.
    pub fn select_through(&mut self, row_id: &str) -> Option<String> {
        let anchor = self
            .selection_anchor
            .clone()
            .filter(|anchor| self.visible_index(anchor).is_some())
            .unwrap_or_else(|| row_id.to_owned());
        let range = match (self.visible_index(&anchor), self.visible_index(row_id)) {
            (Some(from), Some(to)) => self
                .visible_conversations()
                .skip(from.min(to))
                .take(from.abs_diff(to) + 1)
                .map(|row| row.id.clone())
                .collect(),
            _ => HashSet::from([row_id.to_owned()]),
        };
        self.selection_anchor = Some(anchor);
        self.selection_end = Some(row_id.to_owned());
        self.choose(range)
    }

    /// Moves the far end of the rows chosen one row on, as Shift and an arrow
    /// do, stopping at the ends of the list. With nothing chosen it opens the
    /// first row, as a plain arrow would. Returns the row left alone, for the
    /// app to open.
    pub fn extend_selection(&mut self, step: Step) -> Option<String> {
        if self.chosen_ids().is_empty() {
            return self.neighbour(step);
        }
        let end = self.visible_index(self.selection_end.as_deref()?)?;
        let next = match step {
            Step::Next => end + 1,
            Step::Previous => end.checked_sub(1)?,
        };
        let next = self.visible_row(next)?.id.clone();
        self.select_through(&next)
    }

    /// Chooses every row listed, as Command and A do. Returns the row left
    /// alone, for the app to open.
    pub fn select_all(&mut self) -> Option<String> {
        let all: Vec<String> = self
            .visible_conversations()
            .map(|row| row.id.clone())
            .collect();
        if all.is_empty() {
            return None;
        }
        self.selection_anchor = all.first().cloned();
        self.selection_end = all.last().cloned();
        self.choose(all.into_iter().collect())
    }

    /// The far end of the rows chosen together, which Shift and an arrow
    /// move on.
    pub fn selection_end(&self) -> Option<&str> {
        self.selection_end
            .as_deref()
            .filter(|_| !self.selection.is_empty())
    }

    /// Lets go of the rows chosen together, as Escape does, leaving the reader
    /// closed. Whether there were any.
    pub fn clear_selection(&mut self) -> bool {
        !std::mem::take(&mut self.selection).is_empty()
    }

    /// The rows chosen now: those selected together, or the open one.
    fn chosen_ids(&self) -> HashSet<String> {
        if !self.selection.is_empty() {
            return self.selection.clone();
        }
        self.selected_conversation()
            .map(str::to_owned)
            .into_iter()
            .collect()
    }

    /// Settles what is chosen: two rows or more are selected together and
    /// close the reader; a single one is returned for the app to open; none
    /// closes the reader.
    fn choose(&mut self, chosen: HashSet<String>) -> Option<String> {
        if chosen.len() >= 2 {
            self.selection = chosen;
            self.close_reader();
            return None;
        }
        self.selection.clear();
        let only = chosen.into_iter().next();
        if only.is_none() {
            self.close_reader();
        }
        only
    }

    fn visible_index(&self, row_id: &str) -> Option<usize> {
        self.visible_conversations()
            .position(|row| row.id == row_id)
    }

    /// Where a row sits among the visible ones, as a fraction from 0 to 1.
    /// `None` when it is not visible or is the only row.
    pub fn visible_position(&self, row_id: &str) -> Option<f32> {
        let last = self.visible_cache.len().checked_sub(1)?;
        if last == 0 {
            return None;
        }
        let index = self
            .visible_conversations()
            .position(|row| row.id == row_id)?;

        Some(index as f32 / last as f32)
    }

    pub fn selected_conversation(&self) -> Option<&str> {
        self.reader.conversation_id()
    }

    pub fn selected_summary(&self) -> Option<&ConversationSummary> {
        let selected = self.selected_conversation()?;

        self.row(selected)
    }

    /// Every actionable row, including server search results.
    fn rows(&self) -> impl Iterator<Item = &ConversationSummary> {
        self.conversations
            .iter()
            .chain(self.search.iter().flat_map(|search| search.rows.iter()))
    }

    pub(super) fn row(&self, row_id: &str) -> Option<&ConversationSummary> {
        self.rows().find(|row| row.id == row_id)
    }

    /// Every listing that holds one row. A search result can also be in the
    /// open folder, and the two copies must never disagree about it.
    fn row_copies<'a>(
        &'a mut self,
        row_id: &'a str,
    ) -> impl Iterator<Item = &'a mut ConversationSummary> {
        let found = self
            .search
            .iter_mut()
            .flat_map(|search| search.rows.iter_mut());

        self.conversations
            .iter_mut()
            .chain(found)
            .filter(move |row| row.id == row_id)
    }

    /// Whether the folder listing itself holds this row, which is what says
    /// Proton can be told to read it in the open folder.
    fn listed_in_folder(&self, row_id: &str) -> bool {
        self.row_ids.contains(row_id)
    }

    /// The folder a row is known to be in: the open folder, when mail lives
    /// there and lists the row, or where this view saw it moved. Otherwise a
    /// row shown under a label, Starred, Sent or Drafts, or one only a search
    /// found, could be anywhere.
    pub fn location_of(&self, row_id: &str) -> Option<&Folder> {
        if let Some(folder) = self.relocated.get(row_id) {
            return Some(folder);
        }
        (self.folder.is_location() && self.listed_in_folder(row_id)).then_some(&self.folder)
    }

    /// Where the selected row is known to be, so it is not offered a move
    /// into the folder it is already in.
    pub fn selected_location(&self) -> Option<&Folder> {
        self.location_of(self.selected_conversation()?)
    }

    #[cfg(test)]
    pub fn reader(&self) -> Option<&ConversationReader> {
        match &self.reader {
            ReaderState::Loaded(reader) => Some(reader),
            _ => None,
        }
    }

    /// Whether the open reader goes into the reader cache when another takes
    /// its place: only while it is known to show its row as listed.
    #[cfg(test)]
    pub fn reader_cacheable(&self) -> bool {
        self.reader_stamp.is_some()
    }

    /// The conversations the reader cache holds, the one used longest ago
    /// first.
    #[cfg(test)]
    pub fn cached_reader_ids(&self) -> Vec<&str> {
        self.cached_readers
            .iter()
            .map(|cached| cached.reader.conversation_id())
            .collect()
    }

    pub fn reader_state(&self) -> &ReaderState {
        &self.reader
    }

    /// The conversation loading into the reader cache, if any.
    #[cfg(test)]
    pub fn preloading(&self) -> Option<&ReaderRequest> {
        self.preloading.as_ref().map(|(request, _)| request)
    }

    #[cfg(test)]
    pub fn action_pending(&self) -> bool {
        self.action_in_flight.is_some()
    }

    pub fn action_error(&self) -> Option<MailboxError> {
        self.action_error
    }

    pub fn is_busy(&self) -> bool {
        self.status.request_in_flight().is_some()
    }

    /// Whether an action or a move back can start now. A refresh leaves the
    /// rows on screen, so they take actions while it runs. Nothing does while
    /// the folder first loads, a search's answer is on its way, or Load more
    /// runs: it asks Proton by position, which a move meanwhile would shift.
    pub fn takes_actions(&self) -> bool {
        !matches!(
            self.status,
            ListStatus::Loading(_) | ListStatus::LoadingMore(_)
        )
    }

    pub(super) fn auto_refresh_available(&self) -> bool {
        !self.is_busy() && self.action_in_flight.is_none() && self.automatic_reads.is_empty()
    }

    /// Changes shown here that Proton has not confirmed yet: the action on its
    /// way, those waiting behind it, and automatic reads.
    pub fn unsent_changes(&self) -> usize {
        usize::from(self.action_in_flight.is_some())
            + self.sent_with.len()
            + self.waiting_actions.len()
            + self.automatic_reads.len()
    }

    pub(super) fn needs_revalidation(&self) -> bool {
        self.active_dirty && !self.is_busy()
    }

    /// Loads a visible conversation; reselecting a failed one retries it.
    pub fn start_conversation_load(
        &mut self,
        conversation_id: String,
        request: RequestId,
    ) -> Option<ReaderRequest> {
        let reselecting = self.selected_conversation() == Some(conversation_id.as_str());
        if reselecting && !matches!(self.reader, ReaderState::Failed { .. }) {
            return None;
        }
        let summary = self
            .visible_conversations()
            .find(|conversation| conversation.id == conversation_id)?;
        let stamp = ReaderStamp::of(summary);
        self.action_error = None;
        // Opening a row ends a choice of several, and a Shift range starts here.
        self.selection.clear();
        self.selection_anchor = Some(conversation_id.clone());
        self.selection_end = Some(conversation_id.clone());

        if let Some(reader) = self.take_cached_reader(&conversation_id, stamp) {
            self.set_reader(ReaderState::Loaded(reader));
            self.reader_stamp = Some(stamp);
            return None;
        }

        Some(self.open_conversation(conversation_id, stamp.kind, stamp.message_count, request))
    }

    pub fn retry_conversation(&mut self, request: RequestId) -> Option<ReaderRequest> {
        let ReaderState::Failed {
            conversation_id, ..
        } = &self.reader
        else {
            return None;
        };
        let conversation_id = conversation_id.clone();
        // A retry reaches the row even when typing is hiding it, which is why
        // it looks through every loaded row rather than the visible ones.
        let row = self.row(&conversation_id)?;
        let (kind, messages) = (row.kind, row.message_count);

        Some(self.open_conversation(conversation_id, kind, messages, request))
    }

    /// Sets the reader's conversation and active request.
    fn open_conversation(
        &mut self,
        conversation_id: String,
        kind: SummaryKind,
        messages: u32,
        request: RequestId,
    ) -> ReaderRequest {
        let request = ReaderRequest {
            id: request,
            conversation_id,
            kind,
            messages,
        };
        self.set_reader(ReaderState::Loading {
            conversation_id: request.conversation_id.clone(),
            request: request.id,
        });

        request
    }

    /// Applies only the response for the current reader request. A mismatched
    /// detail ID is rejected to preserve the reader-selection invariant.
    pub fn finish_conversation(
        &mut self,
        request: &ReaderRequest,
        result: Result<ConversationDetail, MailboxError>,
    ) -> Option<MailboxError> {
        if !self.awaits(request) {
            return None;
        }

        match result {
            Ok(detail) if detail.id == request.conversation_id => {
                let stamp = self
                    .visible_conversations()
                    .find(|summary| summary.id == request.conversation_id)
                    .map(ReaderStamp::of);
                self.set_reader(ReaderState::Loaded(ConversationReader::new(detail)));
                self.reader_stamp = stamp;
                None
            }
            Ok(_) => {
                let error = MailboxError::Unavailable;
                self.set_reader(ReaderState::Failed {
                    conversation_id: request.conversation_id.clone(),
                    error,
                });
                Some(error)
            }
            Err(error) => {
                self.set_reader(ReaderState::Failed {
                    conversation_id: request.conversation_id.clone(),
                    error,
                });
                Some(error)
            }
        }
    }

    /// Whether the reader is still waiting for `request`'s answer.
    pub fn awaits(&self, request: &ReaderRequest) -> bool {
        matches!(
            &self.reader,
            ReaderState::Loading {
                conversation_id,
                request: current,
            } if conversation_id == &request.conversation_id && current == &request.id
        )
    }

    fn set_reader(&mut self, reader: ReaderState) {
        let previous = std::mem::replace(&mut self.reader, reader);
        let stamp = self.reader_stamp.take();
        if let (ReaderState::Loaded(previous_reader), Some(stamp)) = (previous, stamp) {
            self.cache_reader(previous_reader, stamp);
        }
    }

    fn cache_reader(&mut self, reader: ConversationReader, stamp: ReaderStamp) {
        if let Some(index) = self
            .cached_readers
            .iter()
            .position(|cached| cached.reader.conversation_id() == reader.conversation_id())
        {
            self.cached_readers.remove(index);
        }
        if self.cached_readers.len() >= READER_CACHE_CAPACITY {
            self.cached_readers.remove(0);
        }
        self.cached_readers.push(CachedReader { stamp, reader });
    }

    fn take_cached_reader(
        &mut self,
        conversation_id: &str,
        stamp: ReaderStamp,
    ) -> Option<ConversationReader> {
        let index = self
            .cached_readers
            .iter()
            .position(|cached| cached.reader.conversation_id() == conversation_id)?;
        let cached = self.cached_readers.remove(index);

        (cached.stamp == stamp).then_some(cached.reader)
    }

    /// Starts loading into the reader cache the row that would take the open
    /// conversation's place were it moved out of the view, as
    /// `open_row_after` picks it: the one below, or the one above at the end
    /// of the list. Only once the open one shows, and not when that row is
    /// cached already or on its way.
    pub fn start_preload(&mut self, request: RequestId) -> Option<ReaderRequest> {
        if !matches!(self.reader, ReaderState::Loaded(_)) {
            return None;
        }
        let selected = self.selected_conversation()?;
        let index = self
            .visible_conversations()
            .position(|row| row.id == selected)?;
        let next = self
            .visible_row(index + 1)
            .or_else(|| self.visible_row(index.checked_sub(1)?))?;
        let stamp = ReaderStamp::of(next);
        let wanted = |id: &str, at: ReaderStamp| id == next.id && at == stamp;
        let cached = self
            .cached_readers
            .iter()
            .any(|cached| wanted(cached.reader.conversation_id(), cached.stamp));
        let on_its_way = self
            .preloading
            .as_ref()
            .is_some_and(|(request, at)| wanted(&request.conversation_id, *at));
        if cached || on_its_way {
            return None;
        }
        let request = ReaderRequest {
            id: request,
            conversation_id: next.id.clone(),
            kind: next.kind,
            messages: next.message_count,
        };
        self.preloading = Some((request.clone(), stamp));
        Some(request)
    }

    /// Lets go of the preload on its way, which was stopped: its answer is
    /// not kept.
    pub fn forget_preload(&mut self) {
        self.preloading = None;
    }

    /// Keeps a preloaded conversation in the reader cache, if it is still
    /// the one wanted, as the row was when it was asked for: a row changed
    /// since loads afresh when opened. Nothing shows it or marks it read.
    pub fn finish_preload(
        &mut self,
        request: &ReaderRequest,
        result: Result<ConversationDetail, MailboxError>,
    ) -> Option<MailboxError> {
        if self.preloading.as_ref().map(|(wanted, _)| wanted) != Some(request) {
            return None;
        }
        let (_, stamp) = self.preloading.take()?;
        match result {
            // The reader may have opened it meanwhile, from a load of its own.
            Ok(detail)
                if detail.id == request.conversation_id
                    && self.selected_conversation() != Some(detail.id.as_str()) =>
            {
                self.cache_reader(ConversationReader::new(detail), stamp);
                None
            }
            Ok(_) => None,
            Err(error) => Some(error),
        }
    }

    /// A successful send can add a message to a previously loaded thread.
    pub(super) fn invalidate_reader_cache(&mut self) {
        self.cached_readers.clear();
        self.preloading = None;
        // Keep the open reader visible, but reload it after navigating away.
        self.reader_stamp = None;
    }

    /// Clears content that is known to have left the current listing.
    fn discard_reader(&mut self) {
        self.reader = ReaderState::Empty;
        self.reader_stamp = None;
    }

    pub fn toggle_message(&mut self, message_id: &str) {
        if let ReaderState::Loaded(reader) = &mut self.reader {
            reader.toggle(message_id);
        }
    }

    pub fn toggle_quote(&mut self, message_id: &str, index: usize) {
        if let ReaderState::Loaded(reader) = &mut self.reader {
            reader.toggle_quote(message_id, index);
        }
    }

    /// Starts marking the opened row read once its content is shown. Only an
    /// unread row with no read already in flight qualifies.
    pub fn start_mark_read(&mut self, request: RequestId) -> Option<ActionRequest> {
        let ReaderState::Loaded(reader) = &self.reader else {
            return None;
        };
        let row_id = reader.conversation_id().to_owned();
        // A thread split into its messages while it was open has no row of
        // its own any more: it is read as the thread it was, unread while
        // any of its messages is, and in the folder that lists them.
        let (unread, kind, listed) = match self.row(&row_id) {
            Some(row) => (row.unread, row.kind, self.listed_in_folder(&row_id)),
            None => {
                let group = self.groups.splits.get(&row_id)?;
                let members = || group.message_ids.iter();
                (
                    members().any(|message| self.row(message).is_some_and(|row| row.unread)),
                    group.source.kind,
                    members().any(|message| self.listed_in_folder(message)),
                )
            }
        };
        if !unread || self.automatic_reads.contains_key(&row_id) || self.read_change_queued(&row_id)
        {
            return None;
        }

        let request = ActionRequest {
            id: request,
            context: listed.then(|| self.folder.clone()),
            row_id,
            kind,
            folder: self.folder.clone(),
            action: MailAction::SetUnread(false),
        };
        self.automatic_reads
            .insert(request.row_id.clone(), request.id);
        Some(request)
    }

    /// An action on `row_id` started from the open folder, read by Proton in
    /// that folder while the row is listed there.
    fn new_action_request(
        &self,
        id: RequestId,
        row_id: String,
        kind: SummaryKind,
        action: MailAction,
    ) -> ActionRequest {
        ActionRequest {
            id,
            context: self.listed_in_folder(&row_id).then(|| self.folder.clone()),
            row_id,
            kind,
            folder: self.folder.clone(),
            action,
        }
    }

    /// The action on its way to Proton, then those waiting behind it, in
    /// the order they go.
    fn queued_actions(&self) -> impl Iterator<Item = &ActionRequest> {
        self.action_in_flight
            .iter()
            .chain(&self.sent_with)
            .chain(&self.waiting_actions)
    }

    /// The actions that go to Proton in one request with `head`, head first:
    /// those started together with it, when it is the one in flight.
    pub fn request_group(&self, head: &ActionRequest) -> Vec<ActionRequest> {
        let riders = if self.action_in_flight.as_ref() == Some(head) {
            self.sent_with.as_slice()
        } else {
            &[]
        };
        std::iter::once(head).chain(riders).cloned().collect()
    }

    /// Takes into the request in flight the actions started together with it
    /// and waiting right behind it: the same action on rows of the same kind,
    /// read in the same folder, that can go now, up to a request's limit.
    fn join_in_flight(&mut self) {
        while self.sent_with.len() + 1 < ACTION_GROUP_LIMIT {
            let (Some(head), Some(next)) = (&self.action_in_flight, self.waiting_actions.front())
            else {
                return;
            };
            let joins = self.rides_with_previous.contains(&next.id)
                && (next.kind, &next.action, &next.context, &next.folder)
                    == (head.kind, &head.action, &head.context, &head.folder)
                && self.ready_to_send(next)
                && !std::iter::once(head)
                    .chain(&self.sent_with)
                    .any(|sent| sent.row_id == next.row_id);
            if !joins {
                return;
            }
            let next = self
                .waiting_actions
                .pop_front()
                .expect("the front was just read");
            self.rides_with_previous.remove(&next.id);
            self.sent_with.push(next);
        }
    }

    /// Notes that Proton read `row_id`, or made it unread again. Read, it
    /// leaves the pages Proton lists of unread conversations, though it stays
    /// here until the list is loaded again; unread, it comes back to them.
    /// Among the read ones it is the other way round.
    fn read_changed(&mut self, row_id: &str, unread: bool) {
        let stays = match self.filter {
            ListFilter::Unread => unread,
            ListFilter::Read => !unread,
            ListFilter::All | ListFilter::WithFiles => return,
        };
        // Only a row of the loaded pages moves the pages after them.
        let loaded = self.row_ids.contains(row_id) || self.groups.splits.contains_key(row_id);
        if !loaded {
            return;
        }
        if stays {
            self.moved_out.remove(row_id);
        } else {
            self.moved_out.insert(row_id.to_owned());
        }
    }

    /// Whether an explicit read or unread of `row_id` is on its way or waits.
    fn read_change_queued(&self, row_id: &str) -> bool {
        self.queued_actions()
            .any(|action| action.row_id == row_id && action.action.changes_read())
    }

    /// Applies a confirmed automatic read and releases a queued explicit read
    /// mutation only after that confirmation. Stale responses release nothing.
    /// A failed predecessor also fails its queued action: a timed-out request
    /// might still be applied remotely, so it is unsafe to send its successor.
    /// The action waiting behind that one is still sent.
    pub fn finish_mark_read(
        &mut self,
        request: &ActionRequest,
        result: Result<(), MailboxError>,
    ) -> ReadCompletion {
        if self.automatic_reads.get(&request.row_id) != Some(&request.id) {
            return ReadCompletion::default();
        }
        self.automatic_reads.remove(&request.row_id);
        // Only the head of the queue can be waiting for this read: anything
        // behind it waits for the head first.
        let waiting = self
            .action_in_flight
            .as_ref()
            .filter(|action| action.row_id == request.row_id && action.action.changes_read())
            .cloned();
        if let Err(error) = result {
            let next_action =
                waiting.and_then(|action| self.finish_action(&action, Err(error)).next_action);
            return ReadCompletion {
                read_confirmed: false,
                next_action,
                error: Some(error),
            };
        }
        self.invalidate_listings();
        // A page on its way may have been read before it, as for any action.
        if self.shows(&request.folder)
            && matches!(
                self.status,
                ListStatus::Loading(_) | ListStatus::Refreshing(_)
            )
        {
            self.confirmed_while_loading.push(request.clone());
        }
        // The row is only marked here while its folder is open, but Proton's
        // counts changed wherever the person went meanwhile. A thread split
        // into its messages is read in them.
        if self.shows(&request.folder) {
            self.read_changed(&request.row_id, false);
            let members = self
                .groups
                .splits
                .get(&request.row_id)
                .map(|group| group.message_ids.clone())
                .unwrap_or_default();
            for row_id in std::iter::once(&request.row_id).chain(&members) {
                for row in self.row_copies(row_id) {
                    row.unread = false;
                }
            }
        }

        if waiting.is_some() {
            self.join_in_flight();
        }
        ReadCompletion {
            read_confirmed: true,
            next_action: waiting,
            error: None,
        }
    }

    /// The move the user can still take back, if any.
    pub fn undo(&self) -> Option<&UndoMove> {
        self.undo.as_ref()
    }

    /// Replaces the undo offer with one for `action` on `rows`, moved
    /// together.
    pub fn offer_undo(&mut self, rows: &[(&str, SummaryKind)], action: &MailAction) {
        self.undo = self.undo_offer(rows, action);
    }

    /// The offer to take back a move of `rows`, each out of the physical
    /// folder it is known to be in; asked before they leave the list, which
    /// tells where they were. Label, origin and global-search views cannot
    /// identify that folder.
    fn undo_offer(&self, rows: &[(&str, SummaryKind)], action: &MailAction) -> Option<UndoMove> {
        let to = action.destination()?;
        let rows: Vec<MovedRow> = rows
            .iter()
            .filter_map(|&(row_id, kind)| {
                let from = self.location_of(row_id).filter(|from| *from != to)?;
                Some(MovedRow {
                    row_id: row_id.to_owned(),
                    kind,
                    from: from.clone(),
                    row: self.row(row_id).cloned(),
                })
            })
            .collect();
        (!rows.is_empty()).then(|| UndoMove {
            rows,
            to: to.clone(),
        })
    }

    /// Claims the offer, so it is made only once. While the move back could
    /// not start (see `takes_actions`), the offer stays. Behind an action
    /// still on its way, the move back waits its turn.
    pub fn take_undo(&mut self) -> Option<UndoMove> {
        if !self.takes_actions() {
            return None;
        }
        self.undo.take()
    }

    /// Dismisses only the offer whose timer expired.
    pub fn dismiss_undo(&mut self, offer: &UndoMove) {
        if self.undo.as_ref() == Some(offer) {
            self.undo = None;
        }
    }

    /// Accepts an action on the selected row and shows it at once, before
    /// Proton answers, when `takes_actions` lets it start. The app names the
    /// row, through `start_action_on`; tests act on the open one.
    #[cfg(test)]
    pub fn start_action(&mut self, action: MailAction, request: RequestId) -> Option<ActionStart> {
        let (row_id, kind) = self
            .selected_summary()
            .map(|summary| (summary.id.clone(), summary.kind))?;

        self.start_action_on(row_id, kind, action, request)
    }

    /// Starts an action on a row by name, for rows the list no longer shows,
    /// such as one being moved back where it came from. Like `start_action`,
    /// it shows at once. It is sent now unless another action is on its way,
    /// or a read mutation waits for an automatic read of the same row.
    pub fn start_action_on(
        &mut self,
        row_id: String,
        kind: SummaryKind,
        action: MailAction,
        request: RequestId,
    ) -> Option<ActionStart> {
        if !self.takes_actions() {
            return None;
        }

        let request = self.new_action_request(request, row_id, kind, action);
        self.action_error = None;
        let opened = self.show_action(&request);
        if self.action_in_flight.is_some() {
            self.waiting_actions.push_back(request);
            return Some(ActionStart { send: None, opened });
        }
        self.action_in_flight = Some(request.clone());
        Some(ActionStart {
            send: self.ready_to_send(&request).then_some(request),
            opened,
        })
    }

    /// Starts `action` on each of `rows`, as `start_action_on` does: all of
    /// them show at once, and they are sent one after another. A move that
    /// takes them out of the view offers to take all of them back together.
    pub fn start_actions_on(
        &mut self,
        rows: &[String],
        action: MailAction,
        requests: &[RequestId],
    ) -> Vec<ActionStart> {
        let rows: Vec<(&str, SummaryKind)> = rows
            .iter()
            .filter_map(|row_id| Some((row_id.as_str(), self.row(row_id)?.kind)))
            .collect();
        // Asked while every row is still listed; each start then replaces the
        // offer with its own.
        let offer = self
            .leaves_view(&self.folder, &action)
            .then(|| self.undo_offer(&rows, &action))
            .flatten();
        let mut started = Vec::new();
        let starts: Vec<ActionStart> = rows
            .iter()
            .zip(requests)
            .filter_map(|(&(row_id, kind), &request)| {
                let start =
                    self.start_action_on(row_id.to_owned(), kind, action.clone(), request)?;
                started.push(request);
                Some(start)
            })
            .collect();
        if !starts.is_empty() && offer.is_some() {
            self.undo = offer;
        }
        // Each goes in the same request as the one before it, when it can.
        self.rides_with_previous
            .extend(started.iter().skip(1).copied());
        if starts.first().is_some_and(|start| start.send.is_some()) {
            self.join_in_flight();
        }
        starts
    }

    /// Takes a moved row back, as `start_action_on` starts it, and lists the
    /// row again at once when the open view keeps it: a reload reads only the
    /// first page, so a row from a deeper one would not come back with it.
    /// If Proton refuses the move back, the row leaves again.
    pub fn start_move_back(&mut self, moved: MovedRow, request: RequestId) -> Option<ActionStart> {
        let action = MailAction::MoveTo(moved.from);
        let start = self.start_action_on(moved.row_id, moved.kind, action.clone(), request)?;
        if let Some(row) = moved.row
            && !self.leaves_view(&self.folder, &action)
            && !self.has_row(&row.id)
        {
            self.list_again(row);
        }
        Some(start)
    }

    /// Lists `row` again where its date puts it among the loaded rows, and
    /// remembers that Proton has yet to confirm it is here.
    fn list_again(&mut self, row: ConversationSummary) {
        self.listed_again.insert(row.id.clone());
        self.list_by_date(row);
    }

    /// Lists `row` where its date puts it among the loaded rows, in the
    /// list's order.
    fn list_by_date(&mut self, row: ConversationSummary) {
        let rank = |time: Option<i64>| time.map(|time| self.order.rank(time));
        let at = self
            .conversations
            .iter()
            .position(|listed| rank(listed.time) < rank(row.time))
            .unwrap_or(self.conversations.len());
        self.row_ids.insert(row.id.clone());
        self.conversations.insert(at, row);
        self.last_dated_time = last_dated_time(&self.conversations);
        self.recompute_visible();
    }

    /// Whether a request can go now: a read mutation waits for an automatic
    /// read of the same row already on its way.
    fn ready_to_send(&self, request: &ActionRequest) -> bool {
        !(request.action.changes_read() && self.automatic_reads.contains_key(&request.row_id))
    }

    /// Shows an action before Proton confirms it. A move out of the view takes
    /// the row away, offers to take it back and returns the row opened in its
    /// place; a flag changes on every copy of the row.
    fn show_action(&mut self, request: &ActionRequest) -> Option<String> {
        if self.leaves_view(&self.folder, &request.action) {
            // Offered while the row is still listed, which tells where it was.
            self.offer_undo(&[(&request.row_id, request.kind)], &request.action);
            if self.row_ids.contains(&request.row_id) {
                self.moving_out.insert(request.row_id.clone());
            }
            return self.remove_row(&request.row_id);
        }
        // Only the last move can be taken back, and only until the next action.
        self.undo = None;
        let before = self
            .row(&request.row_id)
            .and_then(|row| match request.action {
                MailAction::SetUnread(unread) if row.unread != unread => {
                    Some(MailAction::SetUnread(row.unread))
                }
                MailAction::SetStarred(starred) if row.starred != starred => {
                    Some(MailAction::SetStarred(row.starred))
                }
                _ => None,
            });
        if let Some(before) = before {
            self.shown_flags.insert(request.id, before);
        }
        self.record_action(&request.row_id, &request.action);
        None
    }

    /// Settles the action on its way and names the next one to send. A refused
    /// action is taken back: a flag returns to what it was, and a row a move
    /// took out of the view comes back with the list, which the app reloads.
    pub fn finish_action(
        &mut self,
        request: &ActionRequest,
        result: Result<(), MailboxError>,
    ) -> ActionFinish {
        if self.action_in_flight.as_ref() != Some(request) {
            return ActionFinish {
                result: Ok(()),
                next_action: None,
                opened: None,
            };
        }
        let next_action = if self.sent_with.is_empty() {
            self.action_in_flight = self.waiting_actions.pop_front();
            if let Some(head) = &self.action_in_flight {
                self.rides_with_previous.remove(&head.id);
            }
            let next = self
                .action_in_flight
                .clone()
                .filter(|next| self.ready_to_send(next));
            if next.is_some() {
                self.join_in_flight();
            }
            next
        } else {
            // The next one sent with it is already on its way.
            self.action_in_flight = Some(self.sent_with.remove(0));
            None
        };
        let before = self.shown_flags.remove(&request.id);
        self.invalidate_listings();
        if let Err(error) = result {
            self.action_error = Some(error);
            self.take_back(request, before);
            return ActionFinish {
                result: Err(error),
                next_action,
                opened: None,
            };
        }
        // After a folder switch these rows no longer contain the acted-on row.
        if !self.shows(&request.folder) {
            return ActionFinish {
                result: Ok(()),
                next_action,
                opened: None,
            };
        }
        if matches!(
            self.status,
            ListStatus::Loading(_) | ListStatus::Refreshing(_)
        ) {
            self.confirmed_while_loading.push(request.clone());
        }

        // A reload while the move was on its way can have brought the row back,
        // and a confirmed automatic read can have covered an unread shown early.
        // A later move of the same row, such as one taking it back, decides
        // whether it stays.
        let opened = if self.leaves_view(&self.folder, &request.action) {
            if self.moving_out.remove(&request.row_id) {
                self.moved_out.insert(request.row_id.clone());
            }
            if self.superseded(request) {
                None
            } else {
                self.remove_row(&request.row_id)
            }
        } else {
            // A move that keeps its row here is where Proton now lists it.
            if request.action.destination().is_some() {
                self.listed_again.remove(&request.row_id);
                // Back where it was, it counts in Proton's pages again.
                self.moved_out.remove(&request.row_id);
            }
            if let MailAction::SetUnread(unread) = request.action {
                self.read_changed(&request.row_id, unread);
            }
            if !self.superseded(request) {
                self.record_action(&request.row_id, &request.action);
            }
            None
        };
        // The reader shows which labels the open conversation carries, and it
        // is the row that was just acted on.
        if let (MailAction::SetLabel { label, on }, ReaderState::Loaded(reader)) =
            (&request.action, &mut self.reader)
            && reader.conversation_id() == request.row_id
        {
            reader.set_label(label, *on);
        }
        ActionFinish {
            result: Ok(()),
            next_action,
            opened,
        }
    }

    /// Undoes what a refused action showed early, where it can be undone here:
    /// `before` is the flag it replaced.
    fn take_back(&mut self, request: &ActionRequest, before: Option<MailAction>) {
        self.moving_out.remove(&request.row_id);
        // A move back that did not happen leaves its row where it went.
        if self.listed_again.remove(&request.row_id) {
            let _ = self.remove_row(&request.row_id);
        }
        // The move did not happen, so there is nothing to take back for this
        // row; the rows moved with it still go back together.
        if let Some(offer) = &mut self.undo {
            offer.rows.retain(|moved| moved.row_id != request.row_id);
            if offer.rows.is_empty() {
                self.undo = None;
            }
        }
        if let Some(before) =
            before.filter(|_| self.shows(&request.folder) && !self.superseded(request))
        {
            self.record_action(&request.row_id, &before);
        }
        // A refused move that kept its row leaves its place unknown again.
        if request.action.destination().is_some()
            && self.shows(&request.folder)
            && !self.leaves_view(&self.folder, &request.action)
            && !self.superseded(request)
        {
            self.relocated.remove(&request.row_id);
        }
    }

    /// Whether an action asked for later changes the same thing on the same
    /// row, so that what it shows is what stays.
    fn superseded(&self, request: &ActionRequest) -> bool {
        self.queued_actions().any(|later| {
            later.row_id == request.row_id
                && std::mem::discriminant(&later.action) == std::mem::discriminant(&request.action)
        })
    }

    /// Whether a refused action had taken its row out of the view on screen,
    /// so only a reload can bring it back.
    pub(super) fn took_out(&self, request: &ActionRequest) -> bool {
        self.shows(&request.folder) && self.leaves_view(&self.folder, &request.action)
    }

    /// Whether a confirmed move belongs in the open view but its row is not
    /// listed, as when a move is taken back; only a reload can show it.
    pub(super) fn brought_back(&self, request: &ActionRequest) -> bool {
        request.action.destination().is_some()
            && !self.leaves_view(&self.folder, &request.action)
            && !self.has_row(&request.row_id)
    }

    /// Whether this action removes the row from the current view.
    fn leaves_view(&self, folder: &Folder, action: &MailAction) -> bool {
        match action {
            MailAction::SetStarred(starred) => {
                !starred && folder.system() == Some(MailFolder::Starred)
            }
            MailAction::SetLabel { label, on } => !on && label == folder,
            MailAction::SetUnread(_) => false,
            MailAction::MoveTo(to) if lists_by_label(folder) => {
                // Moved to another folder, mail keeps its star and labels. Spam
                // and Trash still take the row away, as they always have: the
                // next refresh says whether Proton lists it there.
                matches!(to.system(), Some(MailFolder::Spam | MailFolder::Trash))
            }
            MailAction::MoveTo(to) => to != folder,
        }
    }

    /// Shows a change on every loaded copy of the row.
    pub fn record_action(&mut self, row_id: &str, action: &MailAction) {
        for row in self.row_copies(row_id) {
            match action {
                MailAction::SetUnread(unread) => row.unread = *unread,
                MailAction::SetStarred(starred) => row.starred = *starred,
                MailAction::SetLabel { label, on } => {
                    if let Some(id) = label.custom_id() {
                        row.labels.retain(|known| known != id);
                        if *on {
                            row.labels.push(id.to_owned());
                        }
                    }
                }
                MailAction::MoveTo(_) => {}
            }
        }
        if !self.leaves_view(&self.folder, action) {
            if let Some(to) = action.destination() {
                self.relocated.insert(row_id.to_owned(), to.clone());
            }
        } else if let Some(search) = &mut self.search {
            search.rows.retain(|row| row.id != row_id);
            search.row_ids.remove(row_id);
            self.recompute_visible();
        }
    }

    /// Removes a row. When it was selected, clears the reader and returns the
    /// visible row that took its place.
    fn remove_row(&mut self, row_id: &str) -> Option<String> {
        let was_selected = self.selected_conversation() == Some(row_id);
        let index = self
            .visible_conversations()
            .position(|row| row.id == row_id);
        self.conversations.retain(|row| row.id != row_id);
        self.row_ids.remove(row_id);
        self.groups.retain_rows(&self.row_ids);
        self.last_dated_time = last_dated_time(&self.conversations);
        if let Some(search) = &mut self.search {
            search.rows.retain(|row| row.id != row_id);
            search.row_ids.remove(row_id);
        }
        self.recompute_visible();
        if !was_selected {
            return None;
        }

        match index {
            Some(index) => self.open_row_after(index),
            // What was typed hid it, so it had no place on screen to fill.
            None => {
                self.discard_reader();
                None
            }
        }
    }

    /// Closes the reader and names the visible row that took the place of the
    /// one that was at `index`, so reading continues where it left off.
    fn open_row_after(&mut self, index: usize) -> Option<String> {
        self.discard_reader();
        let last = self.visible_cache.len().checked_sub(1)?;

        self.visible_row(index.min(last)).map(|row| row.id.clone())
    }

    /// Whether `folder` is the one open, by its stable identity: work asked
    /// for under a name the folder has since changed still belongs to it.
    pub fn shows(&self, folder: &Folder) -> bool {
        FolderKey::of(folder) == FolderKey::of(&self.folder)
    }

    /// Updates an account folder's name without reloading its stable id.
    pub fn rename_folder(&mut self, folder: Folder) {
        let same = matches!(
            (self.folder.custom_id(), folder.custom_id()),
            (Some(open), Some(current)) if open == current
        );
        if same {
            self.folder = folder;
        }
    }

    pub fn select_folder(&mut self, folder: Folder, request: RequestId) -> Option<PageRequest> {
        if folder == self.folder {
            return None;
        }

        self.cache_active_listing();
        self.folder = folder;
        // Each folder opens with all its conversations, newest first.
        self.filter = ListFilter::All;
        self.order = ListOrder::NewestFirst;
        self.set_reader(ReaderState::Empty);
        self.action_error = None;
        self.leave_listing();
        self.show_listing(request)
    }

    /// Lists only the folder's conversations that `filter` shows, or all of
    /// them again. The open conversation stays in the reader.
    pub fn show_only(&mut self, filter: ListFilter, request: RequestId) -> Option<PageRequest> {
        self.list_as(filter, self.order, request)
    }

    /// Lists the folder's conversations in `order`.
    pub fn sort_by(&mut self, order: ListOrder, request: RequestId) -> Option<PageRequest> {
        self.list_as(self.filter, order, request)
    }

    fn list_as(
        &mut self,
        filter: ListFilter,
        order: ListOrder,
        request: RequestId,
    ) -> Option<PageRequest> {
        if (filter, order) == (self.filter, self.order) {
            return None;
        }
        // Kept as when leaving the folder, a listing of all of them, newest
        // first, is back at once when the list shows that again.
        self.cache_active_listing();
        self.filter = filter;
        self.order = order;
        self.leave_listing();
        self.show_listing(request)
    }

    /// Whether the list shows all of the folder, newest first: the one
    /// listing that is kept for later.
    fn lists_everything(&self) -> bool {
        self.filter == ListFilter::All && self.order == ListOrder::NewestFirst
    }

    /// Forgets what belongs to the listing on screen, which another is about
    /// to take the place of.
    fn leave_listing(&mut self) {
        // The offer belongs to the list it was made in.
        self.undo = None;
        self.relocated.clear();
        self.listed_again.clear();
        self.moving_out.clear();
        self.moved_out.clear();
        // So do search results: another list is a different question.
        self.search = None;
        self.search_request = None;
        self.search_deferred = false;
    }

    /// Shows the open folder's listing: the one kept from before, unless the
    /// list shows only some of it or another order, or else a first page to
    /// fetch.
    fn show_listing(&mut self, request: RequestId) -> Option<PageRequest> {
        let cached = if self.lists_everything() {
            self.take_cached_listing(&FolderKey::of(&self.folder))
        } else {
            None
        };
        // Only now, so the folder being opened is never the one let go.
        if self.cached_listings.len() > LISTING_CACHE_CAPACITY {
            self.cached_listings.remove(0);
        }
        if let Some(cached) = cached {
            self.put_listing(cached.listing);
            self.active_dirty = cached.dirty;
            self.recompute_visible();
            if cached.dirty {
                // From this point on only a change made while this request is
                // running can make its answer stale again.
                self.active_dirty = false;
                self.status = ListStatus::Refreshing(request);
                Some(self.first_page_request(request))
            } else {
                self.status = ListStatus::Loaded;
                None
            }
        } else {
            self.put_listing(Listing::default());
            self.active_dirty = false;
            self.recompute_visible();
            self.status = ListStatus::Loading(request);
            Some(self.first_page_request(request))
        }
    }

    /// Stores only a real folder listing of all its conversations, newest
    /// first. An initial load has `next_page == 0`; search results, reader
    /// state and transient actions deliberately stay out.
    fn cache_active_listing(&mut self) {
        if self.next_page == 0 || !self.lists_everything() {
            return;
        }
        let dirty = self.active_dirty || matches!(self.status, ListStatus::Refreshing(_));
        let folder = FolderKey::of(&self.folder);
        self.cached_listings
            .retain(|cached| cached.folder != folder);
        let listing = self.take_listing();
        self.cached_listings.push(CachedListing {
            folder,
            listing,
            dirty,
        });
    }

    /// The open folder's listing, taken out of the mailbox, which is left
    /// with none.
    fn take_listing(&mut self) -> Listing {
        Listing {
            conversations: std::mem::take(&mut self.conversations),
            row_ids: std::mem::take(&mut self.row_ids),
            last_dated_time: self.last_dated_time.take(),
            next_page: std::mem::take(&mut self.next_page),
            has_more: std::mem::take(&mut self.has_more),
            groups: std::mem::take(&mut self.groups),
        }
    }

    /// Makes `listing` the open folder's. Each of its fields is named here,
    /// so one added to [`Listing`] cannot be left behind.
    fn put_listing(&mut self, listing: Listing) {
        let Listing {
            conversations,
            row_ids,
            last_dated_time,
            next_page,
            has_more,
            groups,
        } = listing;
        self.conversations = conversations;
        self.row_ids = row_ids;
        self.last_dated_time = last_dated_time;
        self.next_page = next_page;
        self.has_more = has_more;
        self.groups = groups;
    }

    fn take_cached_listing(&mut self, folder: &FolderKey) -> Option<CachedListing> {
        let index = self
            .cached_listings
            .iter()
            .position(|cached| cached.folder == *folder)?;
        Some(self.cached_listings.remove(index))
    }

    /// Marks cached folders stale after a confirmed server mutation.
    pub(super) fn invalidate_listings(&mut self) {
        self.inspection_revision = self
            .inspection_revision
            .checked_add(1)
            .expect("inspection revision exhausted");
        self.active_dirty = true;
        self.invalidate_cached_listings();
    }

    /// A periodic refresh updates the open folder now. Everything else stays
    /// instant to open, but must revalidate before becoming a clean cache hit.
    pub(super) fn invalidate_cached_listings(&mut self) {
        for cached in &mut self.cached_listings {
            cached.dirty = true;
        }
    }

    /// Reloads the first page. The undo offer stays: the moved row is already
    /// gone from the list, so a reload changes nothing the offer describes.
    pub fn refresh(&mut self, request: RequestId) -> Option<PageRequest> {
        if self.is_busy() {
            return None;
        }
        // If a mutation completes after this request starts it sets this back
        // to true, and the app follows this response with one fresh request.
        self.active_dirty = false;

        self.status = if self.conversations.is_empty() {
            ListStatus::Loading(request)
        } else {
            ListStatus::Refreshing(request)
        };
        Some(self.first_page_request(request))
    }

    pub fn load_more(&mut self, request: RequestId) -> Option<PageRequest> {
        if self.is_busy() || self.search.is_some() || !self.has_more {
            return None;
        }

        self.status = ListStatus::LoadingMore(request);
        // Rows that left the loaded pages moved Proton's later ones up: ask for
        // the page holding the first row not loaded, and what it repeats is
        // dropped as already listed.
        let moved_out = u32::try_from(self.moved_out.len()).unwrap_or(u32::MAX);
        let first_unloaded = (self.next_page * self.page_size).saturating_sub(moved_out);
        self.more_page = first_unloaded / self.page_size;
        Some(self.page_request(request, self.more_page))
    }

    /// Starts a counts refresh, superseding any older request.
    pub fn refresh_counts(&mut self, request: RequestId) -> RequestId {
        self.counts_request = Some(request);
        request
    }

    pub fn loaded_conversation_limit(&self) -> u32 {
        self.page_size.saturating_mul(self.next_page.max(1))
    }

    /// Replaces demo data after a local action. Returns the next conversation
    /// to open when the selected one left the current folder.
    pub fn apply_action_snapshot(
        &mut self,
        page: ConversationPage,
        counts: MailboxCounts,
    ) -> Option<String> {
        let selected = self.selected_conversation().map(str::to_owned);
        let selected_index = selected
            .as_deref()
            .and_then(|id| {
                self.visible_conversations()
                    .position(|conversation| conversation.id == id)
            })
            .unwrap_or(0);
        let total = page.total;

        self.conversations = page.conversations;
        self.groups = ConversationGroups::default();
        self.row_ids = collect_ids(&self.conversations);
        self.last_dated_time = last_dated_time(&self.conversations);
        self.counts = Some(counts);
        self.counts_request = None;
        self.status = ListStatus::Loaded;
        self.has_more = self.conversations.len() < total as usize;
        // The demo snapshot is authoritative for the active folder. Other
        // folders may have changed as a side effect and must be revalidated.
        for cached in &mut self.cached_listings {
            cached.dirty = true;
        }
        self.active_dirty = false;
        self.recompute_visible();

        let selected_left = selected.as_deref().is_some_and(|id| {
            !self
                .visible_conversations()
                .any(|conversation| conversation.id == id)
        });
        if !selected_left {
            return None;
        }

        self.open_row_after(selected_index)
    }

    /// Applies a page response. Returns the error of an accepted failed
    /// response; stale responses are ignored.
    pub fn finish_page(
        &mut self,
        request: RequestId,
        result: Result<ConversationPage, MailboxError>,
    ) -> Option<MailboxError> {
        let append = match self.status {
            ListStatus::Loading(id) | ListStatus::Refreshing(id) if id == request => false,
            ListStatus::LoadingMore(id) if id == request => true,
            _ => return None,
        };

        match result {
            Ok(page) => {
                self.apply_page(page, append);
                self.status = ListStatus::Loaded;
                None
            }
            Err(error) => {
                if !append {
                    self.active_dirty = true;
                }
                let attempt = if append {
                    ListAttempt::More
                } else {
                    ListAttempt::Folder
                };
                self.status = ListStatus::Failed(error, attempt);
                Some(error)
            }
        }
    }

    /// Replaces an uninspected conversation with its split message rows once
    /// background inspection confirms it is repeated inbound mail.
    /// Updates the active folder listing if currently viewed, or any cached
    /// listing for that folder if the user navigated away.
    pub fn split_conversation(
        &mut self,
        folder: &Folder,
        conversation_id: &str,
        split_rows: Vec<ConversationSummary>,
    ) {
        if self.shows(folder) {
            if self.groups.apply_split(
                &mut self.conversations,
                self.order,
                conversation_id,
                split_rows,
            ) {
                self.row_ids = collect_ids(&self.conversations);
                self.last_dated_time = last_dated_time(&self.conversations);
                self.recompute_visible();
            }
        } else if let Some(cached) = self
            .cached_listings
            .iter_mut()
            .find(|cached| cached.folder == FolderKey::of(folder))
            // Only a listing newest first is ever kept.
            && cached.listing.groups.apply_split(
                &mut cached.listing.conversations,
                ListOrder::NewestFirst,
                conversation_id,
                split_rows,
            )
        {
            let listing = &mut cached.listing;
            listing.row_ids = collect_ids(&listing.conversations);
            listing.last_dated_time = last_dated_time(&listing.conversations);
        }
    }

    /// Applies a counts response. Returns the error of an accepted failed
    /// response; previously loaded counts are kept on failure.
    pub fn finish_counts(
        &mut self,
        request: RequestId,
        result: Result<MailboxCounts, MailboxError>,
    ) -> Option<MailboxError> {
        if self.counts_request != Some(request) {
            return None;
        }

        self.counts_request = None;
        match result {
            Ok(counts) => {
                self.counts = Some(counts);
                None
            }
            Err(error) => Some(error),
        }
    }

    /// Refresh by server conversation identity. A split member's old timestamp
    /// must not make it look like a different, deeper server conversation.
    fn refreshed_rows(
        &mut self,
        mut fresh: Vec<ConversationSummary>,
        keep_deeper: bool,
    ) -> Vec<ConversationSummary> {
        deduplicate_rows(&mut fresh);
        // The rows a page ends at rank lowest, in either order.
        let order = self.order;
        let deepest = fresh
            .iter()
            .filter_map(|row| row.time)
            .map(|time| order.rank(time))
            .min();
        let mut covered = collect_ids(&fresh);
        for source in &fresh {
            if let Some(group) = self.groups.splits.get(&source.id) {
                covered.extend(group.message_ids.iter().cloned());
            }
        }
        let mut previous = HashMap::new();
        let mut deeper = Vec::new();
        for row in std::mem::take(&mut self.conversations) {
            if covered.contains(&row.id) {
                previous.entry(row.id.clone()).or_insert(row);
            } else if keep_deeper && let Some(deepest) = deepest {
                let parent = self.groups.parents.get(&row.id);
                let time = parent
                    .and_then(|id| self.groups.splits.get(id))
                    .map_or(row.time, |group| group.source.time);
                if time.is_some_and(|time| order.rank(time) < deepest)
                    && !parent.is_some_and(|id| covered.contains(id))
                {
                    deeper.push(row);
                }
            }
        }
        let mut rows = Vec::new();
        for source in fresh {
            if let Some(group) = self.groups.reusable(&source, &self.row_ids) {
                let members = group
                    .message_ids
                    .iter()
                    .filter_map(|id| previous.remove(id));
                rows.extend(members);
                group.source = source;
            } else {
                let _ = self.groups.forget(&source.id);
                rows.push(source);
            }
        }
        rows.extend(deeper);
        deduplicate_rows(&mut rows);
        rows
    }

    /// The actions a first page may have been read without, in the order
    /// asked: those confirmed since it was asked for, then those still on
    /// their way or waiting.
    fn unseen_actions(&mut self) -> Vec<ActionRequest> {
        let confirmed = std::mem::take(&mut self.confirmed_while_loading);
        confirmed
            .into_iter()
            .chain(self.queued_actions().cloned())
            .filter(|request| self.shows(&request.folder))
            .collect()
    }

    /// Shows `actions` again on the rows a first page just replaced, in the
    /// order asked: a move out takes its row away, a move that keeps its row
    /// here lists it as `before` held it, and a flag changes again.
    fn show_again(&mut self, actions: &[ActionRequest], before: &[ConversationSummary]) {
        for request in actions {
            if self.leaves_view(&self.folder, &request.action) {
                let _ = self.remove_row(&request.row_id);
                continue;
            }
            if request.action.destination().is_some()
                && !self.row_ids.contains(&request.row_id)
                && let Some(row) = before.iter().find(|row| row.id == request.row_id)
            {
                self.list_by_date(row.clone());
            }
            self.record_action(&request.row_id, &request.action);
        }
    }

    fn page_request(&self, id: RequestId, page: u32) -> PageRequest {
        PageRequest {
            id,
            folder: self.folder.clone(),
            filter: self.filter,
            order: self.order,
            page,
            page_size: self.page_size,
        }
    }

    /// Asks for the folder's first page. Only actions confirmed from now on
    /// can be missing from what it brings.
    fn first_page_request(&mut self, id: RequestId) -> PageRequest {
        self.confirmed_while_loading.clear();
        self.page_request(id, 0)
    }

    fn apply_page(&mut self, page: ConversationPage, append: bool) {
        let received = page.conversations.len();

        if append {
            let mut invalidated = HashSet::new();
            for source in &page.conversations {
                if let Some(group) = self.groups.reusable(source, &self.row_ids) {
                    group.source = source.clone();
                } else if let Some(group) = self.groups.forget(&source.id) {
                    invalidated.extend(group.message_ids);
                }
            }
            if !invalidated.is_empty() {
                self.conversations
                    .retain(|row| !invalidated.contains(&row.id));
                self.row_ids = collect_ids(&self.conversations);
                self.last_dated_time = last_dated_time(&self.conversations);
            }
            let mut incoming_ids = HashSet::new();
            let new: Vec<_> = page
                .conversations
                .into_iter()
                .filter(|conversation| {
                    // Metadata was checked above; removals can still leave a
                    // previously reusable group with missing members.
                    !self
                        .groups
                        .contains_members(&conversation.id, &self.row_ids)
                        && !self.row_ids.contains(&conversation.id)
                        && incoming_ids.insert(conversation.id.clone())
                })
                .collect();
            self.row_ids.extend(new.iter().map(|row| row.id.clone()));
            let from = self.conversations.len();
            let appended = append_in_order(
                &mut self.conversations,
                new,
                self.last_dated_time,
                self.order,
            );
            self.last_dated_time = appended.last_dated_time;
            if appended.at_end && invalidated.is_empty() && self.search.is_none() {
                self.append_visible(from);
            } else {
                self.recompute_visible();
            }
            // What is loaded now reaches the end of the page asked for.
            self.next_page = self.more_page + 1;
            self.moved_out.clear();
            if !invalidated.is_empty() {
                self.groups.retain_rows(&self.row_ids);
            }
        } else {
            // Proton may have read the page before applying these, so what
            // they showed stays.
            let unseen = self.unseen_actions();
            let before: Vec<_> = self
                .conversations
                .iter()
                .filter(|row| unseen.iter().any(|request| request.row_id == row.id))
                .cloned()
                .collect();
            self.conversations =
                self.refreshed_rows(page.conversations, received >= self.page_size as usize);
            self.row_ids = collect_ids(&self.conversations);
            // Rows split out of a conversation carry their own, older times.
            sort_by_date(&mut self.conversations, self.order);
            self.last_dated_time = last_dated_time(&self.conversations);
            self.recompute_visible();
            self.next_page = self.next_page.max(1);
            self.groups.retain_rows(&self.row_ids);
            self.show_again(&unseen, &before);
        }
        if !append
            && self
                .selected_conversation()
                .is_some_and(|id| !self.has_row(id))
        {
            // A refreshed page can regroup a split message or omit its row.
            // Keep the open content; absence from a page is not a deletion.
            // It must not become a cache hit using the old row's metadata.
            self.reader_stamp = None;
        }

        self.has_more = has_more_pages(
            received,
            u64::from(self.next_page),
            self.page_size,
            page.total,
        );
    }
}

/// Whether a listing has pages left after `pages_loaded` pages of
/// `page_size`. An empty page means it ran out, whatever `total` says.
fn has_more_pages(received: usize, pages_loaded: u64, page_size: u32, total: u32) -> bool {
    received > 0 && pages_loaded * u64::from(page_size) < u64::from(total)
}

fn deduplicate_rows(rows: &mut Vec<ConversationSummary>) {
    let mut seen = HashSet::new();
    rows.retain(|row| seen.insert(row.id.clone()));
}

fn collect_ids(rows: &[ConversationSummary]) -> HashSet<String> {
    rows.iter().map(|row| row.id.clone()).collect()
}

fn last_dated_time(rows: &[ConversationSummary]) -> Option<i64> {
    rows.iter().rev().find_map(|row| row.time)
}

/// Where a page went: the time of the last dated row after it, and whether
/// it went at the end, after every row already there.
struct Appended {
    last_dated_time: Option<i64>,
    at_end: bool,
}

/// Keeps older pages proportional to the new page. Interleaved pages use a
/// stable linear merge, with earlier pages winning ties. Rows compare by the
/// rank `order` gives their dates, which is higher the earlier a row goes.
fn append_in_order(
    rows: &mut Vec<ConversationSummary>,
    incoming: Vec<ConversationSummary>,
    last_dated: Option<i64>,
    order: ListOrder,
) -> Appended {
    let first_rank = |rows: &[ConversationSummary]| {
        let time = rows.iter().find_map(|row| row.time);
        time.map(|time| order.rank(time))
    };
    let last_rank = last_dated.map(|time| order.rank(time));
    let seed = last_rank
        .or_else(|| first_rank(&incoming))
        .unwrap_or(i64::MAX);
    let mut keyed = ranked(incoming, seed, order);
    let tail_key = last_rank.unwrap_or(seed);

    keyed.sort_by_key(|(rank, _)| std::cmp::Reverse(*rank));
    if !rows.is_empty() && keyed.first().is_some_and(|(rank, _)| *rank > tail_key) {
        // The old rows are already ordered. Recreate their undated sort keys
        // using the same seed as sorting the concatenated pages would use.
        let old_seed = first_rank(rows).unwrap_or(seed);
        let mut old = ranked(std::mem::take(rows), old_seed, order)
            .into_iter()
            .peekable();
        let mut new = keyed.into_iter().peekable();
        rows.reserve(old.len() + new.len());

        while let (Some((listed, _)), Some((arriving, _))) = (old.peek(), new.peek()) {
            let next = if listed >= arriving {
                old.next()
            } else {
                new.next()
            };
            rows.extend(next.map(|(_, row)| row));
        }
        rows.extend(old.chain(new).map(|(_, row)| row));
        return Appended {
            last_dated_time: last_dated_time(rows),
            at_end: false,
        };
    }

    let last_dated_time = keyed
        .iter()
        .rev()
        .find_map(|(_, row)| row.time)
        .or(last_dated);
    rows.extend(keyed.into_iter().map(|(_, row)| row));
    Appended {
        last_dated_time,
        at_end: true,
    }
}

/// Sorts by date in `order` while preserving undated rows' relative
/// positions.
fn sort_by_date(rows: &mut Vec<ConversationSummary>, order: ListOrder) {
    let first = rows.iter().find_map(|row| row.time);
    let seed = first.map_or(i64::MAX, |time| order.rank(time));
    let mut keyed = ranked(rows.drain(..), seed, order);
    keyed.sort_by_key(|(rank, _)| std::cmp::Reverse(*rank));
    rows.extend(keyed.into_iter().map(|(_, row)| row));
}

/// Pairs each row with the rank it sorts by: its own date's in `order`, or
/// for an undated row that of the row before it, and `seed` for the first. A
/// stable sort then leaves undated rows beside their neighbours.
fn ranked(
    rows: impl IntoIterator<Item = ConversationSummary>,
    seed: i64,
    order: ListOrder,
) -> Vec<(i64, ConversationSummary)> {
    let mut previous = seed;
    rows.into_iter()
        .map(|row| {
            previous = row.time.map_or(previous, |time| order.rank(time));
            (previous, row)
        })
        .collect()
}

fn matches_search(conversation: &ConversationSummary, query: &str) -> bool {
    query.is_empty()
        || conversation
            .subject
            .as_deref()
            .into_iter()
            .chain(conversation.correspondents.as_deref())
            .chain(conversation.preview.as_deref())
            .any(|value| contains_ignoring_case(value, query))
        || conversation.participants.iter().any(|participant| {
            participant
                .name
                .as_deref()
                .is_some_and(|name| contains_ignoring_case(name, query))
                || contains_ignoring_case(&participant.address, query)
        })
}

/// Whether `haystack` holds `needle`, which is already lowercased.
/// ASCII avoids allocation; other text uses zero-allocation streaming Unicode lowercasing.
fn contains_ignoring_case(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    if haystack.is_ascii() && !needle.is_ascii() {
        return false;
    }
    if haystack.is_ascii() && needle.is_ascii() {
        // Every character is one byte here, so bytes and characters line up.
        return haystack
            .as_bytes()
            .windows(needle.len())
            .any(|window| window.eq_ignore_ascii_case(needle.as_bytes()));
    }

    // For non-ASCII text, avoid allocating a lowercased String on the heap by
    // streaming lowercased characters from each character start position in haystack.
    haystack.char_indices().any(|(start, _)| {
        let mut haystack_chars = haystack[start..].chars().flat_map(char::to_lowercase);
        let mut needle_chars = needle.chars().flat_map(char::to_lowercase);
        loop {
            match needle_chars.next() {
                None => return true,
                Some(nc) => {
                    if haystack_chars.next() != Some(nc) {
                        return false;
                    }
                }
            }
        }
    })
}

/// Whether a view lists mail by a name it carries, Starred or a label, rather
/// than by the folder it sits in.
fn lists_by_label(folder: &Folder) -> bool {
    folder.system() == Some(MailFolder::Starred) || folder.kind() == Some(CustomKind::Label)
}

#[cfg(test)]
mod tests;
