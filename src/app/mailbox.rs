use std::collections::{HashMap, HashSet};

use iced::widget::text_editor;

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
    /// Bodies shown as selectable text, by message. Iced 0.14 cannot select
    /// the text it renders, so a selectable body lives in an editor buffer.
    selectable: HashMap<String, text_editor::Content>,
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
            selectable: HashMap::new(),
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

    /// The only place the reader changes. Selectable bodies belong to the open
    /// conversation, so they are rebuilt here and nowhere else: a plain body
    /// loses nothing inside an editor and arrives selectable, while HTML ones
    /// wait until the user asks for them.
    fn set_reader(&mut self, reader: ReaderState) {
        self.selectable = match &reader {
            ReaderState::Loaded(loaded) => loaded
                .detail()
                .messages
                .iter()
                .filter(|message| message.body.is_plain())
                .map(|message| {
                    (
                        message.id.clone(),
                        text_editor::Content::with_text(&message.body.plain_text()),
                    )
                })
                .collect(),
            _ => HashMap::new(),
        };
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

    /// The editor buffer of a body shown as selectable text, if it has one.
    pub fn selectable_body(&self, message_id: &str) -> Option<&text_editor::Content> {
        self.selectable.get(message_id)
    }

    /// Shows one message as selectable text, or goes back to the formatted
    /// body. Plain bodies are already selectable, so this is meant for HTML
    /// ones, where selecting trades away the formatting.
    pub fn toggle_selection(&mut self, message_id: &str) {
        if self.selectable.remove(message_id).is_some() {
            return;
        }
        let ReaderState::Loaded(reader) = &self.reader else {
            return;
        };
        let Some(message) = reader
            .detail()
            .messages
            .iter()
            .find(|message| message.id == message_id)
        else {
            return;
        };

        self.selectable.insert(
            message.id.clone(),
            text_editor::Content::with_text(&message.body.plain_text()),
        );
    }

    /// Applies a reader interaction to one selectable body. Edits are dropped:
    /// the reader shows mail, it never changes it.
    pub fn select_text(&mut self, message_id: &str, action: text_editor::Action) {
        if action.is_edit() {
            return;
        }
        if let Some(content) = self.selectable.get_mut(message_id) {
            content.perform(action);
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
mod tests {
    use iced::widget::text_editor::{Action, Edit};

    use super::*;
    use crate::mail::{MailAddress, MailMessage, MessageBody, RichBody};

    const PAGE_SIZE: u32 = 50;

    /// One of Proton's own folders, as a place to list from.
    fn sys(folder: MailFolder) -> Folder {
        Folder::System(folder)
    }

    fn summary(id: &str) -> ConversationSummary {
        ConversationSummary {
            id: id.to_owned(),
            kind: SummaryKind::Conversation,
            subject: None,
            correspondents: None,
            participants: Vec::new(),
            preview: None,
            time: None,
            unread: false,
            starred: false,
            message_count: 1,
            has_attachments: false,
        }
    }

    fn searchable_summary(
        id: &str,
        name: &str,
        address: &str,
        subject: &str,
        preview: &str,
    ) -> ConversationSummary {
        ConversationSummary {
            id: id.to_owned(),
            kind: SummaryKind::Conversation,
            subject: Some(subject.to_owned()),
            correspondents: Some(name.to_owned()),
            participants: vec![MailAddress {
                name: Some(name.to_owned()),
                address: address.to_owned(),
            }],
            preview: Some(preview.to_owned()),
            time: None,
            unread: false,
            starred: false,
            message_count: 1,
            has_attachments: false,
        }
    }

    fn page(ids: &[&str], total: u32) -> Result<ConversationPage, MailboxError> {
        Ok(ConversationPage {
            conversations: ids.iter().map(|id| summary(id)).collect(),
            total,
        })
    }

    fn detail(id: &str, message_ids: &[&str]) -> ConversationDetail {
        ConversationDetail {
            id: id.to_owned(),
            subject: None,
            labels: Vec::new(),
            messages: message_ids
                .iter()
                .enumerate()
                .map(|(time, message_id)| MailMessage {
                    id: (*message_id).to_owned(),
                    sender: MailAddress::default(),
                    recipients: Vec::new(),
                    time: Some(time as i64),
                    body: MessageBody::PlainText(String::new()),
                    attachments: Vec::new(),
                })
                .collect(),
        }
    }

    fn ids(mailbox: &Mailbox) -> Vec<&str> {
        mailbox
            .conversations()
            .iter()
            .map(|c| c.id.as_str())
            .collect()
    }

    fn open() -> (Mailbox, PageRequest) {
        Mailbox::open(Folder::INBOX, PAGE_SIZE, 1, 2)
    }

    fn loaded_inbox(ids: &[&str], total: u32) -> Mailbox {
        let (mut mailbox, request) = open();
        mailbox.finish_page(request.id, page(ids, total));
        mailbox
    }

    fn load_detail(
        mailbox: &mut Mailbox,
        conversation_id: &str,
        conversation: ConversationDetail,
        request: RequestId,
    ) {
        let request = mailbox
            .start_conversation_load(conversation_id.into(), request)
            .unwrap();
        assert_eq!(
            mailbox.finish_conversation(&request, Ok(conversation)),
            None
        );
    }

    fn searchable_mailbox() -> Mailbox {
        let (mut mailbox, request) = open();
        let conversations = vec![
            searchable_summary(
                "sender",
                "Alice Stone",
                "alice@example.com",
                "Quarterly roadmap",
                "Budget approval is ready",
            ),
            searchable_summary(
                "subject",
                "Bob Chen",
                "bob@example.com",
                "Rust project plan",
                "Milestones for next month",
            ),
            searchable_summary(
                "preview",
                "Carol Diaz",
                "carol@example.com",
                "Release notes",
                "Launch checklist is complete",
            ),
        ];
        mailbox.finish_page(
            request.id,
            Ok(ConversationPage {
                total: conversations.len() as u32,
                conversations,
            }),
        );
        mailbox
    }

    fn visible_ids(mailbox: &Mailbox) -> Vec<&str> {
        mailbox
            .visible_conversations()
            .map(|conversation| conversation.id.as_str())
            .collect()
    }

    #[test]
    fn plain_bodies_arrive_selectable_and_stay_read_only() {
        let mut mailbox = loaded_inbox(&["a", "b"], 2);
        let mut conversation = detail("a", &["a1", "a2"]);
        conversation.messages[0].body = MessageBody::PlainText("Hello Alex".into());
        load_detail(&mut mailbox, "a", conversation, 3);

        let content = mailbox.selectable_body("a1").unwrap();
        assert_eq!(content.text().trim_end(), "Hello Alex");

        // Selecting and moving are allowed; edits never reach the buffer.
        mailbox.select_text("a1", Action::SelectAll);
        mailbox.select_text("a1", Action::Edit(Edit::Insert('x')));
        mailbox.select_text("a1", Action::Edit(Edit::Backspace));
        mailbox.select_text("a1", Action::Edit(Edit::Paste("spam".to_owned().into())));
        assert_eq!(
            mailbox.selectable_body("a1").unwrap().text().trim_end(),
            "Hello Alex"
        );

        // Bodies switch one at a time, and an unknown one is ignored.
        mailbox.toggle_selection("a1");
        assert!(mailbox.selectable_body("a1").is_none());
        assert!(mailbox.selectable_body("a2").is_some());
        mailbox.toggle_selection("missing");
        assert!(mailbox.selectable_body("missing").is_none());
    }

    #[test]
    fn a_formatted_body_switches_to_selectable_text() {
        let mut mailbox = loaded_inbox(&["a"], 1);
        let mut conversation = detail("a", &["a1", "a2"]);
        conversation.messages[1].body = MessageBody::Rich(RichBody::default());
        load_detail(&mut mailbox, "a", conversation, 2);

        // The plain body is selectable already; the formatted one is not.
        assert!(mailbox.selectable_body("a1").is_some());
        assert!(mailbox.selectable_body("a2").is_none());

        mailbox.toggle_selection("a2");
        assert!(mailbox.selectable_body("a2").is_some());

        mailbox.toggle_selection("a2");
        assert!(mailbox.selectable_body("a2").is_none());
        // The plain body never left.
        assert!(mailbox.selectable_body("a1").is_some());
    }

    #[test]
    fn opening_another_conversation_drops_selectable_bodies() {
        let mut mailbox = loaded_inbox(&["a", "b"], 2);
        load_detail(&mut mailbox, "a", detail("a", &["a1"]), 3);
        assert!(mailbox.selectable_body("a1").is_some());

        load_detail(&mut mailbox, "b", detail("b", &["b1"]), 4);

        assert!(mailbox.selectable_body("a1").is_none());
        assert!(mailbox.selectable_body("b1").is_some());
    }

    fn dated(id: &str, time: i64) -> ConversationSummary {
        ConversationSummary {
            time: Some(time),
            ..summary(id)
        }
    }

    fn dated_page(rows: &[(&str, i64)], total: u32) -> Result<ConversationPage, MailboxError> {
        Ok(ConversationPage {
            conversations: rows.iter().map(|(id, time)| dated(id, *time)).collect(),
            total,
        })
    }

    /// A full page of dated rows named `p<n>`, newest first.
    fn full_page(start: u32, total: u32) -> Result<ConversationPage, MailboxError> {
        let conversations = (0..PAGE_SIZE)
            .map(|index| {
                let number = start + index;
                dated(&format!("p{number}"), i64::from(1_000_000 - number))
            })
            .collect();

        Ok(ConversationPage {
            conversations,
            total,
        })
    }

    #[test]
    fn server_results_replace_the_folder_listing() {
        let mut mailbox = loaded_inbox(&["a", "b"], 120);
        mailbox.set_search_query("invoice".into());

        let request = mailbox.start_search(3).expect("a query reaches the server");
        assert_eq!(request.query, "invoice");

        // Results stand on their own: what came back is not narrowed again.
        assert_eq!(mailbox.finish_search(&request, page(&["found"], 1)), None);
        assert_eq!(visible_ids(&mailbox), ["found"]);
        assert_eq!(mailbox.search_results(), Some("invoice"));

        // The server answers in one batch, so there is no next page to ask
        // for, even though the folder behind the results has more.
        assert_eq!(mailbox.load_more(4), None);

        // Emptying the field puts the folder back.
        mailbox.set_search_query(String::new());
        assert_eq!(visible_ids(&mailbox), ["a", "b"]);
        assert_eq!(mailbox.search_results(), None);
    }

    #[test]
    fn an_abandoned_search_never_lands() {
        let mut mailbox = loaded_inbox(&["a"], 1);
        mailbox.set_search_query("invoice".into());
        let abandoned = mailbox.start_search(3).unwrap();

        // The user gave up before the answer came back.
        mailbox.set_search_query(String::new());

        assert_eq!(mailbox.finish_search(&abandoned, page(&["found"], 1)), None);
        assert_eq!(visible_ids(&mailbox), ["a"]);
        assert!(mailbox.search_results().is_none());
    }

    #[test]
    fn an_empty_query_leaves_the_results_behind() {
        let mut mailbox = loaded_inbox(&["a"], 1);
        mailbox.set_search_query("invoice".into());
        let request = mailbox.start_search(3).unwrap();
        let _ = mailbox.finish_search(&request, page(&["found"], 1));

        mailbox.set_search_query("   ".into());

        assert!(mailbox.start_search(4).is_none());
        assert_eq!(visible_ids(&mailbox), ["a"]);
    }

    #[test]
    fn refreshing_drops_the_offer_to_undo() {
        let mut mailbox = loaded_inbox(&["a", "b"], 2);
        load_detail(&mut mailbox, "a", detail("a", &["a1"]), 3);
        let request = mailbox
            .start_action(MailAction::MoveTo(sys(MailFolder::Archive)), 4)
            .unwrap();
        let _ = mailbox.finish_action(&request, Ok(()));
        assert!(mailbox.undo().is_some());

        // The reloaded list no longer shows what the offer talked about.
        let _ = mailbox.refresh(5);

        assert!(mailbox.undo().is_none());
    }

    #[test]
    fn a_move_out_of_a_folder_can_be_taken_back() {
        let mut mailbox = loaded_inbox(&["a", "b"], 2);
        load_detail(&mut mailbox, "a", detail("a", &["a1"]), 3);
        let request = mailbox
            .start_action(MailAction::MoveTo(sys(MailFolder::Archive)), 4)
            .unwrap();
        assert!(mailbox.undo().is_none());

        assert_eq!(
            mailbox.finish_action(&request, Ok(())),
            Ok(Some("b".into()))
        );

        let undo = mailbox.undo().expect("a move out of the Inbox undoes");
        assert_eq!(undo.row_id, "a");
        assert_eq!(undo.from, sys(MailFolder::Inbox));
        assert_eq!(undo.to, sys(MailFolder::Archive));

        // The offer is claimed once, and the next action replaces it.
        assert!(mailbox.take_undo().is_some());
        assert!(mailbox.undo().is_none());
    }

    /// An Inbox holding `listed`, showing the results of a server search that
    /// returned `found`, with `opened` open in the reader.
    fn searched(listed: &[&str], found: &[&str], opened: &str) -> Mailbox {
        let (mut mailbox, request) = open();
        let rows: Vec<ConversationSummary> = listed
            .iter()
            .map(|id| ConversationSummary {
                unread: true,
                ..summary(id)
            })
            .collect();
        let total = rows.len() as u32;
        mailbox.finish_page(
            request.id,
            Ok(ConversationPage {
                conversations: rows,
                total,
            }),
        );

        mailbox.set_search_query("report".to_owned());
        let search = mailbox.start_search(90).unwrap();
        let results: Vec<ConversationSummary> = found
            .iter()
            .map(|id| ConversationSummary {
                unread: true,
                ..summary(id)
            })
            .collect();
        let total = results.len() as u32;
        assert_eq!(
            mailbox.finish_search(
                &search,
                Ok(ConversationPage {
                    conversations: results,
                    total,
                }),
            ),
            None
        );
        load_detail(&mut mailbox, opened, detail(opened, &["message"]), 91);

        mailbox
    }

    #[test]
    fn a_conversation_a_search_found_can_be_acted_on() {
        let mut mailbox = searched(&["a"], &["found"], "found");

        // The row is nowhere in the folder listing, so Proton is told to read
        // it across every folder rather than in the open one.
        let request = mailbox
            .start_action(MailAction::SetStarred(true), 3)
            .expect("a search result takes actions like any other row");
        assert_eq!(request.row_id, "found");
        assert_eq!(request.context, None);

        assert_eq!(mailbox.finish_action(&request, Ok(())), Ok(None));
        assert!(
            mailbox
                .selected_summary()
                .expect("the open row is still the search result")
                .starred
        );
    }

    #[test]
    fn a_search_result_is_marked_read_like_any_other_row() {
        let mut mailbox = searched(&["a"], &["found"], "found");

        let request = mailbox
            .start_mark_read(3)
            .expect("opening a search result marks it read");
        assert_eq!(request.context, None);

        assert_eq!(mailbox.finish_mark_read(&request, Ok(())), Ok(true));
        assert!(!mailbox.selected_summary().unwrap().unread);
    }

    #[test]
    fn a_row_in_both_listings_changes_in_both() {
        // The search found a row the open folder also lists.
        let mut mailbox = searched(&["a"], &["a"], "a");

        // It is listed in the folder, so the open folder is the context.
        let request = mailbox
            .start_action(MailAction::SetStarred(true), 3)
            .unwrap();
        assert_eq!(request.context.as_ref(), Some(&sys(MailFolder::Inbox)));
        assert_eq!(mailbox.finish_action(&request, Ok(())), Ok(None));

        // Both copies agree, so the folder listing holds no stale row waiting
        // for the search to be left behind.
        assert!(mailbox.selected_summary().unwrap().starred);
        assert!(mailbox.conversations().iter().all(|row| row.starred));
    }

    #[test]
    fn a_row_moved_out_leaves_the_search_results_too() {
        let mut mailbox = searched(&["a"], &["a", "b"], "a");

        let request = mailbox
            .start_action(MailAction::MoveTo(sys(MailFolder::Archive)), 3)
            .unwrap();

        // Reading goes on with the next result, and the moved row is gone
        // from the results and from the folder listing alike.
        assert_eq!(
            mailbox.finish_action(&request, Ok(())),
            Ok(Some("b".into()))
        );
        assert!(!mailbox.has_row("a"));
        assert!(mailbox.conversations().is_empty());
    }

    #[test]
    fn matching_ignores_case_with_or_without_ascii() {
        // The query arrives lowercased, as the mailbox trims and lowers it.
        assert!(contains_ignoring_case("Quarterly Roadmap", "roadmap"));
        assert!(contains_ignoring_case("ALICE@EXAMPLE.COM", "alice@"));
        assert!(!contains_ignoring_case("roadmap", "budget"));
        // A needle longer than what it is looked for in matches nothing.
        assert!(!contains_ignoring_case("hi", "hiring"));
        // An empty query narrows nothing, so everything matches it.
        assert!(contains_ignoring_case("", ""));

        // Text where case is more than one byte takes the slower path and
        // must still match.
        assert!(contains_ignoring_case("Ángel Ruíz", "ángel"));
        assert!(contains_ignoring_case("ÉCOLE", "école"));
        assert!(!contains_ignoring_case("Ángel", "ünsal"));
        // And an ASCII line cannot hold a word that is not ASCII.
        assert!(!contains_ignoring_case("Angel", "ángel"));
    }

    #[test]
    fn taking_up_a_new_name_keeps_the_list_and_the_reader() {
        let mut mailbox = loaded_inbox(&["a"], 1);
        let request = mailbox
            .select_folder(Folder::custom("kZ9", "Invoices"), 2)
            .unwrap();
        mailbox.finish_page(request.id, page(&["i1", "i2"], 2));
        load_detail(&mut mailbox, "i1", detail("i1", &["m1"]), 3);

        mailbox.rename_folder(Folder::custom("kZ9", "Facturas"));

        // Proton knows the place by its id, so the same mail is listed under
        // the new name and nothing had to be fetched again.
        assert_eq!(mailbox.folder().name(), "Facturas");
        assert_eq!(mailbox.conversations().len(), 2);
        assert_eq!(mailbox.selected_conversation(), Some("i1"));

        // Another place is not this one under a new name.
        mailbox.rename_folder(Folder::custom("wN2", "Recibos"));
        assert_eq!(mailbox.folder().name(), "Facturas");
        // Nor is one of Proton's own, which carries no id of its own.
        mailbox.rename_folder(sys(MailFolder::Archive));
        assert_eq!(mailbox.folder().name(), "Facturas");
    }

    #[test]
    fn results_on_screen_offer_no_next_page() {
        // A total past one page is what leaves a next page to ask for.
        let mut mailbox = loaded_inbox(&["a"], PAGE_SIZE * 3);
        assert!(mailbox.has_more(), "the folder has more pages waiting");

        mailbox.set_search_query("report".to_owned());
        let search = mailbox.start_search(3).unwrap();
        assert_eq!(mailbox.finish_search(&search, page(&["found"], 1)), None);

        // The server answered in one batch, so offering to load more would
        // offer something nothing can ask for.
        assert!(!mailbox.has_more());
        assert!(mailbox.load_more(4).is_none());

        // The folder's own pages are still there once the results are gone.
        mailbox.clear_search();
        assert!(mailbox.has_more());
    }

    #[test]
    fn moving_a_row_only_a_search_found_offers_no_undo() {
        let mut mailbox = searched(&["a"], &["found", "next"], "found");

        let request = mailbox
            .start_action(MailAction::MoveTo(sys(MailFolder::Archive)), 3)
            .unwrap();
        assert_eq!(
            mailbox.finish_action(&request, Ok(())),
            Ok(Some("next".into()))
        );

        // The search listed every folder at once, so the Inbox is not where
        // this row came from and putting it there would be a move of its own.
        assert!(mailbox.undo().is_none());
        assert!(!mailbox.has_row("found"));
    }

    #[test]
    fn refreshing_keeps_the_search_result_that_is_open() {
        let mut mailbox = searched(&["a"], &["found"], "found");

        // A reload answers for the folder, which the results are not part of.
        let refresh = mailbox.refresh(3).expect("a search does not block reload");
        assert_eq!(mailbox.finish_page(refresh.id, page(&["a"], 1)), None);

        assert_eq!(mailbox.selected_conversation(), Some("found"));
    }

    #[test]
    fn a_label_leaves_the_row_where_it_is() {
        let receipts = Folder::label("wN2", "Receipts");
        let mut mailbox = loaded_inbox(&["a", "b"], 2);
        load_detail(&mut mailbox, "a", detail("a", &["a1"]), 3);

        let request = mailbox
            .start_action(
                MailAction::SetLabel {
                    label: receipts.clone(),
                    on: true,
                },
                4,
            )
            .unwrap();
        assert_eq!(mailbox.finish_action(&request, Ok(())), Ok(None));

        // The row keeps its place in the Inbox, and the reader shows the
        // label without loading the conversation again.
        assert!(mailbox.has_row("a"));
        assert!(
            mailbox
                .reader()
                .expect("the conversation is still open")
                .detail()
                .carries(&receipts)
        );
        assert!(mailbox.undo().is_none());
    }

    #[test]
    fn taking_a_label_away_inside_its_own_view_empties_the_row_out() {
        let receipts = Folder::label("wN2", "Receipts");
        let mut mailbox = loaded_inbox(&["a"], 1);
        let page_request = mailbox.select_folder(receipts.clone(), 2).unwrap();
        mailbox.finish_page(page_request.id, page(&["r1", "r2"], 2));
        load_detail(&mut mailbox, "r1", detail("r1", &["m1"]), 3);

        let request = mailbox
            .start_action(
                MailAction::SetLabel {
                    label: receipts,
                    on: false,
                },
                4,
            )
            .unwrap();

        // Nothing in this view carries the label any more, so reading moves
        // on to the row that took its place.
        assert_eq!(
            mailbox.finish_action(&request, Ok(())),
            Ok(Some("r2".into()))
        );
        assert!(!mailbox.has_row("r1"));
    }

    #[test]
    fn flags_and_labels_offer_nothing_to_undo() {
        // A flag change moves nothing.
        let mut mailbox = loaded_inbox(&["a"], 1);
        load_detail(&mut mailbox, "a", detail("a", &["a1"]), 2);
        let request = mailbox
            .start_action(MailAction::SetStarred(true), 3)
            .unwrap();
        let _ = mailbox.finish_action(&request, Ok(()));
        assert!(mailbox.undo().is_none());

        // Starred is a label, so there is no folder to put the row back into.
        let page_request = mailbox.select_folder(sys(MailFolder::Starred), 4).unwrap();
        mailbox.finish_page(page_request.id, page(&["s1"], 1));
        load_detail(&mut mailbox, "s1", detail("s1", &["m1"]), 5);
        let request = mailbox
            .start_action(MailAction::MoveTo(sys(MailFolder::Archive)), 6)
            .unwrap();
        let _ = mailbox.finish_action(&request, Ok(()));

        assert!(mailbox.undo().is_none());

        // A label the account made behaves the same: the row keeps the label
        // and moves out of whatever folder it was really in, which this view
        // never knew.
        let page_request = mailbox
            .select_folder(Folder::label("wN2", "Receipts"), 7)
            .unwrap();
        mailbox.finish_page(page_request.id, page(&["r1"], 1));
        load_detail(&mut mailbox, "r1", detail("r1", &["m1"]), 8);
        let request = mailbox
            .start_action(MailAction::MoveTo(sys(MailFolder::Archive)), 9)
            .unwrap();
        let _ = mailbox.finish_action(&request, Ok(()));

        assert!(mailbox.undo().is_none());
    }

    #[test]
    fn the_keyboard_steps_through_visible_rows() {
        let mut mailbox = loaded_inbox(&["a", "b", "c"], 3);

        // Nothing open yet, so either direction starts at the top.
        assert_eq!(mailbox.neighbour(Step::Next).as_deref(), Some("a"));
        assert_eq!(mailbox.neighbour(Step::Previous).as_deref(), Some("a"));

        load_detail(&mut mailbox, "b", detail("b", &["b1"]), 4);
        assert_eq!(mailbox.neighbour(Step::Next).as_deref(), Some("c"));
        assert_eq!(mailbox.neighbour(Step::Previous).as_deref(), Some("a"));

        // The ends stop rather than wrap around.
        load_detail(&mut mailbox, "c", detail("c", &["c1"]), 5);
        assert_eq!(mailbox.neighbour(Step::Next), None);
        load_detail(&mut mailbox, "a", detail("a", &["a1"]), 6);
        assert_eq!(mailbox.neighbour(Step::Previous), None);
    }

    #[test]
    fn a_loaded_row_is_found_even_while_a_search_hides_it() {
        let mut mailbox = loaded_inbox(&["a", "b"], 2);

        assert!(mailbox.has_row("a"));
        assert!(!mailbox.has_row("missing"));

        // Hiding a row from view does not take it out of the loaded list, so
        // an action on it knows the row never left the folder.
        mailbox.set_search_query("nothing matches this".into());

        assert_eq!(mailbox.visible_position("a"), None);
        assert!(mailbox.has_row("a"));
    }

    #[test]
    fn visible_position_places_a_row_between_the_ends() {
        let mut mailbox = loaded_inbox(&["a", "b", "c"], 3);

        assert_eq!(mailbox.visible_position("a"), Some(0.0));
        assert_eq!(mailbox.visible_position("b"), Some(0.5));
        assert_eq!(mailbox.visible_position("c"), Some(1.0));
        assert_eq!(mailbox.visible_position("missing"), None);

        // Positions follow what is on screen, not what is loaded.
        mailbox.set_search_query("nothing matches this".into());
        assert_eq!(mailbox.visible_position("b"), None);

        // A single row has nowhere to scroll to.
        assert_eq!(loaded_inbox(&["only"], 1).visible_position("only"), None);
    }

    #[test]
    fn clicking_a_failed_conversation_again_retries_it() {
        let mut mailbox = loaded_inbox(&["a", "b"], 2);
        let request = mailbox.start_conversation_load("a".into(), 3).unwrap();
        assert_eq!(
            mailbox.finish_conversation(&request, Err(MailboxError::Connection)),
            Some(MailboxError::Connection)
        );

        // The failed row stays selected, so the click used to do nothing.
        assert!(mailbox.start_conversation_load("a".into(), 4).is_some());
    }

    #[test]
    fn refreshing_keeps_the_pages_already_loaded() {
        let (mut mailbox, request) = open();
        mailbox.finish_page(request.id, full_page(0, 120));
        let more = mailbox.load_more(5).unwrap();
        mailbox.finish_page(more.id, full_page(PAGE_SIZE, 120));
        assert_eq!(mailbox.conversations().len(), 2 * PAGE_SIZE as usize);

        let refresh = mailbox.refresh(6).unwrap();
        mailbox.finish_page(refresh.id, full_page(0, 120));

        // Both pages survive, so refreshing does not send the user back to
        // the first fifty rows.
        assert_eq!(mailbox.conversations().len(), 2 * PAGE_SIZE as usize);
        assert_eq!(mailbox.loaded_conversation_limit(), 2 * PAGE_SIZE);
        assert!(mailbox.has_more());

        // A short reload means the folder itself is short now.
        let refresh = mailbox.refresh(7).unwrap();
        mailbox.finish_page(refresh.id, dated_page(&[("p0", 1_000_000)], 1));
        assert_eq!(visible_ids(&mailbox), ["p0"]);
    }

    #[test]
    fn a_lost_counts_response_never_blocks_later_refreshes() {
        let mut mailbox = loaded_inbox(&["a"], 1);
        let counts = |unread: u32| -> MailboxCounts {
            [(sys(MailFolder::Inbox), unread)].into_iter().collect()
        };

        // The response for the first request never arrives.
        assert_eq!(mailbox.refresh_counts(3), Some(3));
        assert_eq!(mailbox.refresh_counts(4), Some(4));

        // Arriving late, it is ignored in favour of the newest request.
        assert_eq!(mailbox.finish_counts(3, Ok(counts(7))), None);
        assert!(mailbox.counts().is_none());

        assert_eq!(mailbox.finish_counts(4, Ok(counts(2))), None);
        assert_eq!(
            mailbox.counts().unwrap().unread(&sys(MailFolder::Inbox)),
            Some(2)
        );
    }

    #[test]
    fn closing_the_reader_drops_the_selectable_bodies() {
        let mut mailbox = loaded_inbox(&["a", "b"], 2);
        load_detail(&mut mailbox, "a", detail("a", &["a1"]), 3);
        assert!(mailbox.selectable_body("a1").is_some());

        mailbox.select_folder(sys(MailFolder::Archive), 4);
        assert!(mailbox.selectable_body("a1").is_none());

        // Same when a search hides the open conversation.
        let mut mailbox = loaded_inbox(&["a", "b"], 2);
        load_detail(&mut mailbox, "a", detail("a", &["a1"]), 5);
        mailbox.set_search_query("nothing matches this".into());

        assert!(mailbox.selectable_body("a1").is_none());
    }

    #[test]
    fn a_row_without_a_date_keeps_its_place_among_dated_mail() {
        let mut rows = vec![
            summary("undated"),
            dated("older", 100),
            dated("newest", 300),
        ];

        sort_newest_first(&mut rows);

        // It borrows the time of the row it sits next to instead of floating
        // to the top of the mailbox.
        assert_eq!(
            rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
            ["newest", "undated", "older"]
        );
    }

    #[test]
    fn opening_loads_first_inbox_page_with_unknown_counts() {
        let (mailbox, request) = open();

        assert_eq!(
            request,
            PageRequest {
                id: 1,
                folder: Folder::INBOX,
                page: 0,
                page_size: PAGE_SIZE,
            }
        );
        assert_eq!(mailbox.status(), ListStatus::Loading(1));
        assert_eq!(mailbox.counts(), None);
        assert!(mailbox.reader().is_none());
    }

    #[test]
    fn loading_then_loaded() {
        let mailbox = loaded_inbox(&["a", "b"], 2);

        assert_eq!(mailbox.status(), ListStatus::Loaded);
        assert_eq!(ids(&mailbox), ["a", "b"]);
        assert!(!mailbox.has_more());
    }

    #[test]
    fn loading_then_error() {
        let (mut mailbox, request) = open();

        let error = mailbox.finish_page(request.id, Err(MailboxError::Connection));

        assert_eq!(error, Some(MailboxError::Connection));
        assert_eq!(
            mailbox.status(),
            ListStatus::Failed(MailboxError::Connection)
        );
        assert!(mailbox.conversations().is_empty());
    }

    #[test]
    fn folder_switch_resets_pagination() {
        let mut mailbox = loaded_inbox(&["a"], 200);
        mailbox.start_conversation_load("a".into(), 9);

        let request = mailbox.select_folder(sys(MailFolder::Sent), 3).unwrap();

        assert_eq!(request.folder, sys(MailFolder::Sent));
        assert_eq!(request.page, 0);
        assert_eq!(mailbox.status(), ListStatus::Loading(3));
        assert!(mailbox.conversations().is_empty());
        assert!(!mailbox.has_more());
        assert_eq!(mailbox.selected_conversation(), None);
        assert!(mailbox.reader().is_none());
    }

    #[test]
    fn selecting_current_folder_is_a_no_op() {
        let mut mailbox = loaded_inbox(&["a"], 1);

        assert_eq!(mailbox.select_folder(sys(MailFolder::Inbox), 3), None);
        assert_eq!(ids(&mailbox), ["a"]);
    }

    #[test]
    fn reselecting_keeps_expansion_and_switching_resets_it() {
        let mut mailbox = loaded_inbox(&["a", "b"], 2);
        load_detail(&mut mailbox, "a", detail("a", &["a1", "a2"]), 3);
        mailbox.toggle_message("a1");

        assert_eq!(mailbox.start_conversation_load("a".into(), 4), None);
        assert!(mailbox.reader().unwrap().is_expanded("a1"));

        load_detail(&mut mailbox, "b", detail("b", &["b1"]), 5);
        load_detail(&mut mailbox, "a", detail("a", &["a1", "a2"]), 6);
        let reader = mailbox.reader().unwrap();
        assert!(!reader.is_expanded("a1"));
        assert!(reader.is_expanded("a2"));
    }

    #[test]
    fn stale_response_does_not_replace_current_folder() {
        let (mut mailbox, inbox) = open();
        let sent = mailbox.select_folder(sys(MailFolder::Sent), 3).unwrap();

        assert_eq!(mailbox.finish_page(inbox.id, page(&["inbox"], 1)), None);
        assert_eq!(mailbox.status(), ListStatus::Loading(sent.id));
        assert!(mailbox.conversations().is_empty());

        mailbox.finish_page(sent.id, page(&["sent"], 1));
        assert_eq!(mailbox.folder(), &sys(MailFolder::Sent));
        assert_eq!(ids(&mailbox), ["sent"]);
    }

    #[test]
    fn stale_error_is_ignored() {
        let (mut mailbox, inbox) = open();
        mailbox.select_folder(sys(MailFolder::Sent), 3);

        assert_eq!(
            mailbox.finish_page(inbox.id, Err(MailboxError::SessionExpired)),
            None
        );
        assert_eq!(mailbox.status(), ListStatus::Loading(3));
    }

    #[test]
    fn load_more_appends_without_duplicates() {
        let mut mailbox = loaded_inbox(&["a", "b"], 120);
        assert!(mailbox.has_more());

        let request = mailbox.load_more(3).unwrap();
        assert_eq!(request.page, 1);
        assert_eq!(mailbox.status(), ListStatus::LoadingMore(3));
        assert_eq!(ids(&mailbox), ["a", "b"]);

        mailbox.finish_page(request.id, page(&["b", "c"], 120));

        assert_eq!(ids(&mailbox), ["a", "b", "c"]);
        assert!(mailbox.has_more());
        assert_eq!(mailbox.load_more(4).unwrap().page, 2);
    }

    #[test]
    fn page_size_controls_whether_more_pages_exist() {
        let (mut mailbox, request) = Mailbox::open(Folder::INBOX, 2, 1, 2);
        mailbox.finish_page(request.id, page(&["a", "b"], 3));
        assert!(mailbox.has_more());

        let request = mailbox.load_more(3).unwrap();
        assert_eq!(request.page_size, 2);
        mailbox.finish_page(request.id, page(&["c"], 3));

        assert!(!mailbox.has_more());
    }

    #[test]
    fn duplicate_load_more_is_prevented() {
        let mut mailbox = loaded_inbox(&["a"], 120);

        assert!(mailbox.load_more(3).is_some());
        assert_eq!(mailbox.load_more(4), None);
        assert_eq!(mailbox.refresh(5), None);
        assert_eq!(mailbox.status(), ListStatus::LoadingMore(3));
    }

    #[test]
    fn load_more_requires_another_page() {
        let mut mailbox = loaded_inbox(&["a"], 1);

        assert_eq!(mailbox.load_more(3), None);
    }

    #[test]
    fn failed_load_more_keeps_loaded_conversations() {
        let mut mailbox = loaded_inbox(&["a"], 120);
        let request = mailbox.load_more(3).unwrap();

        mailbox.finish_page(request.id, Err(MailboxError::Connection));

        assert_eq!(ids(&mailbox), ["a"]);
        assert_eq!(mailbox.load_more(4).unwrap().page, 1);
    }

    #[test]
    fn refresh_replaces_list_only_after_success() {
        let mut mailbox = loaded_inbox(&["a", "b"], 2);
        mailbox.start_conversation_load("b".into(), 9);

        let request = mailbox.refresh(3).unwrap();
        assert_eq!(request.page, 0);
        assert_eq!(mailbox.status(), ListStatus::Refreshing(3));
        assert_eq!(ids(&mailbox), ["a", "b"]);

        mailbox.finish_page(request.id, page(&["c", "b"], 2));

        assert_eq!(ids(&mailbox), ["c", "b"]);
        assert_eq!(mailbox.selected_conversation(), Some("b"));
    }

    #[test]
    fn refresh_closes_reader_when_conversation_disappears() {
        let mut mailbox = loaded_inbox(&["a", "b"], 2);
        mailbox.start_conversation_load("b".into(), 9);

        let request = mailbox.refresh(3).unwrap();
        mailbox.finish_page(request.id, page(&["a"], 1));

        assert!(mailbox.reader().is_none());
    }

    #[test]
    fn failed_refresh_keeps_previous_list() {
        let mut mailbox = loaded_inbox(&["a"], 1);
        let request = mailbox.refresh(3).unwrap();

        mailbox.finish_page(request.id, Err(MailboxError::Service));

        assert_eq!(ids(&mailbox), ["a"]);
        assert_eq!(mailbox.status(), ListStatus::Failed(MailboxError::Service));
    }

    #[test]
    fn counts_apply_once() {
        let (mut mailbox, _) = open();

        // `open` left request 2 in flight; its response is the one that counts.
        let counts: MailboxCounts = [(sys(MailFolder::Inbox), 4)].into_iter().collect();
        mailbox.finish_counts(2, Ok(counts));
        assert_eq!(
            mailbox.counts().unwrap().unread(&sys(MailFolder::Inbox)),
            Some(4)
        );
        assert_eq!(
            mailbox.counts().unwrap().unread(&sys(MailFolder::Sent)),
            None
        );

        assert_eq!(mailbox.finish_counts(2, Ok(MailboxCounts::default())), None);
        assert_eq!(
            mailbox.counts().unwrap().unread(&sys(MailFolder::Inbox)),
            Some(4)
        );
        assert_eq!(mailbox.refresh_counts(3), Some(3));
    }

    #[test]
    fn selecting_conversation_starts_detail_loading() {
        let mut mailbox = loaded_inbox(&["a"], 1);

        let request = mailbox.start_conversation_load("a".into(), 3).unwrap();

        assert_eq!(
            request,
            ReaderRequest {
                id: 3,
                conversation_id: "a".into(),
                kind: SummaryKind::Conversation,
            }
        );
        assert_eq!(
            mailbox.reader_state(),
            &ReaderState::Loading {
                conversation_id: "a".into(),
                request: 3,
            }
        );
    }

    #[test]
    fn successful_detail_response_populates_matching_reader() {
        let mut mailbox = loaded_inbox(&["a"], 1);
        let request = mailbox.start_conversation_load("a".into(), 3).unwrap();

        assert_eq!(
            mailbox.finish_conversation(&request, Ok(detail("a", &["a1"]))),
            None
        );

        let reader = mailbox.reader().unwrap();
        assert_eq!(mailbox.selected_conversation(), Some("a"));
        assert_eq!(reader.detail().id, "a");
    }

    #[test]
    fn mismatched_detail_id_is_rejected() {
        let mut mailbox = loaded_inbox(&["a"], 1);
        let request = mailbox.start_conversation_load("a".into(), 3).unwrap();

        assert_eq!(
            mailbox.finish_conversation(&request, Ok(detail("b", &["b1"]))),
            Some(MailboxError::Unavailable)
        );
        assert_eq!(
            mailbox.reader_state(),
            &ReaderState::Failed {
                conversation_id: "a".into(),
                error: MailboxError::Unavailable,
            }
        );
    }

    #[test]
    fn newer_selection_ignores_stale_detail_response() {
        let mut mailbox = loaded_inbox(&["a", "b"], 2);
        let request_a = mailbox.start_conversation_load("a".into(), 3).unwrap();
        let request_b = mailbox.start_conversation_load("b".into(), 4).unwrap();

        assert_eq!(
            mailbox.finish_conversation(&request_a, Ok(detail("a", &["a1"]))),
            None
        );
        assert_eq!(mailbox.selected_conversation(), Some("b"));
        assert!(mailbox.reader().is_none());

        mailbox.finish_conversation(&request_b, Ok(detail("b", &["b1"])));
        assert_eq!(mailbox.reader().unwrap().detail().id, "b");
    }

    #[test]
    fn folder_switch_invalidates_outstanding_detail_request() {
        let mut mailbox = loaded_inbox(&["a"], 1);
        let detail_request = mailbox.start_conversation_load("a".into(), 3).unwrap();

        mailbox.select_folder(sys(MailFolder::Archive), 4);
        mailbox.finish_conversation(&detail_request, Ok(detail("a", &["a1"])));

        assert_eq!(mailbox.reader_state(), &ReaderState::Empty);
        assert_eq!(mailbox.selected_conversation(), None);
    }

    #[test]
    fn failed_detail_can_retry_with_new_request() {
        let mut mailbox = loaded_inbox(&["a"], 1);
        let first = mailbox.start_conversation_load("a".into(), 3).unwrap();
        mailbox.finish_conversation(&first, Err(MailboxError::Connection));
        assert_eq!(
            mailbox.reader_state(),
            &ReaderState::Failed {
                conversation_id: "a".into(),
                error: MailboxError::Connection,
            }
        );

        let retry = mailbox.retry_conversation(4).unwrap();

        assert_eq!(retry.id, 4);
        assert_eq!(retry.conversation_id, "a");
        assert_eq!(
            mailbox.reader_state(),
            &ReaderState::Loading {
                conversation_id: "a".into(),
                request: 4,
            }
        );
    }

    fn message_row(id: &str) -> ConversationSummary {
        ConversationSummary {
            kind: SummaryKind::Message,
            ..searchable_summary(id, "Bank", "bank@example.com", "Statement", "")
        }
    }

    #[test]
    fn message_rows_load_as_single_messages() {
        let (mut mailbox, request) = open();
        mailbox.finish_page(
            request.id,
            Ok(ConversationPage {
                conversations: vec![message_row("m1"), summary("c1")],
                total: 2,
            }),
        );

        let message = mailbox.start_conversation_load("m1".into(), 3).unwrap();
        assert_eq!(message.kind, SummaryKind::Message);
        mailbox.finish_conversation(&message, Err(MailboxError::Connection));
        assert_eq!(
            mailbox.retry_conversation(4).unwrap().kind,
            SummaryKind::Message
        );

        let conversation = mailbox.start_conversation_load("c1".into(), 5).unwrap();
        assert_eq!(conversation.kind, SummaryKind::Conversation);
    }

    #[test]
    fn stale_classified_page_cannot_replace_newer_folder() {
        let (mut mailbox, inbox) = open();
        mailbox.select_folder(sys(MailFolder::Sent), 3);

        let stale = mailbox.finish_page(
            inbox.id,
            Ok(ConversationPage {
                conversations: vec![message_row("m1"), message_row("m2")],
                total: 1,
            }),
        );

        assert_eq!(stale, None);
        assert_eq!(mailbox.status(), ListStatus::Loading(3));
        assert!(mailbox.conversations().is_empty());
    }

    #[test]
    fn split_rows_are_searchable_and_leave_counts_alone() {
        let (mut mailbox, request) = open();
        let counts: MailboxCounts = [(sys(MailFolder::Inbox), 2)].into_iter().collect();
        mailbox.finish_counts(2, Ok(counts.clone()));
        mailbox.finish_page(
            request.id,
            Ok(ConversationPage {
                conversations: vec![message_row("m1"), message_row("m2"), summary("c1")],
                total: 2,
            }),
        );

        mailbox.set_search_query("bank".into());

        let visible: Vec<_> = mailbox
            .visible_conversations()
            .map(|row| row.id.as_str())
            .collect();
        assert_eq!(visible, ["m1", "m2"]);
        assert_eq!(mailbox.counts(), Some(&counts));
    }

    fn selected_in(folder: MailFolder, ids: &[&str], selected: &str) -> Mailbox {
        let (mut mailbox, inbox) = open();
        let request = if folder == MailFolder::Inbox {
            inbox.id
        } else {
            mailbox.select_folder(sys(folder), 80).unwrap().id
        };
        mailbox.finish_page(request, page(ids, ids.len() as u32));
        let request = mailbox
            .start_conversation_load(selected.into(), 82)
            .unwrap();
        mailbox.finish_conversation(&request, Ok(detail(selected, &["message"])));
        mailbox
    }

    fn opened_unread(ids: &[&str], opened: &str) -> Mailbox {
        let (mut mailbox, request) = open();
        let rows = ids
            .iter()
            .map(|id| ConversationSummary {
                unread: true,
                ..summary(id)
            })
            .collect();
        mailbox.finish_page(
            request.id,
            Ok(ConversationPage {
                conversations: rows,
                total: ids.len() as u32,
            }),
        );
        let load = mailbox.start_conversation_load(opened.into(), 82).unwrap();
        mailbox.finish_conversation(&load, Ok(detail(opened, &["message"])));
        mailbox
    }

    #[test]
    fn opened_unread_rows_are_marked_read_once_confirmed() {
        let mut mailbox = opened_unread(&["a", "b"], "a");

        let request = mailbox.start_mark_read(3).unwrap();
        assert_eq!(request.row_id, "a");
        assert_eq!(request.action, MailAction::SetUnread(false));
        assert_eq!(mailbox.start_mark_read(4), None);
        assert!(mailbox.selected_summary().unwrap().unread);

        assert_eq!(mailbox.finish_mark_read(&request, Ok(())), Ok(true));
        assert!(!mailbox.selected_summary().unwrap().unread);
        assert_eq!(mailbox.start_mark_read(5), None);
    }

    #[test]
    fn reads_wait_for_the_content_to_load() {
        let (mut mailbox, request) = open();
        mailbox.finish_page(
            request.id,
            Ok(ConversationPage {
                conversations: vec![ConversationSummary {
                    unread: true,
                    ..summary("a")
                }],
                total: 1,
            }),
        );
        let load = mailbox.start_conversation_load("a".into(), 3).unwrap();

        assert_eq!(mailbox.start_mark_read(4), None);

        mailbox.finish_conversation(&load, Err(MailboxError::Connection));
        assert_eq!(mailbox.start_mark_read(5), None);
    }

    #[test]
    fn failed_or_stale_reads_leave_the_row_unread() {
        let mut mailbox = opened_unread(&["a"], "a");
        let request = mailbox.start_mark_read(3).unwrap();
        let stale = ActionRequest {
            id: 99,
            ..request.clone()
        };

        assert_eq!(mailbox.finish_mark_read(&stale, Ok(())), Ok(false));
        assert_eq!(
            mailbox.finish_mark_read(&request, Err(MailboxError::Connection)),
            Err(MailboxError::Connection)
        );
        assert!(mailbox.selected_summary().unwrap().unread);
        assert!(mailbox.start_mark_read(4).is_some());
    }

    #[test]
    fn explicit_unread_wins_over_an_automatic_read() {
        let mut mailbox = opened_unread(&["a"], "a");
        let read = mailbox.start_mark_read(3).unwrap();
        let unread = mailbox
            .start_action(MailAction::SetUnread(true), 4)
            .unwrap();

        assert_eq!(mailbox.finish_mark_read(&read, Ok(())), Ok(false));
        assert!(mailbox.selected_summary().unwrap().unread);
        assert_eq!(mailbox.finish_action(&unread, Ok(())), Ok(None));
        assert!(mailbox.selected_summary().unwrap().unread);
    }

    #[test]
    fn reads_after_a_folder_switch_change_nothing() {
        let mut mailbox = opened_unread(&["a"], "a");
        let request = mailbox.start_mark_read(3).unwrap();
        mailbox.select_folder(sys(MailFolder::Sent), 4);
        mailbox.finish_page(
            4,
            Ok(ConversationPage {
                conversations: vec![ConversationSummary {
                    unread: true,
                    ..summary("a")
                }],
                total: 1,
            }),
        );

        assert_eq!(mailbox.finish_mark_read(&request, Ok(())), Ok(false));
        assert!(mailbox.conversations()[0].unread);
    }

    #[test]
    fn one_action_runs_at_a_time_on_the_selected_row() {
        let mut mailbox = selected_in(MailFolder::Inbox, &["a", "b"], "a");

        let request = mailbox
            .start_action(MailAction::SetStarred(true), 3)
            .unwrap();

        assert_eq!(request.row_id, "a");
        assert_eq!(request.folder, sys(MailFolder::Inbox));
        assert!(mailbox.action_pending());
        assert_eq!(
            mailbox.start_action(MailAction::MoveTo(sys(MailFolder::Archive)), 4),
            None
        );
    }

    #[test]
    fn actions_wait_for_the_list_to_load() {
        let mut mailbox = selected_in(MailFolder::Inbox, &["a"], "a");
        mailbox.refresh(3);

        assert_eq!(
            mailbox.start_action(MailAction::MoveTo(sys(MailFolder::Archive)), 4),
            None
        );
        assert!(!mailbox.action_pending());
    }

    #[test]
    fn confirmed_flag_changes_update_the_row() {
        let mut mailbox = selected_in(MailFolder::Inbox, &["a", "b"], "a");

        let unread = mailbox
            .start_action(MailAction::SetUnread(true), 3)
            .unwrap();
        assert_eq!(mailbox.finish_action(&unread, Ok(())), Ok(None));
        let star = mailbox
            .start_action(MailAction::SetStarred(true), 4)
            .unwrap();
        assert_eq!(mailbox.finish_action(&star, Ok(())), Ok(None));

        let row = mailbox.selected_summary().unwrap();
        assert!(row.unread && row.starred);
        assert!(!mailbox.action_pending());
    }

    #[test]
    fn confirmed_moves_remove_the_row_and_open_the_next() {
        let mut mailbox = selected_in(MailFolder::Inbox, &["a", "b", "c"], "b");

        let request = mailbox
            .start_action(MailAction::MoveTo(sys(MailFolder::Archive)), 3)
            .unwrap();

        assert_eq!(
            mailbox.finish_action(&request, Ok(())),
            Ok(Some("c".into()))
        );
        assert_eq!(ids(&mailbox), ["a", "c"]);
        assert_eq!(mailbox.reader_state(), &ReaderState::Empty);
    }

    #[test]
    fn moving_into_the_current_folder_keeps_the_row() {
        let mut mailbox = selected_in(MailFolder::Archive, &["a"], "a");

        let request = mailbox
            .start_action(MailAction::MoveTo(sys(MailFolder::Archive)), 3)
            .unwrap();

        assert_eq!(mailbox.finish_action(&request, Ok(())), Ok(None));
        assert_eq!(ids(&mailbox), ["a"]);
    }

    #[test]
    fn unstarring_in_starred_removes_the_row() {
        let mut mailbox = selected_in(MailFolder::Starred, &["a", "b"], "b");

        let request = mailbox
            .start_action(MailAction::SetStarred(false), 3)
            .unwrap();

        assert_eq!(
            mailbox.finish_action(&request, Ok(())),
            Ok(Some("a".into()))
        );
        assert_eq!(ids(&mailbox), ["a"]);
    }

    #[test]
    fn failed_actions_keep_rows_and_report_the_error() {
        let mut mailbox = selected_in(MailFolder::Inbox, &["a", "b"], "a");
        let request = mailbox
            .start_action(MailAction::MoveTo(sys(MailFolder::Trash)), 3)
            .unwrap();

        let result = mailbox.finish_action(&request, Err(MailboxError::Service));

        assert_eq!(result, Err(MailboxError::Service));
        assert_eq!(ids(&mailbox), ["a", "b"]);
        assert_eq!(mailbox.action_error(), Some(MailboxError::Service));
        assert!(!mailbox.action_pending());
    }

    #[test]
    fn stale_or_moved_action_responses_change_nothing() {
        let mut mailbox = selected_in(MailFolder::Inbox, &["a", "b"], "a");
        let request = mailbox
            .start_action(MailAction::MoveTo(sys(MailFolder::Archive)), 3)
            .unwrap();
        let stale = ActionRequest {
            id: 99,
            ..request.clone()
        };

        assert_eq!(mailbox.finish_action(&stale, Ok(())), Ok(None));
        assert_eq!(ids(&mailbox), ["a", "b"]);
        assert!(mailbox.action_pending());

        mailbox.select_folder(sys(MailFolder::Sent), 4);
        mailbox.finish_page(4, page(&["a"], 1));
        assert_eq!(mailbox.finish_action(&request, Ok(())), Ok(None));
        assert_eq!(ids(&mailbox), ["a"]);
    }

    #[test]
    fn rows_without_a_time_keep_their_place() {
        let timed = |id: &str, time| ConversationSummary {
            time: Some(time),
            ..summary(id)
        };
        let (mut mailbox, request) = open();

        mailbox.finish_page(
            request.id,
            Ok(ConversationPage {
                conversations: vec![
                    timed("c1", 50),
                    summary("undated"),
                    timed("m1", 10),
                    timed("c2", 30),
                ],
                total: 4,
            }),
        );

        assert_eq!(ids(&mailbox), ["c1", "undated", "c2", "m1"]);
    }

    #[test]
    fn rows_stay_newest_first_across_pages() {
        let timed = |id: &str, time| ConversationSummary {
            time: Some(time),
            ..summary(id)
        };
        let (mut mailbox, request) = open();
        mailbox.finish_page(
            request.id,
            Ok(ConversationPage {
                conversations: vec![
                    timed("c1", 50),
                    timed("m1", 40),
                    timed("m2", 10),
                    timed("c2", 30),
                ],
                total: 120,
            }),
        );
        assert_eq!(ids(&mailbox), ["c1", "m1", "c2", "m2"]);

        let more = mailbox.load_more(3).unwrap();
        mailbox.finish_page(
            more.id,
            Ok(ConversationPage {
                conversations: vec![timed("c3", 20)],
                total: 120,
            }),
        );

        assert_eq!(ids(&mailbox), ["c1", "m1", "c2", "c3", "m2"]);
    }

    #[test]
    fn search_does_not_mutate_loaded_detail() {
        let mut mailbox = searchable_mailbox();
        let conversation = detail("subject", &["message"]);
        load_detail(&mut mailbox, "subject", conversation.clone(), 3);

        mailbox.set_search_query("rust".into());
        assert_eq!(mailbox.reader().unwrap().detail(), &conversation);

        mailbox.set_search_query(String::new());
        assert_eq!(mailbox.reader().unwrap().detail(), &conversation);
    }

    #[test]
    fn empty_search_returns_normal_folder_contents() {
        let mailbox = searchable_mailbox();

        assert_eq!(visible_ids(&mailbox), ["sender", "subject", "preview"]);
    }

    #[test]
    fn search_is_case_insensitive() {
        let mut mailbox = searchable_mailbox();

        mailbox.set_search_query("rUsT".into());

        assert_eq!(visible_ids(&mailbox), ["subject"]);
    }

    #[test]
    fn search_matches_sender_name() {
        let mut mailbox = searchable_mailbox();

        mailbox.set_search_query("alice stone".into());

        assert_eq!(visible_ids(&mailbox), ["sender"]);
    }

    #[test]
    fn search_matches_sender_email() {
        let mut mailbox = searchable_mailbox();

        mailbox.set_search_query("alice@example.com".into());

        assert_eq!(visible_ids(&mailbox), ["sender"]);
    }

    #[test]
    fn search_matches_subject() {
        let mut mailbox = searchable_mailbox();

        mailbox.set_search_query("project plan".into());

        assert_eq!(visible_ids(&mailbox), ["subject"]);
    }

    #[test]
    fn search_matches_preview() {
        let mut mailbox = searchable_mailbox();

        mailbox.set_search_query("launch checklist".into());

        assert_eq!(visible_ids(&mailbox), ["preview"]);
    }

    #[test]
    fn search_ignores_surrounding_whitespace() {
        let mut mailbox = searchable_mailbox();

        mailbox.set_search_query("  rust  ".into());

        assert_eq!(visible_ids(&mailbox), ["subject"]);
    }

    #[test]
    fn search_excludes_non_matching_conversations() {
        let mut mailbox = searchable_mailbox();

        mailbox.set_search_query("no such conversation".into());

        assert!(visible_ids(&mailbox).is_empty());
    }

    #[test]
    fn search_applies_to_the_selected_folder() {
        let mut mailbox = searchable_mailbox();
        mailbox.set_search_query("rust".into());
        let request = mailbox.select_folder(sys(MailFolder::Archive), 3).unwrap();
        mailbox.finish_page(
            request.id,
            Ok(ConversationPage {
                conversations: vec![searchable_summary(
                    "archived",
                    "Archived Sender",
                    "archive@example.com",
                    "Archived Rust notes",
                    "Old project",
                )],
                total: 1,
            }),
        );

        assert_eq!(mailbox.folder(), &sys(MailFolder::Archive));
        assert_eq!(visible_ids(&mailbox), ["archived"]);
    }

    #[test]
    fn clearing_search_restores_normal_results() {
        let mut mailbox = searchable_mailbox();
        mailbox.set_search_query("rust".into());

        mailbox.set_search_query(String::new());

        assert_eq!(visible_ids(&mailbox), ["sender", "subject", "preview"]);
    }

    #[test]
    fn no_match_clears_hidden_reader_selection() {
        let mut mailbox = searchable_mailbox();
        mailbox.start_conversation_load("sender".into(), 3);

        mailbox.set_search_query("rust".into());

        assert_eq!(mailbox.selected_conversation(), None);
        assert_eq!(visible_ids(&mailbox), ["subject"]);
    }

    #[test]
    fn search_does_not_change_mailbox_counts() {
        let mut mailbox = searchable_mailbox();
        let counts: MailboxCounts = [(sys(MailFolder::Inbox), 7)].into_iter().collect();
        mailbox.finish_counts(2, Ok(counts.clone()));

        mailbox.set_search_query("rust".into());
        mailbox.set_search_query("missing".into());
        mailbox.set_search_query(String::new());

        assert_eq!(mailbox.counts(), Some(&counts));
    }

    #[test]
    fn repeated_search_does_not_mutate_source_conversations() {
        let mut mailbox = searchable_mailbox();
        let original = mailbox.conversations().to_vec();

        for query in ["rust", "alice", "missing", ""] {
            mailbox.set_search_query(query.into());
            let _ = mailbox.visible_conversations().count();
        }

        assert_eq!(mailbox.conversations(), original);
    }
}
