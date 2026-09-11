use std::collections::HashMap;

/// Standard Proton Mail folders supported by Ruston's mailbox view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MailFolder {
    Inbox,
    Drafts,
    Sent,
    Starred,
    Archive,
    Spam,
    Trash,
}

impl MailFolder {
    pub const ALL: [Self; 7] = [
        Self::Inbox,
        Self::Drafts,
        Self::Sent,
        Self::Starred,
        Self::Archive,
        Self::Spam,
        Self::Trash,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Inbox => "Inbox",
            Self::Drafts => "Drafts",
            Self::Sent => "Sent",
            Self::Starred => "Starred",
            Self::Archive => "Archive",
            Self::Spam => "Spam",
            Self::Trash => "Trash",
        }
    }
}

/// The conversation metadata shown in the mailbox list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationSummary {
    pub id: String,
    pub subject: Option<String>,
    /// Senders, or recipients in Sent and Drafts.
    pub correspondents: Option<String>,
    /// Unix timestamp in seconds.
    pub time: Option<i64>,
    pub unread: bool,
    pub starred: bool,
    pub message_count: u32,
}

#[derive(Debug, Clone)]
pub struct ConversationPage {
    pub conversations: Vec<ConversationSummary>,
    /// Total conversations in the folder, across all pages.
    pub total: u32,
}

/// Unread conversation counts per folder. A missing folder means unknown.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MailboxCounts {
    unread: HashMap<MailFolder, u32>,
}

impl MailboxCounts {
    pub fn unread(&self, folder: MailFolder) -> Option<u32> {
        self.unread.get(&folder).copied()
    }
}

impl FromIterator<(MailFolder, u32)> for MailboxCounts {
    fn from_iter<I: IntoIterator<Item = (MailFolder, u32)>>(iter: I) -> Self {
        Self {
            unread: iter.into_iter().collect(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailboxError {
    Connection,
    SessionExpired,
    Service,
    Unavailable,
}

impl MailboxError {
    pub fn message(self) -> &'static str {
        match self {
            Self::Connection => "Ruston could not connect to Proton Mail.",
            Self::SessionExpired => "Your Proton session has expired. Sign in again.",
            Self::Service => "Proton Mail could not load this folder. Try again later.",
            Self::Unavailable => "Ruston could not load this folder. Try again.",
        }
    }
}
