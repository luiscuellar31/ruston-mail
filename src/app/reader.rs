use std::collections::HashSet;

use crate::mail::{ConversationDetail, Folder, MailboxError};
use crate::settings::Reading;

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
    /// Messages folded the other way from what the settings ask for. Holding
    /// the difference rather than the state means changing the setting shows
    /// at once on the open conversation, without discarding what was folded
    /// by hand.
    toggled: HashSet<String>,
    /// Quoted passages folded the other way, as (message id, position).
    toggled_quotes: HashSet<(String, usize)>,
    /// Whether the labels the conversation does not carry are on show. It
    /// belongs to the open conversation, so moving on puts the row back the
    /// way it usually reads.
    showing_labels: bool,
}

impl ConversationReader {
    /// Orders messages oldest first. Which of them start open is left to the
    /// settings, read through [`Self::is_expanded`].
    pub fn new(mut detail: ConversationDetail) -> Self {
        detail.messages.sort_by_key(|message| message.time);

        Self {
            detail,
            toggled: HashSet::new(),
            toggled_quotes: HashSet::new(),
            showing_labels: false,
        }
    }

    pub fn conversation_id(&self) -> &str {
        &self.detail.id
    }

    pub fn detail(&self) -> &ConversationDetail {
        &self.detail
    }

    pub fn is_showing_labels(&self) -> bool {
        self.showing_labels
    }

    pub fn toggle_labels(&mut self) {
        self.showing_labels = !self.showing_labels;
    }

    /// Records a label the open conversation was just given or had taken
    /// away, so the reader shows it without loading the thread again.
    pub fn set_label(&mut self, label: &Folder, on: bool) {
        self.detail.set_label(label, on);
    }

    pub fn is_expanded(&self, message_id: &str, reading: Reading) -> bool {
        self.opens_on_its_own(message_id, reading) != self.toggled.contains(message_id)
    }

    /// Whether the settings alone would have this message open. Only the
    /// newest is worth reading straight away in a thread; the rest are the
    /// history behind it, unless every message was asked for.
    fn opens_on_its_own(&self, message_id: &str, reading: Reading) -> bool {
        reading.expand_all_messages
            || self
                .detail
                .messages
                .last()
                .is_some_and(|message| message.id == message_id)
    }

    pub fn toggle(&mut self, message_id: &str) {
        if self.knows(message_id) && !self.toggled.remove(message_id) {
            self.toggled.insert(message_id.to_owned());
        }
    }

    /// Whether the quoted passage at `index` of `message_id` is unfolded.
    /// Quotes start folded unless asked for: a reply usually repeats the whole
    /// thread below it.
    pub fn is_quote_expanded(&self, message_id: &str, index: usize, reading: Reading) -> bool {
        let toggled = self
            .toggled_quotes
            .iter()
            .any(|(id, position)| id == message_id && *position == index);
        reading.show_quoted_text != toggled
    }

    pub fn toggle_quote(&mut self, message_id: &str, index: usize) {
        if !self.knows(message_id) {
            return;
        }
        let quote = (message_id.to_owned(), index);
        if !self.toggled_quotes.remove(&quote) {
            self.toggled_quotes.insert(quote);
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
            attachments: Vec::new(),
        }
    }

    /// The settings as they ship: newest message open, quotes folded.
    fn plain() -> Reading {
        Reading::default()
    }

    fn reader(messages: Vec<MailMessage>) -> ConversationReader {
        let detail = ConversationDetail {
            id: "conversation".into(),
            subject: None,
            messages,
            labels: Vec::new(),
        };
        ConversationReader::new(detail)
    }

    #[test]
    fn the_label_picker_belongs_to_the_open_conversation() {
        let mut reader = reader(vec![message("a", 1)]);
        assert!(!reader.is_showing_labels());

        reader.toggle_labels();
        assert!(reader.is_showing_labels());
        reader.toggle_labels();
        assert!(!reader.is_showing_labels());

        // Moving on puts the row back the way it usually reads.
        reader.toggle_labels();
        let next = ConversationReader::new(reader.detail().clone());
        assert!(!next.is_showing_labels());
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
        assert!(reader.is_expanded("c", plain()));
        assert!(!reader.is_expanded("a", plain()));
        assert!(!reader.is_expanded("b", plain()));
    }

    #[test]
    fn toggling_changes_one_message_independently() {
        let mut reader = reader(vec![message("a", 10), message("b", 20), message("c", 30)]);

        reader.toggle("a");
        assert!(reader.is_expanded("a", plain()));
        assert!(reader.is_expanded("c", plain()));
        assert!(!reader.is_expanded("b", plain()));

        reader.toggle("c");
        assert!(!reader.is_expanded("c", plain()));
        assert!(reader.is_expanded("a", plain()));
    }

    #[test]
    fn quotes_start_folded_and_toggle_one_at_a_time() {
        let mut reader = reader(vec![message("a", 10), message("b", 20)]);

        assert!(!reader.is_quote_expanded("a", 0, plain()));

        reader.toggle_quote("a", 0);
        assert!(reader.is_quote_expanded("a", 0, plain()));
        // Neither the next quote of the same message nor the same position of
        // another message follows along.
        assert!(!reader.is_quote_expanded("a", 1, plain()));
        assert!(!reader.is_quote_expanded("b", 0, plain()));

        reader.toggle_quote("a", 0);
        assert!(!reader.is_quote_expanded("a", 0, plain()));
    }

    #[test]
    fn unknown_messages_cannot_be_expanded() {
        let mut reader = reader(vec![message("a", 10)]);

        reader.toggle("missing");
        reader.toggle_quote("missing", 0);

        assert!(!reader.is_expanded("missing", plain()));
        assert!(!reader.is_quote_expanded("missing", 0, plain()));
    }

    #[test]
    fn empty_detail_has_nothing_expanded() {
        let empty = reader(Vec::new());

        assert!(empty.detail().messages.is_empty());
        assert!(empty.toggled.is_empty());
    }

    #[test]
    fn asking_for_every_message_opens_the_whole_thread() {
        let reader = reader(vec![message("a", 10), message("b", 20), message("c", 30)]);
        let all = Reading {
            expand_all_messages: true,
            ..plain()
        };

        for id in ["a", "b", "c"] {
            assert!(reader.is_expanded(id, all), "{id} stayed folded");
        }
        // The same reader still answers for the settings as they were.
        assert!(!reader.is_expanded("a", plain()));
    }

    #[test]
    fn a_message_folded_by_hand_stays_that_way_when_the_setting_changes() {
        // The reader holds the difference from the settings, not the state,
        // so a change reaches the open conversation without undoing a fold.
        let mut reader = reader(vec![message("a", 10), message("b", 20)]);
        let all = Reading {
            expand_all_messages: true,
            ..plain()
        };

        reader.toggle("a");
        assert!(reader.is_expanded("a", plain()));
        assert!(!reader.is_expanded("a", all));
        assert!(reader.is_expanded("b", all));
    }

    #[test]
    fn asking_for_quoted_text_unfolds_it_without_losing_a_fold() {
        let mut reader = reader(vec![message("a", 10)]);
        let quoted = Reading {
            show_quoted_text: true,
            ..plain()
        };

        assert!(reader.is_quote_expanded("a", 0, quoted));

        reader.toggle_quote("a", 0);
        assert!(!reader.is_quote_expanded("a", 0, quoted));
        assert!(reader.is_quote_expanded("a", 0, plain()));
    }

    #[test]
    fn reader_state_starts_empty() {
        assert_eq!(ReaderState::default(), ReaderState::Empty);
        assert_eq!(ReaderState::Empty.conversation_id(), None);
    }
}
