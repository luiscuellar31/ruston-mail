use std::collections::{HashMap, HashSet};

use super::reader::{ConversationReader, ReaderState};
use crate::mail::{
    ConversationDetail, ConversationPage, ConversationSummary, Folder, MailAction, MailFolder,
    MailboxCounts, MailboxError, SummaryKind,
};

/// Identifies an asynchronous mailbox request. Only the response matching the
/// request currently in flight is applied; anything else is stale.
pub type RequestId = u64;

/// Recent readers are kept only for quick backtracking within this session.
const READER_CACHE_CAPACITY: usize = 8;

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

/// A move the user can take back: the row, and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoMove {
    pub row_id: String,
    pub kind: SummaryKind,
    /// The folder the row was moved out of, and goes back to.
    pub from: Folder,
    /// Where it was moved, which is what the offer says.
    pub to: Folder,
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
    Failed(MailboxError),
}

/// One server search: what was asked, and what came back. Results replace the
/// folder listing while they are shown, and never merge into it.
struct SearchState {
    query: String,
    rows: Vec<ConversationSummary>,
    next_page: u32,
    has_more: bool,
}

/// The loaded part of one folder while another folder is open.
/// Memory-only, so Ruston never creates a second persistent mail store.
struct CachedListing {
    conversations: Vec<ConversationSummary>,
    next_page: u32,
    has_more: bool,
    /// A confirmed mailbox change may have affected this folder. It can still
    /// be shown immediately, but must be refreshed before it is cached clean.
    dirty: bool,
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
    /// Indices of visible rows into either `search.rows` or `conversations`.
    visible_cache: Vec<usize>,
    page_size: u32,
    next_page: u32,
    has_more: bool,
    cached_listings: HashMap<FolderKey, CachedListing>,
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
    action_request: Option<ActionRequest>,
    action_error: Option<MailboxError>,
    /// The last move, while it can still be taken back.
    undo: Option<UndoMove>,
    /// Automatic reads in flight, by row, so each response only affects its
    /// own row and never blocks the action toolbar.
    pending_reads: HashMap<String, RequestId>,
}

impl Mailbox {
    /// Opens `folder`, which is the one last read, returning the first page
    /// to fetch. Counts are fetched under `counts_request`.
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
            visible_cache: Vec::new(),
            page_size,
            next_page: 0,
            has_more: false,
            cached_listings: HashMap::new(),
            active_dirty: false,
            counts: None,
            counts_request: Some(counts_request),
            reader: ReaderState::Empty,
            reader_stamp: None,
            cached_readers: Vec::new(),
            search_query: String::new(),
            search: None,
            search_request: None,
            action_request: None,
            action_error: None,
            undo: None,
            pending_reads: HashMap::new(),
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
            None => {
                let trimmed = self.search_query.trim();
                if trimmed.is_empty() {
                    self.visible_cache.extend(0..self.conversations.len());
                } else {
                    let query = trimmed.to_lowercase();
                    self.visible_cache
                        .extend(self.conversations.iter().enumerate().filter_map(
                            |(idx, conversation)| {
                                matches_search(conversation, &query).then_some(idx)
                            },
                        ));
                }
            }
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
    pub fn search_results(&self) -> Option<&str> {
        self.search.as_ref().map(|search| search.query.as_str())
    }

    pub fn search_query(&self) -> &str {
        &self.search_query
    }

    pub fn is_searching(&self) -> bool {
        !self.search_query.trim().is_empty()
    }

    /// Starts a global backend search, or clears empty input.
    pub fn start_search(&mut self, request: RequestId) -> Option<SearchRequest> {
        let query = self.search_query.trim().to_owned();
        if query.is_empty() {
            self.clear_search();
            return None;
        }

        self.search_request = Some(request);
        self.status = ListStatus::Loading(request);

        Some(SearchRequest {
            id: request,
            query,
            page: 0,
            page_size: self.page_size,
        })
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
                let has_more = !page.conversations.is_empty()
                    && (u64::from(request.page) + 1) * u64::from(request.page_size)
                        < u64::from(page.total);
                if request.page > 0 {
                    let search = self
                        .search
                        .as_mut()
                        .expect("later search page has first page");
                    let known: HashSet<&str> =
                        search.rows.iter().map(|row| row.id.as_str()).collect();
                    let new: Vec<_> = page
                        .conversations
                        .into_iter()
                        .filter(|row| !known.contains(row.id.as_str()))
                        .collect();
                    search.rows.extend(new);
                    search.next_page = request.page.saturating_add(1);
                    search.has_more = has_more;
                    self.recompute_visible();
                } else {
                    self.search = Some(SearchState {
                        query: request.query.clone(),
                        rows: page.conversations,
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
                self.status = ListStatus::Failed(error);
                Some(error)
            }
        }
    }

    /// Goes back to the folder listing, leaving any results behind.
    pub fn clear_search(&mut self) {
        self.search = None;
        if self.search_request.is_some() {
            self.status = ListStatus::Loaded;
        }
        self.search_request = None;
        if matches!(self.status, ListStatus::Failed(_)) {
            self.status = ListStatus::Loaded;
        }
        self.recompute_visible();
        self.close_reader_if_hidden();
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
        // Emptying the field is how someone leaves the results behind.
        if self.search_query.trim().is_empty() {
            self.clear_search();
        } else {
            self.recompute_visible();
            self.close_reader_if_hidden();
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
            Some(search) => search.rows.iter().any(|row| row.id == row_id),
            None => self.conversations.iter().any(|row| row.id == row_id),
        }
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

    fn row(&self, row_id: &str) -> Option<&ConversationSummary> {
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
        self.conversations.iter().any(|row| row.id == row_id)
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

    pub fn action_pending(&self) -> bool {
        self.action_request.is_some()
    }

    pub fn action_error(&self) -> Option<MailboxError> {
        self.action_error
    }

    pub fn is_busy(&self) -> bool {
        matches!(
            self.status,
            ListStatus::Loading(_) | ListStatus::Refreshing(_) | ListStatus::LoadingMore(_)
        )
    }

    pub(super) fn auto_refresh_available(&self) -> bool {
        !self.is_busy() && self.action_request.is_none() && self.pending_reads.is_empty()
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

        Some(self.open_conversation(conversation_id, stamp.kind, request))
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
        let kind = self.row(&conversation_id)?.kind;

        Some(self.open_conversation(conversation_id, kind, request))
    }

    /// Sets the reader's conversation and active request.
    fn open_conversation(
        &mut self,
        conversation_id: String,
        kind: SummaryKind,
        request: RequestId,
    ) -> ReaderRequest {
        let request = ReaderRequest {
            id: request,
            conversation_id,
            kind,
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
        let accepted = matches!(
            &self.reader,
            ReaderState::Loading {
                conversation_id,
                request: current,
            } if conversation_id == &request.conversation_id && current == &request.id
        );
        if !accepted {
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

    fn set_reader(&mut self, reader: ReaderState) {
        let previous = std::mem::replace(&mut self.reader, reader);
        let stamp = self.reader_stamp.take();
        if let (ReaderState::Loaded(reader), Some(stamp)) = (previous, stamp) {
            self.cache_reader(reader, stamp);
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
        if !row.unread || self.pending_reads.contains_key(&row.id) {
            return None;
        }
        let (row_id, kind) = (row.id.clone(), row.kind);

        let request = ActionRequest {
            id: request,
            context: self.listed_in_folder(&row_id).then(|| self.folder.clone()),
            row_id,
            kind,
            folder: self.folder.clone(),
            action: MailAction::SetUnread(false),
        };
        self.pending_reads
            .insert(request.row_id.clone(), request.id);
        Some(request)
    }

    /// Applies a confirmed automatic read. Returns whether the row changed.
    /// Stale or failed responses leave it unread, so "Mark read" still works.
    pub fn finish_mark_read(
        &mut self,
        request: &ActionRequest,
        result: Result<(), MailboxError>,
    ) -> Result<bool, MailboxError> {
        if self.pending_reads.get(&request.row_id) != Some(&request.id) {
            return Ok(false);
        }
        self.pending_reads.remove(&request.row_id);
        result?;
        self.invalidate_listings();
        if request.folder != self.folder {
            return Ok(false);
        }

        let mut found = false;
        for row in self.row_copies(&request.row_id) {
            row.unread = false;
            found = true;
        }

        Ok(found)
    }

    /// The move the user can still take back, if any.
    pub fn undo(&self) -> Option<&UndoMove> {
        self.undo.as_ref()
    }

    /// Replaces the undo offer for moves out of a known physical folder.
    /// Label, origin and global-search views cannot identify that folder.
    pub fn offer_undo(&mut self, row_id: &str, kind: SummaryKind, action: MailAction) {
        let from_here = self.folder.is_location() && self.listed_in_folder(row_id);

        self.undo = match action.destination() {
            Some(to) if from_here && *to != self.folder => Some(UndoMove {
                row_id: row_id.to_owned(),
                kind,
                from: self.folder.clone(),
                to: to.clone(),
            }),
            _ => None,
        };
    }

    /// Claims the offer, so it is made only once.
    pub fn take_undo(&mut self) -> Option<UndoMove> {
        self.undo.take()
    }

    /// Dismisses only the offer whose timer expired.
    pub fn dismiss_undo(&mut self, offer: &UndoMove) {
        if self.undo.as_ref() == Some(offer) {
            self.undo = None;
        }
    }

    /// Starts an action on the selected row. Only one action runs at a time,
    /// and none while the list itself is loading.
    pub fn start_action(
        &mut self,
        action: MailAction,
        request: RequestId,
    ) -> Option<ActionRequest> {
        let (row_id, kind) = self
            .selected_summary()
            .map(|summary| (summary.id.clone(), summary.kind))?;

        self.start_action_on(row_id, kind, action, request)
    }

    /// Starts an action on a row by name, for rows the list no longer shows,
    /// such as one being moved back where it came from.
    pub fn start_action_on(
        &mut self,
        row_id: String,
        kind: SummaryKind,
        action: MailAction,
        request: RequestId,
    ) -> Option<ActionRequest> {
        if self.is_busy() || self.action_request.is_some() {
            return None;
        }

        let request = ActionRequest {
            id: request,
            context: self.listed_in_folder(&row_id).then(|| self.folder.clone()),
            row_id,
            kind,
            folder: self.folder.clone(),
            action,
        };
        self.action_error = None;
        // Only the last move can be taken back, and only until the next one.
        self.undo = None;
        // An explicit read or unread wins over an automatic read in flight.
        if matches!(request.action, MailAction::SetUnread(_)) {
            self.pending_reads.remove(&request.row_id);
        }
        self.action_request = Some(request.clone());
        Some(request)
    }

    /// Applies a current action response and returns the next row or error.
    pub fn finish_action(
        &mut self,
        request: &ActionRequest,
        result: Result<(), MailboxError>,
    ) -> Result<Option<String>, MailboxError> {
        if self.action_request.as_ref() != Some(request) {
            return Ok(None);
        }
        self.action_request = None;
        if let Err(error) = result {
            self.action_error = Some(error);
            return Err(error);
        }
        self.invalidate_listings();
        // After a folder switch these rows no longer contain the acted-on row.
        if request.folder != self.folder {
            return Ok(None);
        }

        if self.leaves_view(&request.folder, &request.action) {
            self.offer_undo(&request.row_id, request.kind, request.action.clone());

            return Ok(self.remove_row(&request.row_id));
        }

        self.record_action(&request.row_id, &request.action);
        // The reader shows which labels the open conversation carries, and it
        // is the row that was just acted on.
        if let (MailAction::SetLabel { label, on }, ReaderState::Loaded(reader)) =
            (&request.action, &mut self.reader)
            && reader.conversation_id() == request.row_id
        {
            reader.set_label(label, *on);
        }
        Ok(None)
    }

    /// Whether this action removes the row from the current view.
    fn leaves_view(&self, folder: &Folder, action: &MailAction) -> bool {
        match action {
            MailAction::SetStarred(starred) => {
                !starred && folder.system() == Some(MailFolder::Starred)
            }
            MailAction::SetLabel { label, on } => !on && label == folder,
            MailAction::SetUnread(_) => false,
            moved => moved.destination() != Some(folder),
        }
    }

    /// Applies a confirmed change to every loaded copy of the row.
    pub fn record_action(&mut self, row_id: &str, action: &MailAction) {
        for row in self.row_copies(row_id) {
            match action {
                MailAction::SetUnread(unread) => row.unread = *unread,
                MailAction::SetStarred(starred) => row.starred = *starred,
                MailAction::MoveTo(_) | MailAction::SetLabel { .. } => {}
            }
        }
        if self.leaves_view(&self.folder, action)
            && let Some(search) = &mut self.search
        {
            search.rows.retain(|row| row.id != row_id);
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
        if let Some(search) = &mut self.search {
            search.rows.retain(|row| row.id != row_id);
        }
        self.recompute_visible();
        if !was_selected {
            return None;
        }

        self.open_row_after(index?)
    }

    /// Closes the reader and names the visible row that took the place of the
    /// one that was at `index`, so reading continues where it left off.
    fn open_row_after(&mut self, index: usize) -> Option<String> {
        self.discard_reader();
        let last = self.visible_cache.len().checked_sub(1)?;

        self.visible_row(index.min(last)).map(|row| row.id.clone())
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
        // So do search results: a folder is a different question.
        self.search = None;
        self.search_request = None;

        if let Some(cached) = self.cached_listings.remove(&FolderKey::of(&self.folder)) {
            self.conversations = cached.conversations;
            self.next_page = cached.next_page;
            self.has_more = cached.has_more;
            self.active_dirty = cached.dirty;
            self.recompute_visible();
            if cached.dirty {
                // From this point on only a change made while this request is
                // running can make its answer stale again.
                self.active_dirty = false;
                self.status = ListStatus::Refreshing(request);
                Some(self.page_request(request, 0))
            } else {
                self.status = ListStatus::Loaded;
                None
            }
        } else {
            self.conversations.clear();
            self.next_page = 0;
            self.has_more = false;
            self.active_dirty = false;
            self.recompute_visible();
            self.status = ListStatus::Loading(request);
            Some(self.page_request(request, 0))
        }
    }

    /// Stores only a real folder listing. An initial load has `next_page == 0`;
    /// search results, reader state and transient actions deliberately stay out.
    fn cache_active_listing(&mut self) {
        if self.next_page == 0 {
            return;
        }
        let dirty = self.active_dirty || matches!(self.status, ListStatus::Refreshing(_));
        self.cached_listings.insert(
            FolderKey::of(&self.folder),
            CachedListing {
                conversations: std::mem::take(&mut self.conversations),
                next_page: self.next_page,
                has_more: self.has_more,
                dirty,
            },
        );
    }

    /// Marks cached folders stale after a confirmed server mutation.
    pub(super) fn invalidate_listings(&mut self) {
        self.active_dirty = true;
        self.invalidate_cached_listings();
    }

    /// A periodic refresh updates the open folder now. Everything else stays
    /// instant to open, but must revalidate before becoming a clean cache hit.
    pub(super) fn invalidate_cached_listings(&mut self) {
        for cached in self.cached_listings.values_mut() {
            cached.dirty = true;
        }
    }

    pub fn refresh(&mut self, request: RequestId) -> Option<PageRequest> {
        if self.is_busy() {
            return None;
        }
        // The reloaded list no longer shows what the offer talked about.
        self.undo = None;
        // If a mutation completes after this request starts it sets this back
        // to true, and the app follows this response with one fresh request.
        self.active_dirty = false;

        self.status = if self.conversations.is_empty() {
            ListStatus::Loading(request)
        } else {
            ListStatus::Refreshing(request)
        };
        Some(self.page_request(request, 0))
    }

    pub fn load_more(&mut self, request: RequestId) -> Option<PageRequest> {
        if self.is_busy() || self.search.is_some() || !self.has_more {
            return None;
        }

        self.status = ListStatus::LoadingMore(request);
        Some(self.page_request(request, self.next_page))
    }

    /// Starts a counts refresh, superseding any older request.
    pub fn refresh_counts(&mut self, request: RequestId) -> Option<RequestId> {
        self.counts_request = Some(request);
        Some(request)
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
        self.counts = Some(counts);
        self.counts_request = None;
        self.status = ListStatus::Loaded;
        self.has_more = self.conversations.len() < total as usize;
        // The demo snapshot is authoritative for the active folder. Other
        // folders may have changed as a side effect and must be revalidated.
        for cached in self.cached_listings.values_mut() {
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
                self.status = ListStatus::Failed(error);
                Some(error)
            }
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

    /// Returns loaded rows older than the refreshed first page.
    fn rows_below(&mut self, fresh: &[ConversationSummary]) -> Vec<ConversationSummary> {
        let Some(oldest) = fresh.iter().filter_map(|row| row.time).min() else {
            return Vec::new();
        };
        let covered: HashSet<&str> = fresh.iter().map(|row| row.id.as_str()).collect();

        self.conversations
            .drain(..)
            .filter(|row| {
                row.time.is_some_and(|time| time < oldest) && !covered.contains(row.id.as_str())
            })
            .collect()
    }

    fn page_request(&self, id: RequestId, page: u32) -> PageRequest {
        PageRequest {
            id,
            folder: self.folder.clone(),
            page,
            page_size: self.page_size,
        }
    }

    fn apply_page(&mut self, page: ConversationPage, append: bool) {
        let received = page.conversations.len();

        if append {
            let known: HashSet<&str> = self.conversations.iter().map(|c| c.id.as_str()).collect();
            let new: Vec<_> = page
                .conversations
                .into_iter()
                .filter(|conversation| !known.contains(conversation.id.as_str()))
                .collect();
            self.conversations.extend(new);
            self.next_page += 1;
        } else {
            // Preserve deeper pages unless a short first page proves they are gone.
            let deeper = if received >= self.page_size as usize {
                self.rows_below(&page.conversations)
            } else {
                Vec::new()
            };
            self.conversations = page.conversations;
            self.conversations.extend(deeper);
            self.next_page = self.next_page.max(1);
        }
        // Rows split out of a conversation carry their own, older times.
        sort_newest_first(&mut self.conversations);
        self.recompute_visible();
        if !append {
            // Search results are not part of this answer and stay on screen,
            // so what the reader holds is only closed when it truly left.
            self.close_reader_if_hidden();
        }

        self.has_more = received > 0
            && u64::from(self.next_page) * u64::from(self.page_size) < u64::from(page.total);
    }
}

/// Sorts newest first while preserving undated rows' relative positions.
fn sort_newest_first(rows: &mut Vec<ConversationSummary>) {
    // Borrow a neighboring time so stable sorting does not move undated rows.
    let mut previous = rows.iter().find_map(|row| row.time).unwrap_or(i64::MAX);
    let mut keyed: Vec<_> = rows
        .drain(..)
        .map(|row| {
            previous = row.time.unwrap_or(previous);
            (previous, row)
        })
        .collect();
    keyed.sort_by_key(|(time, _)| std::cmp::Reverse(*time));
    rows.extend(keyed.into_iter().map(|(_, row)| row));
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

#[cfg(test)]
mod tests;
