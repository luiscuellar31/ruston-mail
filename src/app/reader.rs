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

/// Reader state for the selected conversation. Which messages are expanded is
/// view state, so it lives here rather than in the mail models.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationReader {
    detail: ConversationDetail,
    expanded: HashSet<String>,
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

        Self { detail, expanded }
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
        let known = self
            .detail
            .messages
            .iter()
            .any(|message| message.id == message_id);
        if known && !self.expanded.remove(message_id) {
            self.expanded.insert(message_id.to_owned());
        }
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
    fn unknown_messages_cannot_be_expanded() {
        let mut reader = reader(vec![message("a", 10)]);

        reader.toggle("missing");

        assert!(!reader.is_expanded("missing"));
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
