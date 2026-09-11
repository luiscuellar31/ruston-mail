use std::collections::HashSet;

use crate::mail::ConversationDetail;

/// Reader state for the selected conversation. Which messages are expanded is
/// view state, so it lives here rather than in the mail models.
pub struct ConversationReader {
    conversation_id: String,
    /// `None` when the backend cannot read conversations yet.
    detail: Option<ConversationDetail>,
    expanded: HashSet<String>,
}

impl ConversationReader {
    /// Orders messages oldest first and expands only the newest one.
    pub fn new(conversation_id: String, mut detail: Option<ConversationDetail>) -> Self {
        if let Some(detail) = &mut detail {
            detail.messages.sort_by_key(|message| message.time);
        }
        let expanded = detail
            .as_ref()
            .and_then(|detail| detail.messages.last())
            .map(|message| message.id.clone())
            .into_iter()
            .collect();

        Self {
            conversation_id,
            detail,
            expanded,
        }
    }

    pub fn conversation_id(&self) -> &str {
        &self.conversation_id
    }

    pub fn detail(&self) -> Option<&ConversationDetail> {
        self.detail.as_ref()
    }

    pub fn is_expanded(&self, message_id: &str) -> bool {
        self.expanded.contains(message_id)
    }

    pub fn toggle(&mut self, message_id: &str) {
        let known = self
            .detail
            .as_ref()
            .is_some_and(|detail| detail.messages.iter().any(|m| m.id == message_id));
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
        ConversationReader::new("conversation".into(), Some(detail))
    }

    fn order(reader: &ConversationReader) -> Vec<&str> {
        reader
            .detail()
            .unwrap()
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
    fn missing_or_empty_detail_has_nothing_expanded() {
        let unavailable = ConversationReader::new("conversation".into(), None);
        let empty = reader(Vec::new());

        assert!(unavailable.detail().is_none());
        assert!(empty.detail().unwrap().messages.is_empty());
        assert!(empty.expanded.is_empty());
    }
}
