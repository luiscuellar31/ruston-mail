use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Standard Proton Mail folders supported by Ruston's mailbox view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
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

    /// Whether mail actually lives in this folder, so a row can be moved back
    /// into it. Starred is a label, and Sent and Drafts describe origin.
    pub fn is_location(self) -> bool {
        matches!(self, Self::Inbox | Self::Archive | Self::Spam | Self::Trash)
    }

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

/// What a place the account made is: somewhere mail lives, or a name mail
/// carries while living somewhere else.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CustomKind {
    /// A folder: mail is in it, and moving a row elsewhere takes it out.
    #[default]
    Folder,
    /// A label: mail keeps its folder and carries this name as well.
    Label,
}

/// A system folder or account-owned folder/label.
/// The Proton layer maps it to a wire identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Folder {
    /// Reads the settings written before custom folders existed, which hold
    /// a bare name like `"Inbox"`.
    System(MailFolder),
    Custom {
        id: String,
        name: String,
        /// Missing from settings written before labels were listed, which
        /// held folders only.
        #[serde(default)]
        kind: CustomKind,
    },
}

impl Folder {
    pub const INBOX: Self = Self::System(MailFolder::Inbox);

    /// A folder the account made.
    pub fn custom(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self::Custom {
            id: id.into(),
            name: name.into(),
            kind: CustomKind::Folder,
        }
    }

    /// A label the account made.
    pub fn label(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self::Custom {
            id: id.into(),
            name: name.into(),
            kind: CustomKind::Label,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::System(folder) => folder.name(),
            Self::Custom { name, .. } => name,
        }
    }

    /// The system folder this is, if it is one. Rules that only make sense
    /// for Proton's own folders ask through this.
    pub fn system(&self) -> Option<MailFolder> {
        match self {
            Self::System(folder) => Some(*folder),
            Self::Custom { .. } => None,
        }
    }

    /// What Proton calls a place the account made. Proton's own folders are
    /// named by the Proton layer instead, so this is `None` for them.
    pub fn custom_id(&self) -> Option<&str> {
        match self {
            Self::System(_) => None,
            Self::Custom { id, .. } => Some(id),
        }
    }

    /// What kind of place the account made this one as; `None` for Proton's
    /// own folders.
    pub fn kind(&self) -> Option<CustomKind> {
        match self {
            Self::System(_) => None,
            Self::Custom { kind, .. } => Some(*kind),
        }
    }

    /// Whether mail actually lives here, so a row can be moved back into it.
    /// A label is a name mail carries, not a place it sits in.
    pub fn is_location(&self) -> bool {
        match self {
            Self::System(folder) => folder.is_location(),
            Self::Custom { kind, .. } => *kind == CustomKind::Folder,
        }
    }
}

impl From<MailFolder> for Folder {
    fn from(folder: MailFolder) -> Self {
        Self::System(folder)
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
    /// Whether any message carries a file. Ruston Mail never downloads them.
    pub has_attachments: bool,
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

/// A message body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageBody {
    PlainText(String),
    /// Structure and inline styles taken from an HTML body. Nothing in it
    /// runs scripts or loads remote content.
    Rich(RichBody),
}

impl MessageBody {
    /// The body as plain text, for quoting it in an answer.
    pub fn plain_text(&self) -> String {
        match self {
            Self::PlainText(text) => text.clone(),
            Self::Rich(rich) => rich.plain_text(),
        }
    }
}

/// The readable structure of an HTML body, in reading order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RichBody {
    pub blocks: Vec<RichBlock>,
}

impl RichBody {
    /// The body as plain text, one block per line. Styles and links are
    /// dropped; structure is kept with markers and quote prefixes.
    pub fn plain_text(&self) -> String {
        let mut out = String::new();
        for block in &self.blocks {
            let line = match &block.kind {
                BlockKind::Paragraph(spans) | BlockKind::Heading { spans, .. } => join(spans),
                BlockKind::ListItem { marker, spans, .. } if marker.is_empty() => join(spans),
                BlockKind::ListItem { marker, spans, .. } => format!("{marker} {}", join(spans)),
                BlockKind::Preformatted(text) => text.clone(),
                BlockKind::Image { description } => format!("[Image: {description}]"),
                BlockKind::Rule => "---".to_owned(),
            };
            let quote = "> ".repeat(usize::from(block.quote_depth));
            for line in line.lines() {
                out.push_str(&quote);
                out.push_str(line);
                out.push('\n');
            }
        }

        out
    }
}

fn join(spans: &[RichSpan]) -> String {
    spans.iter().map(|span| span.text.as_str()).collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RichBlock {
    pub kind: BlockKind,
    /// How deeply the block sits inside quotes.
    pub quote_depth: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockKind {
    Paragraph(Vec<RichSpan>),
    Heading {
        level: u8,
        spans: Vec<RichSpan>,
    },
    /// A list item. `marker` is a bullet or number, and empty for further
    /// paragraphs of the same item.
    ListItem {
        marker: String,
        depth: u8,
        spans: Vec<RichSpan>,
    },
    /// Text whose spacing matters, shown in a monospace font.
    Preformatted(String),
    /// An image that is not loaded; only its description is shown.
    Image {
        description: String,
    },
    Rule,
}

/// A run of text sharing one style.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RichSpan {
    pub text: String,
    pub strong: bool,
    pub emphasis: bool,
    pub code: bool,
    pub struck: bool,
    /// Only absolute `http`, `https` and `mailto` links are kept.
    pub link: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailMessage {
    pub id: String,
    pub sender: MailAddress,
    pub recipients: Vec<MailAddress>,
    /// Unix timestamp in seconds.
    pub time: Option<i64>,
    pub body: MessageBody,
    /// Files the message carries, without their contents: those are fetched
    /// only when someone asks to save one.
    pub attachments: Vec<MailAttachment>,
}

/// A file on a message. Inline parts, which belong to the body rather than to
/// the reader, are left out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailAttachment {
    pub id: String,
    pub name: String,
    /// Size in bytes, as the server reports it.
    pub size: u64,
}

/// A conversation with its messages, as shown in the reader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationDetail {
    pub id: String,
    pub subject: Option<String>,
    pub messages: Vec<MailMessage>,
    /// Folder and label identifiers reported by Proton.
    pub labels: Vec<String>,
}

impl ConversationDetail {
    /// Whether the conversation carries a label the account made.
    #[cfg(test)]
    pub fn carries(&self, label: &Folder) -> bool {
        label
            .custom_id()
            .is_some_and(|id| self.labels.iter().any(|carried| carried == id))
    }

    /// Records a label the conversation was just given or had taken away, so
    /// the reader shows the change without asking the server again.
    pub fn set_label(&mut self, label: &Folder, on: bool) {
        let Some(id) = label.custom_id() else {
            return;
        };
        self.labels.retain(|carried| carried != id);
        if on {
            self.labels.push(id.to_owned());
        }
    }
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
    unread: HashMap<Folder, u32>,
}

impl MailboxCounts {
    pub fn unread(&self, folder: &Folder) -> Option<u32> {
        self.unread.get(folder).copied()
    }
}

impl FromIterator<(Folder, u32)> for MailboxCounts {
    fn from_iter<I: IntoIterator<Item = (Folder, u32)>>(iter: I) -> Self {
        Self {
            unread: iter.into_iter().collect(),
        }
    }
}

/// A change applied to the selected mailbox row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MailAction {
    /// Relabels the row into a folder; nothing is ever deleted.
    MoveTo(Folder),
    SetUnread(bool),
    SetStarred(bool),
    /// Gives the row a label the account made, or takes it away. The row
    /// stays in whatever folder it is already in.
    SetLabel {
        label: Folder,
        on: bool,
    },
}

impl MailAction {
    /// The folder a move sends the row to; `None` for flag changes.
    pub fn destination(&self) -> Option<&Folder> {
        match self {
            Self::MoveTo(folder) => Some(folder),
            // A label leaves the row where it is, so there is nothing to
            // take back.
            Self::SetUnread(_) | Self::SetStarred(_) | Self::SetLabel { .. } => None,
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
            Self::Connection => "Ruston Mail could not connect to Proton.",
            Self::SessionExpired => "Your Proton session has expired. Sign in again.",
            Self::Service => "Proton Mail could not load this folder. Try again later.",
            Self::Unavailable => "Ruston Mail could not load this folder. Try again.",
        }
    }

    pub fn conversation_message(self) -> &'static str {
        match self {
            Self::Connection => "Ruston Mail could not connect to load this conversation.",
            Self::SessionExpired => "Your Proton session has expired. Sign in again.",
            Self::Service => "Proton Mail could not load this conversation. Try again later.",
            Self::Unavailable => "Ruston Mail could not load this conversation. Try again.",
        }
    }

    pub fn action_message(self) -> &'static str {
        match self {
            Self::Connection => "Ruston Mail could not connect to update this conversation.",
            Self::SessionExpired => "Your Proton session has expired. Sign in again.",
            Self::Service => "Proton Mail could not update this conversation. Try again later.",
            Self::Unavailable => "Ruston Mail could not update this conversation. Try again.",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_written_before_custom_ones_still_reads() {
        // Settings files in the wild hold a bare system name.
        let stored: Folder = serde_json::from_str("\"Archive\"").unwrap();

        assert_eq!(stored, Folder::System(MailFolder::Archive));
        assert_eq!(stored.name(), "Archive");
    }

    #[test]
    fn a_custom_folder_survives_a_round_trip() {
        let folder = Folder::custom("kZ9", "Invoices");

        let text = serde_json::to_string(&folder).unwrap();
        assert_eq!(serde_json::from_str::<Folder>(&text).unwrap(), folder);

        assert_eq!(folder.name(), "Invoices");
        assert_eq!(folder.system(), None);
        // A folder someone made is somewhere mail lives, so a move can be
        // taken back into it, unlike Starred or Sent.
        assert!(folder.is_location());
        assert!(!Folder::System(MailFolder::Starred).is_location());
        assert!(Folder::INBOX.is_location());
    }

    #[test]
    fn a_label_survives_a_round_trip_and_is_not_a_location() {
        let label = Folder::label("wN2", "Receipts");

        let text = serde_json::to_string(&label).unwrap();
        assert_eq!(serde_json::from_str::<Folder>(&text).unwrap(), label);

        assert_eq!(label.name(), "Receipts");
        assert_eq!(label.kind(), Some(CustomKind::Label));
        // Mail keeps its folder while carrying a label, so a row that leaves
        // the folder it sits in cannot be put back through the label view.
        assert!(!label.is_location());
    }

    #[test]
    fn a_folder_written_before_labels_were_listed_still_reads() {
        // Files in the wild hold the id and name only.
        let stored: Folder = serde_json::from_str(r#"{"id": "kZ9", "name": "Invoices"}"#).unwrap();

        assert_eq!(stored, Folder::custom("kZ9", "Invoices"));
    }

    #[test]
    fn a_conversation_remembers_the_labels_it_carries() {
        let receipts = Folder::label("wN2", "Receipts");
        let mut detail = ConversationDetail {
            id: "c".to_owned(),
            subject: None,
            messages: Vec::new(),
            labels: vec!["0".to_owned()],
        };

        assert!(!detail.carries(&receipts));
        detail.set_label(&receipts, true);
        assert!(detail.carries(&receipts));

        // Giving the same label again records it once, not twice.
        detail.set_label(&receipts, true);
        assert_eq!(detail.labels.iter().filter(|id| *id == "wN2").count(), 1);

        detail.set_label(&receipts, false);
        assert!(!detail.carries(&receipts));
        // Proton's own folders are named in the Proton layer, so asking about
        // one here answers no rather than guessing at an identifier.
        assert!(!detail.carries(&Folder::INBOX));
        // Through all of it the folder the mail sits in is left alone.
        assert_eq!(detail.labels, ["0"]);
    }

    #[test]
    fn a_label_change_is_not_a_move() {
        let action = MailAction::SetLabel {
            label: Folder::label("wN2", "Receipts"),
            on: true,
        };

        // Nothing left its folder, so there is nothing to take back.
        assert_eq!(action.destination(), None);
    }

    fn spans(text: &str) -> Vec<RichSpan> {
        vec![RichSpan {
            text: text.to_owned(),
            ..RichSpan::default()
        }]
    }

    fn block(kind: BlockKind, quote_depth: u8) -> RichBlock {
        RichBlock { kind, quote_depth }
    }

    #[test]
    fn plain_text_keeps_structure_one_block_per_line() {
        let body = RichBody {
            blocks: vec![
                block(
                    BlockKind::Heading {
                        level: 1,
                        spans: spans("Title"),
                    },
                    0,
                ),
                block(BlockKind::Paragraph(spans("Hello")), 0),
                block(
                    BlockKind::ListItem {
                        marker: "•".to_owned(),
                        depth: 1,
                        spans: spans("One"),
                    },
                    0,
                ),
                block(
                    BlockKind::Image {
                        description: "Logo".to_owned(),
                    },
                    0,
                ),
                block(BlockKind::Rule, 0),
                block(BlockKind::Paragraph(spans("Quoted")), 1),
                block(
                    BlockKind::Preformatted("let x = 1;\n  indented".to_owned()),
                    2,
                ),
            ],
        };

        assert_eq!(
            body.plain_text(),
            "Title\nHello\n• One\n[Image: Logo]\n---\n> Quoted\n> > let x = 1;\n> >   indented\n"
        );
    }

    #[test]
    fn plain_text_covers_both_body_kinds() {
        let rich = MessageBody::Rich(RichBody {
            blocks: vec![block(BlockKind::Paragraph(spans("Hi")), 0)],
        });

        assert_eq!(MessageBody::PlainText("Hi".into()).plain_text(), "Hi");
        assert_eq!(rich.plain_text(), "Hi\n");
        assert_eq!(RichBody::default().plain_text(), "");
    }
}
