use std::collections::{HashMap, HashSet, VecDeque};

use super::reader::{ConversationReader, ReaderState};
use crate::mail::{
    ConversationDetail, ConversationPage, ConversationSummary, CustomKind, Folder, MailAction,
    MailFolder, MailboxCounts, MailboxError, SummaryKind,
};

/// Identifies an asynchronous mailbox request. Only the response matching the
/// request currently in flight is applied; anything else is stale.
pub type RequestId = u64;

/// Recent readers are kept only for quick backtracking within this session.
const READER_CACHE_CAPACITY: usize = 8;
/// Folders left most recently keep their loaded listing in memory, besides
/// the open one.
const LISTING_CACHE_CAPACITY: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageRequest {
    pub id: RequestId,
    pub folder: Folder,
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

/// A move the user can take back: the row, and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoMove {
    pub row_id: String,
    pub kind: SummaryKind,
    /// The folder the row was moved out of, and goes back to.
    pub from: Folder,
    /// Where it was moved, which is what the offer says.
    pub to: Folder,
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
}

/// The loaded part of one folder while another folder is open.
/// Memory-only, so Ruston never creates a second persistent mail store.
struct CachedListing {
    folder: FolderKey,
    conversations: Vec<ConversationSummary>,
    row_ids: HashSet<String>,
    last_dated_time: Option<i64>,
    next_page: u32,
    has_more: bool,
    /// A confirmed mailbox change may have affected this folder. It can still
    /// be shown immediately, but must be refreshed before it is cached clean.
    dirty: bool,
    groups: ConversationGroups,
}

struct SplitConversation {
    source: ConversationSummary,
    message_ids: Vec<String>,
}

impl SplitConversation {
    fn matches(&self, source: &ConversationSummary) -> bool {
        // Read/star flags do not change threading. Reinspection refreshes the
        // individual flags while these rows remain visible.
        let mut current = source.clone();
        current.unread = self.source.unread;
        current.starred = self.source.starred;
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
        sort_newest_first(rows);
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
    search_query: String,
    /// Results of the last server search, while they are on screen.
    search: Option<SearchState>,
    /// The search in flight, so a late answer to an abandoned one is ignored.
    search_request: Option<RequestId>,
    /// Enter was pressed while a folder page was loading. Searching then
    /// would make that page look stale, so the search waits for it.
    search_deferred: bool,
    /// The action on its way to Proton, or waiting for an automatic read of
    /// the same row.
    action_in_flight: Option<ActionRequest>,
    /// Actions asked for while another was on its way, in the order asked.
    /// Each already shows; they are sent one at a time, so taking a move back
    /// always follows the move.
    waiting_actions: VecDeque<ActionRequest>,
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
            search_query: String::new(),
            search: None,
            search_request: None,
            search_deferred: false,
            action_in_flight: None,
            waiting_actions: VecDeque::new(),
            shown_flags: HashMap::new(),
            action_error: None,
            undo: None,
            automatic_reads: HashMap::new(),
            relocated: HashMap::new(),
            listed_again: HashSet::new(),
            confirmed_while_loading: Vec::new(),
        };
        let page = mailbox.page_request(page_request, 0);

        (mailbox, page)
    }

    pub fn folder(&self) -> &Folder {
        &self.folder
    }

    pub fn status(&self) -> ListStatus {
        self.status
    }

    #[cfg(test)]
    pub fn conversations(&self) -> &[ConversationSummary] {
        &self.conversations
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
        self.status = ListStatus::Loading(request);

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
                    });
                    self.recompute_visible();
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

    /// Goes back to the folder listing, leaving any results behind.
    pub fn clear_search(&mut self) {
        let leaving_results = self.search.is_some();
        self.search = None;
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

    pub(super) fn inspection_revision(&self) -> u64 {
        self.inspection_revision
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
        let current = self.selected_conversation().and_then(|selected| {
            self.visible_conversations()
                .position(|row| row.id == selected)
        });

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

    pub fn reader_state(&self) -> &ReaderState {
        &self.reader
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

    /// A successful send can add a message to a previously loaded thread.
    pub(super) fn invalidate_reader_cache(&mut self) {
        self.cached_readers.clear();
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
        let row = self.row(reader.conversation_id())?;
        if !row.unread
            || self.automatic_reads.contains_key(&row.id)
            || self.read_change_queued(&row.id)
        {
            return None;
        }
        let (row_id, kind) = (row.id.clone(), row.kind);

        let request = self.new_action_request(request, row_id, kind, MailAction::SetUnread(false));
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
        self.action_in_flight.iter().chain(&self.waiting_actions)
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
        // The row is only marked here while its folder is open, but Proton's
        // counts changed wherever the person went meanwhile.
        if self.shows(&request.folder) {
            for row in self.row_copies(&request.row_id) {
                row.unread = false;
            }
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

    /// Replaces the undo offer for moves out of a known physical folder.
    /// Label, origin and global-search views cannot identify that folder.
    pub fn offer_undo(&mut self, row_id: &str, kind: SummaryKind, action: MailAction) {
        self.undo = match (self.location_of(row_id), action.destination()) {
            (Some(from), Some(to)) if to != from => Some(UndoMove {
                row_id: row_id.to_owned(),
                kind,
                from: from.clone(),
                to: to.clone(),
                row: self.row(row_id).cloned(),
            }),
            _ => None,
        };
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

    /// Takes a move back, as `start_action_on` starts it, and lists the row
    /// again at once when the open view keeps it: a reload reads only the
    /// first page, so a row from a deeper one would not come back with it.
    /// If Proton refuses the move back, the row leaves again.
    pub fn start_move_back(&mut self, undo: UndoMove, request: RequestId) -> Option<ActionStart> {
        let action = MailAction::MoveTo(undo.from);
        let start = self.start_action_on(undo.row_id, undo.kind, action.clone(), request)?;
        if let Some(row) = undo.row
            && !self.leaves_view(&self.folder, &action)
            && !self.has_row(&row.id)
        {
            self.list_again(row);
        }
        Some(start)
    }

    /// Lists `row` again where its date puts it among the loaded rows, newest
    /// first, and remembers that Proton has yet to confirm it is here.
    fn list_again(&mut self, row: ConversationSummary) {
        self.listed_again.insert(row.id.clone());
        self.list_by_date(row);
    }

    /// Lists `row` where its date puts it among the loaded rows, newest first.
    fn list_by_date(&mut self, row: ConversationSummary) {
        let at = self
            .conversations
            .iter()
            .position(|listed| listed.time < row.time)
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
            self.offer_undo(&request.row_id, request.kind, request.action.clone());
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
        self.action_in_flight = self.waiting_actions.pop_front();
        let next_action = self
            .action_in_flight
            .clone()
            .filter(|next| self.ready_to_send(next));
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
        // The move did not happen, so there is nothing to take back.
        if self
            .undo
            .as_ref()
            .is_some_and(|offer| offer.row_id == request.row_id)
        {
            self.undo = None;
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
                MailAction::MoveTo(_) | MailAction::SetLabel { .. } => {}
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
        self.set_reader(ReaderState::Empty);
        self.action_error = None;
        // The offer belongs to the folder it was made in.
        self.undo = None;
        self.relocated.clear();
        self.listed_again.clear();
        self.moving_out.clear();
        self.moved_out.clear();
        // So do search results: a folder is a different question.
        self.search = None;
        self.search_request = None;
        self.search_deferred = false;

        let cached = self.take_cached_listing(&FolderKey::of(&self.folder));
        // Only now, so the folder being opened is never the one let go.
        if self.cached_listings.len() > LISTING_CACHE_CAPACITY {
            self.cached_listings.remove(0);
        }
        if let Some(cached) = cached {
            self.conversations = cached.conversations;
            self.row_ids = cached.row_ids;
            self.last_dated_time = cached.last_dated_time;
            self.next_page = cached.next_page;
            self.has_more = cached.has_more;
            self.active_dirty = cached.dirty;
            self.groups = cached.groups;
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
            self.conversations.clear();
            self.row_ids.clear();
            self.last_dated_time = None;
            self.next_page = 0;
            self.has_more = false;
            self.active_dirty = false;
            self.groups = ConversationGroups::default();
            self.recompute_visible();
            self.status = ListStatus::Loading(request);
            Some(self.first_page_request(request))
        }
    }

    /// Stores only a real folder listing. An initial load has `next_page == 0`;
    /// search results, reader state and transient actions deliberately stay out.
    fn cache_active_listing(&mut self) {
        if self.next_page == 0 {
            return;
        }
        let dirty = self.active_dirty || matches!(self.status, ListStatus::Refreshing(_));
        let folder = FolderKey::of(&self.folder);
        self.cached_listings
            .retain(|cached| cached.folder != folder);
        self.cached_listings.push(CachedListing {
            folder,
            conversations: std::mem::take(&mut self.conversations),
            row_ids: std::mem::take(&mut self.row_ids),
            last_dated_time: self.last_dated_time.take(),
            next_page: self.next_page,
            has_more: self.has_more,
            dirty,
            groups: std::mem::take(&mut self.groups),
        });
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
            if self
                .groups
                .apply_split(&mut self.conversations, conversation_id, split_rows)
            {
                self.row_ids = collect_ids(&self.conversations);
                self.last_dated_time = last_dated_time(&self.conversations);
                self.recompute_visible();
            }
        } else if let Some(cached) = self
            .cached_listings
            .iter_mut()
            .find(|cached| cached.folder == FolderKey::of(folder))
            && cached
                .groups
                .apply_split(&mut cached.conversations, conversation_id, split_rows)
        {
            cached.row_ids = collect_ids(&cached.conversations);
            cached.last_dated_time = last_dated_time(&cached.conversations);
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
        let oldest = fresh.iter().filter_map(|row| row.time).min();
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
            } else if keep_deeper && let Some(oldest) = oldest {
                let parent = self.groups.parents.get(&row.id);
                let time = parent
                    .and_then(|id| self.groups.splits.get(id))
                    .map_or(row.time, |group| group.source.time);
                if time.is_some_and(|time| time < oldest)
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
            let appended = append_newest_first(&mut self.conversations, new, self.last_dated_time);
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
            sort_newest_first(&mut self.conversations);
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
/// stable linear merge, with earlier pages winning ties.
fn append_newest_first(
    rows: &mut Vec<ConversationSummary>,
    incoming: Vec<ConversationSummary>,
    last_dated: Option<i64>,
) -> Appended {
    let seed = last_dated
        .or_else(|| incoming.iter().find_map(|row| row.time))
        .unwrap_or(i64::MAX);
    let mut keyed = keyed_by_time(incoming, seed);
    let tail_key = last_dated.unwrap_or(seed);

    keyed.sort_by_key(|(time, _)| std::cmp::Reverse(*time));
    if !rows.is_empty() && keyed.first().is_some_and(|(time, _)| *time > tail_key) {
        // The old rows are already ordered. Recreate their undated sort keys
        // using the same seed as sorting the concatenated pages would use.
        let old_seed = rows.iter().find_map(|row| row.time).unwrap_or(seed);
        let mut old = keyed_by_time(std::mem::take(rows), old_seed)
            .into_iter()
            .peekable();
        let mut new = keyed.into_iter().peekable();
        rows.reserve(old.len() + new.len());

        while let (Some((older, _)), Some((newer, _))) = (old.peek(), new.peek()) {
            let next = if older >= newer {
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

/// Sorts newest first while preserving undated rows' relative positions.
fn sort_newest_first(rows: &mut Vec<ConversationSummary>) {
    let seed = rows.iter().find_map(|row| row.time).unwrap_or(i64::MAX);
    let mut keyed = keyed_by_time(rows.drain(..), seed);
    keyed.sort_by_key(|(time, _)| std::cmp::Reverse(*time));
    rows.extend(keyed.into_iter().map(|(_, row)| row));
}

/// Pairs each row with the time it sorts by: its own, or for an undated row
/// the time of the row before it, and `seed` for the first. A stable sort
/// then leaves undated rows beside their neighbours.
fn keyed_by_time(
    rows: impl IntoIterator<Item = ConversationSummary>,
    seed: i64,
) -> Vec<(i64, ConversationSummary)> {
    let mut previous = seed;
    rows.into_iter()
        .map(|row| {
            previous = row.time.unwrap_or(previous);
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
