use std::collections::HashSet;

use crate::mail::{ConversationPage, ConversationSummary, MailFolder, MailboxCounts, MailboxError};

pub const PAGE_SIZE: u32 = 50;

/// Identifies an asynchronous mailbox request. Only the response matching the
/// request currently in flight is applied; anything else is stale.
pub type RequestId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageRequest {
    pub id: RequestId,
    pub folder: MailFolder,
    /// Zero-based server page.
    pub page: u32,
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
    next_page: u32,
    has_more: bool,
    counts: Option<MailboxCounts>,
    counts_request: Option<RequestId>,
    selected_conversation: Option<String>,
}

impl Mailbox {
    /// Opens the Inbox, returning the first page to fetch. Counts are fetched
    /// under `counts_request`.
    pub fn open(page_request: RequestId, counts_request: RequestId) -> (Self, PageRequest) {
        let mailbox = Self {
            folder: MailFolder::Inbox,
            status: ListStatus::Loading(page_request),
            conversations: Vec::new(),
            next_page: 0,
            has_more: false,
            counts: None,
            counts_request: Some(counts_request),
            selected_conversation: None,
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

    pub fn conversations(&self) -> &[ConversationSummary] {
        &self.conversations
    }

    pub fn counts(&self) -> Option<&MailboxCounts> {
        self.counts.as_ref()
    }

    pub fn has_more(&self) -> bool {
        self.has_more
    }

    pub fn selected_conversation(&self) -> Option<&str> {
        self.selected_conversation.as_deref()
    }

    pub fn is_busy(&self) -> bool {
        matches!(
            self.status,
            ListStatus::Loading(_) | ListStatus::Refreshing(_) | ListStatus::LoadingMore(_)
        )
    }

    pub fn select_conversation(&mut self, id: String) {
        self.selected_conversation = Some(id);
    }

    pub fn select_folder(&mut self, folder: MailFolder, request: RequestId) -> Option<PageRequest> {
        if folder == self.folder {
            return None;
        }

        self.folder = folder;
        self.conversations.clear();
        self.next_page = 0;
        self.has_more = false;
        self.selected_conversation = None;
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
            if let Some(selected) = &self.selected_conversation
                && !self.conversations.iter().any(|c| &c.id == selected)
            {
                self.selected_conversation = None;
            }
        }

        self.has_more = received > 0
            && u64::from(self.next_page) * u64::from(PAGE_SIZE) < u64::from(page.total);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(id: &str) -> ConversationSummary {
        ConversationSummary {
            id: id.to_owned(),
            subject: None,
            correspondents: None,
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

    fn ids(mailbox: &Mailbox) -> Vec<&str> {
        mailbox
            .conversations()
            .iter()
            .map(|c| c.id.as_str())
            .collect()
    }

    fn loaded_inbox(ids: &[&str], total: u32) -> Mailbox {
        let (mut mailbox, request) = Mailbox::open(1, 2);
        mailbox.finish_page(request.id, page(ids, total));
        mailbox
    }

    #[test]
    fn opening_loads_first_inbox_page_with_unknown_counts() {
        let (mailbox, request) = Mailbox::open(1, 2);

        assert_eq!(
            request,
            PageRequest {
                id: 1,
                folder: MailFolder::Inbox,
                page: 0
            }
        );
        assert_eq!(mailbox.status(), ListStatus::Loading(1));
        assert_eq!(mailbox.counts(), None);
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
        let (mut mailbox, request) = Mailbox::open(1, 2);

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
        mailbox.select_conversation("a".into());

        let request = mailbox.select_folder(MailFolder::Sent, 3).unwrap();

        assert_eq!(request.folder, MailFolder::Sent);
        assert_eq!(request.page, 0);
        assert_eq!(mailbox.status(), ListStatus::Loading(3));
        assert!(mailbox.conversations().is_empty());
        assert!(!mailbox.has_more());
        assert_eq!(mailbox.selected_conversation(), None);
    }

    #[test]
    fn selecting_current_folder_is_a_no_op() {
        let mut mailbox = loaded_inbox(&["a"], 1);

        assert_eq!(mailbox.select_folder(MailFolder::Inbox, 3), None);
        assert_eq!(ids(&mailbox), ["a"]);
    }

    #[test]
    fn stale_response_does_not_replace_current_folder() {
        let (mut mailbox, inbox) = Mailbox::open(1, 2);
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
        let (mut mailbox, inbox) = Mailbox::open(1, 2);
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
        mailbox.select_conversation("b".into());

        let request = mailbox.refresh(3).unwrap();
        assert_eq!(request.page, 0);
        assert_eq!(mailbox.status(), ListStatus::Refreshing(3));
        assert_eq!(ids(&mailbox), ["a", "b"]);

        mailbox.finish_page(request.id, page(&["c", "b"], 2));

        assert_eq!(ids(&mailbox), ["c", "b"]);
        assert_eq!(mailbox.selected_conversation(), Some("b"));
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
        let (mut mailbox, _) = Mailbox::open(1, 2);
        assert_eq!(mailbox.refresh_counts(3), None);

        let counts: MailboxCounts = [(MailFolder::Inbox, 4)].into_iter().collect();
        mailbox.finish_counts(2, Ok(counts));
        assert_eq!(mailbox.counts().unwrap().unread(MailFolder::Inbox), Some(4));
        assert_eq!(mailbox.counts().unwrap().unread(MailFolder::Sent), None);

        assert_eq!(mailbox.finish_counts(2, Ok(MailboxCounts::default())), None);
        assert_eq!(mailbox.counts().unwrap().unread(MailFolder::Inbox), Some(4));
        assert_eq!(mailbox.refresh_counts(3), Some(3));
    }
}
