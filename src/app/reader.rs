use std::collections::HashSet;

use crate::mail::{ConversationDetail, MailboxError};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ReaderState {
    #[default]
    Empty,
    Loading {
        conversation_id: String,
        request: u64,
    },
    Loaded(ConversationReader),
    Failed {
        conversation_id: String,
        error: MailboxError,
    },
}

impl ReaderState {
    pub fn conversation_id(&self) -> Option<&str> {
        match self {
            Self::Empty => None,
            Self::Loading {
                conversation_id, ..
            }
            | Self::Failed {
                conversation_id, ..
            } => Some(conversation_id),
            Self::Loaded(reader) => Some(reader.conversation_id()),
        }
    }
}

/// Reader state for the selected conversation. Which messages and quoted
/// passages are expanded is view state, so it lives here rather than in the
/// mail models.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationReader {
    detail: ConversationDetail,
    expanded: HashSet<String>,
    /// Unfolded quoted passages, as (message id, position in that message).
    expanded_quotes: HashSet<(String, usize)>,
}

impl ConversationReader {
    /// Orders messages oldest first and expands only the newest one.
    pub fn new(mut detail: ConversationDetail) -> Self {
        detail.messages.sort_by_key(|message| message.time);
        let expanded = detail
            .messages
            .last()
            .map(|message| message.id.clone())
            .into_iter()
            .collect();

        Self {
            detail,
            expanded,
            expanded_quotes: HashSet::new(),
        }
    }

    pub fn conversation_id(&self) -> &str {
        &self.detail.id
    }

    pub fn detail(&self) -> &ConversationDetail {
        &self.detail
    }

    pub fn is_expanded(&self, message_id: &str) -> bool {
        self.expanded.contains(message_id)
    }

    pub fn toggle(&mut self, message_id: &str) {
        if self.knows(message_id) && !self.expanded.remove(message_id) {
            self.expanded.insert(message_id.to_owned());
        }
    }

    /// Whether the quoted passage at `index` of `message_id` is unfolded.
    /// Quotes start folded: a reply usually repeats the whole thread below it.
    pub fn is_quote_expanded(&self, message_id: &str, index: usize) -> bool {
        self.expanded_quotes
            .iter()
            .any(|(id, position)| id == message_id && *position == index)
    }

    pub fn toggle_quote(&mut self, message_id: &str, index: usize) {
        if !self.knows(message_id) {
            return;
        }
        let quote = (message_id.to_owned(), index);
        if !self.expanded_quotes.remove(&quote) {
            self.expanded_quotes.insert(quote);
        }
    }

    fn knows(&self, message_id: &str) -> bool {
        self.detail
            .messages
            .iter()
            .any(|message| message.id == message_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail::{MailAddress, MailMessage, MessageBody};

    fn message(id: &str, time: i64) -> MailMessage {
        MailMessage {
            id: id.to_owned(),
            sender: MailAddress::default(),
            recipients: Vec::new(),
            time: Some(time),
            body: MessageBody::PlainText(String::new()),
            attachments: 0,
        }
    }

    fn reader(messages: Vec<MailMessage>) -> ConversationReader {
        let detail = ConversationDetail {
            id: "conversation".into(),
            subject: None,
            messages,
        };
        ConversationReader::new(detail)
    }

    fn order(reader: &ConversationReader) -> Vec<&str> {
        reader
            .detail()
            .messages
            .iter()
            .map(|m| m.id.as_str())
            .collect()
    }

    #[test]
    fn messages_are_chronological_with_newest_expanded() {
        let reader = reader(vec![message("c", 30), message("a", 10), message("b", 20)]);

        assert_eq!(order(&reader), ["a", "b", "c"]);
        assert!(reader.is_expanded("c"));
        assert!(!reader.is_expanded("a"));
        assert!(!reader.is_expanded("b"));
    }

    #[test]
    fn toggling_changes_one_message_independently() {
        let mut reader = reader(vec![message("a", 10), message("b", 20), message("c", 30)]);

        reader.toggle("a");
        assert!(reader.is_expanded("a"));
        assert!(reader.is_expanded("c"));
        assert!(!reader.is_expanded("b"));

        reader.toggle("c");
        assert!(!reader.is_expanded("c"));
        assert!(reader.is_expanded("a"));
    }

    #[test]
    fn quotes_start_folded_and_toggle_one_at_a_time() {
        let mut reader = reader(vec![message("a", 10), message("b", 20)]);

        assert!(!reader.is_quote_expanded("a", 0));

        reader.toggle_quote("a", 0);
        assert!(reader.is_quote_expanded("a", 0));
        // Neither the next quote of the same message nor the same position of
        // another message follows along.
        assert!(!reader.is_quote_expanded("a", 1));
        assert!(!reader.is_quote_expanded("b", 0));

        reader.toggle_quote("a", 0);
        assert!(!reader.is_quote_expanded("a", 0));
    }

    #[test]
    fn unknown_messages_cannot_be_expanded() {
        let mut reader = reader(vec![message("a", 10)]);

        reader.toggle("missing");
        reader.toggle_quote("missing", 0);

        assert!(!reader.is_expanded("missing"));
        assert!(!reader.is_quote_expanded("missing", 0));
    }

    #[test]
    fn empty_detail_has_nothing_expanded() {
        let empty = reader(Vec::new());

        assert!(empty.detail().messages.is_empty());
        assert!(empty.expanded.is_empty());
    }

    #[test]
    fn reader_state_starts_empty() {
        assert_eq!(ReaderState::default(), ReaderState::Empty);
        assert_eq!(ReaderState::Empty.conversation_id(), None);
    }
}
