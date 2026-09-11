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

/// What a mailbox row opens in the reader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SummaryKind {
    /// A Proton conversation shown as one thread.
    Conversation,
    /// One message that Ruston shows on its own instead of inside its Proton
    /// conversation.
    Message,
}

/// The conversation metadata shown in the mailbox list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationSummary {
    /// Conversation ID, or message ID for `SummaryKind::Message` rows.
    pub id: String,
    pub kind: SummaryKind,
    pub subject: Option<String>,
    /// Senders, or recipients in Sent and Drafts.
    pub correspondents: Option<String>,
    /// Locally available senders and recipients used by mailbox search.
    pub participants: Vec<MailAddress>,
    /// Locally available conversation snippet. `None` when the backend omits it.
    pub preview: Option<String>,
    /// Unix timestamp in seconds.
    pub time: Option<i64>,
    pub unread: bool,
    pub starred: bool,
    pub message_count: u32,
}

/// A mail address. Either part may be missing in server data.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MailAddress {
    pub name: Option<String>,
    pub address: String,
}

impl MailAddress {
    /// The display name, falling back to the address. `None` when both are empty.
    pub fn display_name(&self) -> Option<&str> {
        self.name
            .as_deref()
            .filter(|name| !name.is_empty())
            .or((!self.address.is_empty()).then_some(self.address.as_str()))
    }
}

/// A message body. Only plain text is supported for now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageBody {
    PlainText(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailMessage {
    pub id: String,
    pub sender: MailAddress,
    pub recipients: Vec<MailAddress>,
    /// Unix timestamp in seconds.
    pub time: Option<i64>,
    pub body: MessageBody,
}

/// A conversation with its messages, as shown in the reader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationDetail {
    pub id: String,
    pub subject: Option<String>,
    pub messages: Vec<MailMessage>,
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

    pub fn conversation_message(self) -> &'static str {
        match self {
            Self::Connection => "Ruston could not connect to load this conversation.",
            Self::SessionExpired => "Your Proton session has expired. Sign in again.",
            Self::Service => "Proton Mail could not load this conversation. Try again later.",
            Self::Unavailable => "Ruston could not load this conversation. Try again.",
        }
    }
}
