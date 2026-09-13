use std::collections::HashSet;

use crate::mail::{ConversationDetail, Folder, MailboxError, MessageBody};
use crate::settings::Reading;

/// How much of a message its collapsed header stands in for.
const PREVIEW_WORDS: usize = 40;

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
    /// The line each collapsed message shows, in the order `detail.messages`
    /// are in.
    ///
    /// Each is derived from a body that cannot change while the conversation
    /// is open, so they are settled here rather than in the drawing:
    /// flattening a message every frame cost more the longer the message was,
    /// which is backwards for something only shown when the message is folded
    /// away.
    previews: Vec<String>,
}

impl ConversationReader {
    /// Orders messages oldest first. Which of them start open is left to the
    /// settings, read through [`Self::is_expanded`].
    pub fn new(mut detail: ConversationDetail) -> Self {
        detail.messages.sort_by_key(|message| message.time);
        let previews = detail
            .messages
            .iter()
            .map(|message| preview(&message.body))
            .collect();

        Self {
            detail,
            toggled: HashSet::new(),
            toggled_quotes: HashSet::new(),
            showing_labels: false,
            previews,
        }
    }

    /// The line that stands in for a message while it is folded away.
    pub fn preview(&self, message_id: &str) -> &str {
        self.detail
            .messages
            .iter()
            .position(|message| message.id == message_id)
            .and_then(|index| self.previews.get(index))
            .map_or("", String::as_str)
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

/// The opening words of a message, on one line.
///
/// A rich body is flattened before it is split into words, not after:
/// splitting each span on its own strands the punctuation that follows a
/// styled run, which read as "confirmed . The" in the collapsed header.
fn preview(body: &MessageBody) -> String {
    let text = match body {
        MessageBody::PlainText(content) => content.clone(),
        MessageBody::Rich(rich) => rich.plain_text(),
    };
    text.split_whitespace()
        .take(PREVIEW_WORDS)
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail::{
        BlockKind, MailAddress, MailMessage, MessageBody, RichBlock, RichBody, RichSpan,
    };

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

    fn saying(id: &str, body: MessageBody) -> MailMessage {
        MailMessage {
            body,
            ..message(id, 10)
        }
    }

    #[test]
    fn a_collapsed_message_is_summarised_in_one_bounded_line() {
        let rich = RichBody {
            blocks: ["Hello  there", "again"]
                .into_iter()
                .map(|text| RichBlock {
                    kind: BlockKind::Paragraph(vec![RichSpan {
                        text: text.to_owned(),
                        ..RichSpan::default()
                    }]),
                    quote_depth: 0,
                })
                .collect(),
        };
        let reader = reader(vec![
            saying(
                "plain",
                MessageBody::PlainText("Hi Alex,\n\nFirst line.\nSecond line.".into()),
            ),
            saying("long", MessageBody::PlainText("word ".repeat(100))),
            saying("rich", MessageBody::Rich(rich)),
        ]);

        assert_eq!(reader.preview("plain"), "Hi Alex, First line. Second line.");
        assert_eq!(reader.preview("long").split(' ').count(), PREVIEW_WORDS);
        // Blocks are joined, and the words inside one keep their spacing.
        assert_eq!(reader.preview("rich"), "Hello there again");
        // A message the conversation does not carry has nothing to show.
        assert_eq!(reader.preview("missing"), "");
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
