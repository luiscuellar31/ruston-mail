use std::collections::{HashMap, HashSet};

use super::reader::{ConversationReader, ReaderState};
use crate::mail::{
    ConversationDetail, ConversationPage, ConversationSummary, MailAction, MailFolder,
    MailboxCounts, MailboxError, SummaryKind,
};

/// Identifies an asynchronous mailbox request. Only the response matching the
/// request currently in flight is applied; anything else is stale.
pub type RequestId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageRequest {
    pub id: RequestId,
    pub folder: MailFolder,
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
    /// The folder the action was started from.
    pub folder: MailFolder,
    pub action: MailAction,
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

pub struct Mailbox {
    folder: MailFolder,
    status: ListStatus,
    conversations: Vec<ConversationSummary>,
    page_size: u32,
    next_page: u32,
    has_more: bool,
    counts: Option<MailboxCounts>,
    counts_request: Option<RequestId>,
    reader: ReaderState,
    search_query: String,
    action_request: Option<ActionRequest>,
    action_error: Option<MailboxError>,
    /// Automatic reads in flight, by row, so each response only affects its
    /// own row and never blocks the action toolbar.
    pending_reads: HashMap<String, RequestId>,
}

impl Mailbox {
    /// Opens the Inbox, returning the first page to fetch. Counts are fetched
    /// under `counts_request`.
    pub fn open(
        page_size: u32,
        page_request: RequestId,
        counts_request: RequestId,
    ) -> (Self, PageRequest) {
        let mailbox = Self {
            folder: MailFolder::Inbox,
            status: ListStatus::Loading(page_request),
            conversations: Vec::new(),
            page_size,
            next_page: 0,
            has_more: false,
            counts: None,
            counts_request: Some(counts_request),
            reader: ReaderState::Empty,
            search_query: String::new(),
            action_request: None,
            action_error: None,
            pending_reads: HashMap::new(),
        };
        let page = mailbox.page_request(page_request, 0);

        (mailbox, page)
    }

    pub fn folder(&self) -> MailFolder {
        self.folder
    }

    pub fn status(&self) -> ListStatus {
        self.status
    }

    #[cfg(test)]
    pub fn conversations(&self) -> &[ConversationSummary] {
        &self.conversations
    }

    pub fn visible_conversations(&self) -> impl Iterator<Item = &ConversationSummary> {
        let query = self.search_query.trim().to_lowercase();
        self.conversations
            .iter()
            .filter(move |conversation| matches_search(conversation, &query))
    }

    pub fn search_query(&self) -> &str {
        &self.search_query
    }

    pub fn is_searching(&self) -> bool {
        !self.search_query.trim().is_empty()
    }

    pub fn set_search_query(&mut self, query: String) {
        self.search_query = query;
        let selected_is_hidden = self.selected_conversation().is_some_and(|selected| {
            !self
                .visible_conversations()
                .any(|conversation| conversation.id == selected)
        });
        if selected_is_hidden {
            self.reader = ReaderState::Empty;
        }
    }

    pub fn counts(&self) -> Option<&MailboxCounts> {
        self.counts.as_ref()
    }

    pub fn has_more(&self) -> bool {
        self.has_more
    }

    pub fn selected_conversation(&self) -> Option<&str> {
        self.reader.conversation_id()
    }

    pub fn selected_summary(&self) -> Option<&ConversationSummary> {
        let selected = self.selected_conversation()?;
        self.conversations
            .iter()
            .find(|conversation| conversation.id == selected)
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
    /// conversation keeps its loaded or in-flight state.
    pub fn start_conversation_load(
        &mut self,
        conversation_id: String,
        request: RequestId,
    ) -> Option<ReaderRequest> {
        if self.selected_conversation() == Some(conversation_id.as_str()) {
            return None;
        }
        let kind = self
            .visible_conversations()
            .find(|conversation| conversation.id == conversation_id)?
            .kind;
        self.action_error = None;

        let request = ReaderRequest {
            id: request,
            conversation_id,
            kind,
        };
        self.reader = ReaderState::Loading {
            conversation_id: request.conversation_id.clone(),
            request: request.id,
        };
        Some(request)
    }

    pub fn retry_conversation(&mut self, request: RequestId) -> Option<ReaderRequest> {
        let ReaderState::Failed {
            conversation_id, ..
        } = &self.reader
        else {
            return None;
        };
        let kind = self
            .conversations
            .iter()
            .find(|conversation| &conversation.id == conversation_id)?
            .kind;
        let request = ReaderRequest {
            id: request,
            conversation_id: conversation_id.clone(),
            kind,
        };
        self.reader = ReaderState::Loading {
            conversation_id: request.conversation_id.clone(),
            request: request.id,
        };
        Some(request)
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
                self.reader = ReaderState::Loaded(ConversationReader::new(detail));
                None
            }
            Ok(_) => {
                let error = MailboxError::Unavailable;
                self.reader = ReaderState::Failed {
                    conversation_id: request.conversation_id.clone(),
                    error,
                };
                Some(error)
            }
            Err(error) => {
                self.reader = ReaderState::Failed {
                    conversation_id: request.conversation_id.clone(),
                    error,
                };
                Some(error)
            }
        }
    }

    pub fn toggle_message(&mut self, message_id: &str) {
        if let ReaderState::Loaded(reader) = &mut self.reader {
            reader.toggle(message_id);
        }
    }

    /// Starts marking the opened row read once its content is shown. Only an
    /// unread row with no read already in flight qualifies.
    pub fn start_mark_read(&mut self, request: RequestId) -> Option<ActionRequest> {
        let ReaderState::Loaded(reader) = &self.reader else {
            return None;
        };
        let row = self
            .conversations
            .iter()
            .find(|row| row.id == reader.conversation_id())?;
        if !row.unread || self.pending_reads.contains_key(&row.id) {
            return None;
        }

        let request = ActionRequest {
            id: request,
            row_id: row.id.clone(),
            kind: row.kind,
            folder: self.folder,
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

        let Some(row) = self
            .conversations
            .iter_mut()
            .find(|row| row.id == request.row_id)
        else {
            return Ok(false);
        };
        row.unread = false;
        Ok(true)
    }

    /// Starts an action on the selected row. Only one action runs at a time,
    /// and none while the list itself is loading.
    pub fn start_action(
        &mut self,
        action: MailAction,
        request: RequestId,
    ) -> Option<ActionRequest> {
        if self.is_busy() || self.action_request.is_some() {
            return None;
        }
        let (row_id, kind) = self
            .selected_summary()
            .map(|summary| (summary.id.clone(), summary.kind))?;

        let request = ActionRequest {
            id: request,
            row_id,
            kind,
            folder: self.folder,
            action,
        };
        self.action_error = None;
        // An explicit read or unread wins over an automatic read in flight.
        if matches!(action, MailAction::SetUnread(_)) {
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

        let leaves_folder = match request.action {
            MailAction::SetStarred(starred) => !starred && request.folder == MailFolder::Starred,
            MailAction::SetUnread(_) => false,
            moved => moved.destination() != Some(request.folder),
        };
        if leaves_folder {
            return Ok(self.remove_row(&request.row_id));
        }

        if let Some(row) = self
            .conversations
            .iter_mut()
            .find(|row| row.id == request.row_id)
        {
            match request.action {
                MailAction::SetUnread(unread) => row.unread = unread,
                MailAction::SetStarred(starred) => row.starred = starred,
                MailAction::Archive | MailAction::MoveToSpam | MailAction::MoveToTrash => {}
            }
        }
        Ok(None)
    }

    /// Removes a row. When it was selected, clears the reader and returns the
    /// visible row that took its place.
    fn remove_row(&mut self, row_id: &str) -> Option<String> {
        let was_selected = self.selected_conversation() == Some(row_id);
        let index = self
            .visible_conversations()
            .position(|row| row.id == row_id);
        self.conversations.retain(|row| row.id != row_id);
        if !was_selected {
            return None;
        }

        self.reader = ReaderState::Empty;
        let last = self.visible_conversations().count().checked_sub(1)?;
        self.visible_conversations()
            .nth(index?.min(last))
            .map(|row| row.id.clone())
    }

    pub fn select_folder(&mut self, folder: MailFolder, request: RequestId) -> Option<PageRequest> {
        if folder == self.folder {
            return None;
        }

        self.folder = folder;
        self.conversations.clear();
        self.next_page = 0;
        self.has_more = false;
        self.reader = ReaderState::Empty;
        self.action_error = None;
        self.status = ListStatus::Loading(request);
        Some(self.page_request(request, 0))
    }

    pub fn refresh(&mut self, request: RequestId) -> Option<PageRequest> {
        if self.is_busy() {
            return None;
        }

        self.status = if self.conversations.is_empty() {
            ListStatus::Loading(request)
        } else {
            ListStatus::Refreshing(request)
        };
        Some(self.page_request(request, 0))
    }

    pub fn load_more(&mut self, request: RequestId) -> Option<PageRequest> {
        if self.is_busy() || !self.has_more {
            return None;
        }

        self.status = ListStatus::LoadingMore(request);
        Some(self.page_request(request, self.next_page))
    }

    pub fn refresh_counts(&mut self, request: RequestId) -> Option<RequestId> {
        if self.counts_request.is_some() {
            return None;
        }

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

        self.reader = ReaderState::Empty;
        let next_index = selected_index.min(self.visible_conversations().count().saturating_sub(1));
        self.visible_conversations()
            .nth(next_index)
            .map(|conversation| conversation.id.clone())
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

    fn page_request(&self, id: RequestId, page: u32) -> PageRequest {
        PageRequest {
            id,
            folder: self.folder,
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
            self.conversations = page.conversations;
            self.next_page = 1;
            let selected_left = self.selected_conversation().is_some_and(|selected| {
                !self
                    .conversations
                    .iter()
                    .any(|conversation| conversation.id == selected)
            });
            if selected_left {
                self.reader = ReaderState::Empty;
            }
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
    let mut previous = i64::MAX;
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
            .any(|value| value.to_lowercase().contains(query))
        || conversation.participants.iter().any(|participant| {
            participant
                .name
                .as_deref()
                .is_some_and(|name| name.to_lowercase().contains(query))
                || participant.address.to_lowercase().contains(query)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail::{MailAddress, MailMessage, MessageBody};

    const PAGE_SIZE: u32 = 50;

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
            messages: message_ids
                .iter()
                .enumerate()
                .map(|(time, message_id)| MailMessage {
                    id: (*message_id).to_owned(),
                    sender: MailAddress::default(),
                    recipients: Vec::new(),
                    time: Some(time as i64),
                    body: MessageBody::PlainText(String::new()),
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
        Mailbox::open(PAGE_SIZE, 1, 2)
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
    fn opening_loads_first_inbox_page_with_unknown_counts() {
        let (mailbox, request) = open();

        assert_eq!(
            request,
            PageRequest {
                id: 1,
                folder: MailFolder::Inbox,
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

        let request = mailbox.select_folder(MailFolder::Sent, 3).unwrap();

        assert_eq!(request.folder, MailFolder::Sent);
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

        assert_eq!(mailbox.select_folder(MailFolder::Inbox, 3), None);
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
        let sent = mailbox.select_folder(MailFolder::Sent, 3).unwrap();

        assert_eq!(mailbox.finish_page(inbox.id, page(&["inbox"], 1)), None);
        assert_eq!(mailbox.status(), ListStatus::Loading(sent.id));
        assert!(mailbox.conversations().is_empty());

        mailbox.finish_page(sent.id, page(&["sent"], 1));
        assert_eq!(mailbox.folder(), MailFolder::Sent);
        assert_eq!(ids(&mailbox), ["sent"]);
    }

    #[test]
    fn stale_error_is_ignored() {
        let (mut mailbox, inbox) = open();
        mailbox.select_folder(MailFolder::Sent, 3);

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
        let (mut mailbox, request) = Mailbox::open(2, 1, 2);
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
    fn counts_apply_once_and_prevent_duplicate_requests() {
        let (mut mailbox, _) = open();
        assert_eq!(mailbox.refresh_counts(3), None);

        let counts: MailboxCounts = [(MailFolder::Inbox, 4)].into_iter().collect();
        mailbox.finish_counts(2, Ok(counts));
        assert_eq!(mailbox.counts().unwrap().unread(MailFolder::Inbox), Some(4));
        assert_eq!(mailbox.counts().unwrap().unread(MailFolder::Sent), None);

        assert_eq!(mailbox.finish_counts(2, Ok(MailboxCounts::default())), None);
        assert_eq!(mailbox.counts().unwrap().unread(MailFolder::Inbox), Some(4));
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

        mailbox.select_folder(MailFolder::Archive, 4);
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
        mailbox.select_folder(MailFolder::Sent, 3);

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
        let counts: MailboxCounts = [(MailFolder::Inbox, 2)].into_iter().collect();
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
            mailbox.select_folder(folder, 80).unwrap().id
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
        mailbox.select_folder(MailFolder::Sent, 4);
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
        assert_eq!(request.folder, MailFolder::Inbox);
        assert!(mailbox.action_pending());
        assert_eq!(mailbox.start_action(MailAction::Archive, 4), None);
    }

    #[test]
    fn actions_wait_for_the_list_to_load() {
        let mut mailbox = selected_in(MailFolder::Inbox, &["a"], "a");
        mailbox.refresh(3);

        assert_eq!(mailbox.start_action(MailAction::Archive, 4), None);
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

        let request = mailbox.start_action(MailAction::Archive, 3).unwrap();

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

        let request = mailbox.start_action(MailAction::Archive, 3).unwrap();

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
        let request = mailbox.start_action(MailAction::MoveToTrash, 3).unwrap();

        let result = mailbox.finish_action(&request, Err(MailboxError::Service));

        assert_eq!(result, Err(MailboxError::Service));
        assert_eq!(ids(&mailbox), ["a", "b"]);
        assert_eq!(mailbox.action_error(), Some(MailboxError::Service));
        assert!(!mailbox.action_pending());
    }

    #[test]
    fn stale_or_moved_action_responses_change_nothing() {
        let mut mailbox = selected_in(MailFolder::Inbox, &["a", "b"], "a");
        let request = mailbox.start_action(MailAction::Archive, 3).unwrap();
        let stale = ActionRequest {
            id: 99,
            ..request.clone()
        };

        assert_eq!(mailbox.finish_action(&stale, Ok(())), Ok(None));
        assert_eq!(ids(&mailbox), ["a", "b"]);
        assert!(mailbox.action_pending());

        mailbox.select_folder(MailFolder::Sent, 4);
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
        let request = mailbox.select_folder(MailFolder::Archive, 3).unwrap();
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

        assert_eq!(mailbox.folder(), MailFolder::Archive);
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
        let counts: MailboxCounts = [(MailFolder::Inbox, 7)].into_iter().collect();
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
