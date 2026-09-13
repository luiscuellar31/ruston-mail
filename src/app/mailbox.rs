use std::collections::{HashMap, HashSet};

use super::reader::{ConversationReader, ReaderState};
use crate::mail::{
    ConversationDetail, ConversationPage, ConversationSummary, Folder, MailAction, MailFolder,
    MailboxCounts, MailboxError, SummaryKind,
};

/// Identifies an asynchronous mailbox request. Only the response matching the
/// request currently in flight is applied; anything else is stale.
pub type RequestId = u64;

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
}

/// A search sent to the backend, remembered so only its own answer is applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRequest {
    pub id: RequestId,
    pub query: String,
}

pub struct Mailbox {
    folder: Folder,
    status: ListStatus,
    conversations: Vec<ConversationSummary>,
    page_size: u32,
    next_page: u32,
    has_more: bool,
    counts: Option<MailboxCounts>,
    counts_request: Option<RequestId>,
    reader: ReaderState,
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
            page_size,
            next_page: 0,
            has_more: false,
            counts: None,
            counts_request: Some(counts_request),
            reader: ReaderState::Empty,
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

    /// The rows on screen: search results when a search is showing, otherwise
    /// the loaded folder narrowed by what is typed in the search field.
    pub fn visible_conversations(&self) -> impl Iterator<Item = &ConversationSummary> {
        let (rows, query) = match &self.search {
            // Results already match, so nothing is filtered out of them.
            Some(search) => (&search.rows, String::new()),
            None => (&self.conversations, self.search_query.trim().to_lowercase()),
        };

        rows.iter()
            .filter(move |conversation| matches_search(conversation, &query))
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

    /// Sends the typed query to the backend, which looks past the loaded rows
    /// and past the open folder. Returns `None` for an empty query, which
    /// simply leaves the results behind.
    pub fn start_search(&mut self, request: RequestId) -> Option<SearchRequest> {
        let query = self.search_query.trim().to_owned();
        if query.is_empty() {
            self.clear_search();
            return None;
        }

        self.search_request = Some(request);
        self.status = ListStatus::Loading(request);

        Some(SearchRequest { id: request, query })
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
                self.search = Some(SearchState {
                    query: request.query.clone(),
                    rows: page.conversations,
                });
                self.status = ListStatus::Loaded;
                self.close_reader_if_hidden();
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
        self.search_request = None;
        if matches!(self.status, ListStatus::Failed(_)) {
            self.status = ListStatus::Loaded;
        }
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
        }
        let selected_is_hidden = self.selected_conversation().is_some_and(|selected| {
            !self
                .visible_conversations()
                .any(|conversation| conversation.id == selected)
        });
        if selected_is_hidden {
            self.set_reader(ReaderState::Empty);
        }
    }

    pub fn counts(&self) -> Option<&MailboxCounts> {
        self.counts.as_ref()
    }

    /// Whether the list can be extended. A search answers in one batch, so
    /// while its results are what is on screen there is no next page to ask
    /// for, however many the open folder still has waiting.
    pub fn has_more(&self) -> bool {
        self.search.is_none() && self.has_more
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
        let rows: Vec<&str> = self
            .visible_conversations()
            .map(|row| row.id.as_str())
            .collect();
        let current = self
            .selected_conversation()
            .and_then(|selected| rows.iter().position(|row| *row == selected));

        let index = match (current, step) {
            (None, _) => 0,
            (Some(index), Step::Next) => index + 1,
            (Some(0), Step::Previous) => return None,
            (Some(index), Step::Previous) => index - 1,
        };

        rows.get(index).map(|id| (*id).to_owned())
    }

    /// Closes the reader without touching the list, as Escape does.
    pub fn close_reader(&mut self) {
        self.set_reader(ReaderState::Empty);
    }

    /// Where a row sits among the visible ones, as a fraction from 0 to 1.
    /// `None` when it is not visible or is the only row.
    pub fn visible_position(&self, row_id: &str) -> Option<f32> {
        let index = self
            .visible_conversations()
            .position(|row| row.id == row_id)?;
        let last = self.visible_conversations().count().checked_sub(1)?;

        (last > 0).then(|| index as f32 / last as f32)
    }

    pub fn selected_conversation(&self) -> Option<&str> {
        self.reader.conversation_id()
    }

    pub fn selected_summary(&self) -> Option<&ConversationSummary> {
        let selected = self.selected_conversation()?;

        self.row(selected)
    }

    /// Every row the user can act on. Search results are rows too: they are
    /// what is on screen while a search is showing, and a conversation found
    /// that way is usually nowhere in the folder listing.
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

    /// Starts loading a visible conversation. Reselecting the current
    /// conversation keeps its loaded or in-flight state, except after a
    /// failure: clicking the row again is a retry.
    pub fn start_conversation_load(
        &mut self,
        conversation_id: String,
        request: RequestId,
    ) -> Option<ReaderRequest> {
        let reselecting = self.selected_conversation() == Some(conversation_id.as_str());
        if reselecting && !matches!(self.reader, ReaderState::Failed { .. }) {
            return None;
        }
        let kind = self
            .visible_conversations()
            .find(|conversation| conversation.id == conversation_id)?
            .kind;
        self.action_error = None;

        Some(self.open_conversation(conversation_id, kind, request))
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

    /// Puts the reader on a conversation and names the request that will fill
    /// it. Both opening and retrying land here, so the loading state is set in
    /// one place.
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
                self.set_reader(ReaderState::Loaded(ConversationReader::new(detail)));
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
        self.reader = reader;
    }

    pub fn toggle_message(&mut self, message_id: &str) {
        if let ReaderState::Loaded(reader) = &mut self.reader {
            reader.toggle(message_id);
        }
    }

    /// Shows or hides the labels the open conversation does not carry.
    pub fn toggle_labels(&mut self) {
        if let ReaderState::Loaded(reader) = &mut self.reader {
            reader.toggle_labels();
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

    /// Remembers a move so it can be taken back, replacing any earlier offer.
    /// Only moves out of a real folder qualify: Starred is a label, and Sent
    /// and Drafts describe where mail came from, so there is nowhere to put it
    /// back. Nor does a row only a search found: it was listed from every
    /// folder at once, so the open one is not where it came from and putting
    /// it there would be a move of its own. Both backends record the offer
    /// here, since demo actions never reach `finish_action`.
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

    /// Applies an action once the backend confirmed it. Returns the row to
    /// open next when the selected one left the folder, or the error of an
    /// accepted failed response. Stale responses change nothing.
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

    /// Whether an action started in `folder` takes the row out of what the
    /// view lists, rather than only changing how the row reads. Unstarring
    /// inside Starred and taking a label away inside that label's own view
    /// are the two ways something other than a move empties a row out.
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

    /// Writes a confirmed change into every listing that holds the row: the
    /// fields it changes, and the row itself where the change takes it out of
    /// the view. Proton's path removes such a row before reaching here; the
    /// demo has no server to re-read and reloads only the open folder, so the
    /// search results, which span every folder, are left to this.
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
        if !was_selected {
            return None;
        }

        self.open_row_after(index?)
    }

    /// Closes the reader and names the visible row that took the place of the
    /// one that was at `index`, so reading continues where it left off.
    fn open_row_after(&mut self, index: usize) -> Option<String> {
        self.set_reader(ReaderState::Empty);
        let last = self.visible_conversations().count().checked_sub(1)?;

        self.visible_conversations()
            .nth(index.min(last))
            .map(|row| row.id.clone())
    }

    /// Takes up the account's current name for the open place. Proton knows
    /// it by its id, so the listing is the same one either way: nothing is
    /// reloaded, and what is open stays open.
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

        self.folder = folder;
        self.conversations.clear();
        self.next_page = 0;
        self.has_more = false;
        self.set_reader(ReaderState::Empty);
        self.action_error = None;
        // The offer belongs to the folder it was made in.
        self.undo = None;
        // So do search results: a folder is a different question.
        self.search = None;
        self.search_request = None;
        self.status = ListStatus::Loading(request);
        Some(self.page_request(request, 0))
    }

    pub fn refresh(&mut self, request: RequestId) -> Option<PageRequest> {
        if self.is_busy() {
            return None;
        }
        // The reloaded list no longer shows what the offer talked about.
        self.undo = None;

        self.status = if self.conversations.is_empty() {
            ListStatus::Loading(request)
        } else {
            ListStatus::Refreshing(request)
        };
        Some(self.page_request(request, 0))
    }

    pub fn load_more(&mut self, request: RequestId) -> Option<PageRequest> {
        if self.is_busy() || !self.has_more() {
            return None;
        }

        self.status = ListStatus::LoadingMore(request);
        Some(self.page_request(request, self.next_page))
    }

    /// Starts a counts refresh. A newer request supersedes one still in
    /// flight, so a slow or lost response can never block later refreshes;
    /// `finish_counts` then ignores everything but the newest.
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

    /// Takes the loaded rows that sit below `fresh`, that is, older than
    /// everything it contains. Rows the fresh page covers are dropped, since
    /// it is the newer word on them.
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
            // A reload speaks only for the first page, so deeper pages the
            // user already loaded are kept and refreshing never shortens the
            // list. A short page is the exception: then the folder itself has
            // no more rows, and everything below it is gone.
            let deeper = if received >= self.page_size as usize {
                self.rows_below(&page.conversations)
            } else {
                Vec::new()
            };
            self.conversations = page.conversations;
            self.conversations.extend(deeper);
            self.next_page = self.next_page.max(1);

            // Search results are not part of this answer and stay on screen,
            // so what the reader holds is only closed when it truly left.
            self.close_reader_if_hidden();
        }
        // Rows split out of a conversation carry their own, older times.
        sort_newest_first(&mut self.conversations);

        self.has_more = received > 0
            && u64::from(self.next_page) * u64::from(self.page_size) < u64::from(page.total);
    }
}

/// Newest first, stable, so rows already in order never move. A row without
/// a time stays right after the row that preceded it, since only the server
/// knows where it belongs.
fn sort_newest_first(rows: &mut Vec<ConversationSummary>) {
    // A row with no time of its own takes the time of the last dated row
    // above it, or of the first dated row when it leads the list. Sorting is
    // stable, so it keeps its place beside the row it borrowed from instead
    // of floating to the top of the mailbox.
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
///
/// Every visible row is asked this on every redraw, so the ASCII that most
/// mail is written in answers without building a lowercased copy of each
/// field. Anything else falls back to real lowercasing, which is the only
/// way to match text where case is not a matter of one byte.
fn contains_ignoring_case(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    if haystack.is_ascii() {
        // Every character is one byte here, so bytes and characters line up.
        return haystack
            .as_bytes()
            .windows(needle.len())
            .any(|window| window.eq_ignore_ascii_case(needle.as_bytes()));
    }

    haystack.to_lowercase().contains(needle)
}

#[cfg(test)]
mod tests;
