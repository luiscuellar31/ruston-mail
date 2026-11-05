//! Conversations: the threads Proton groups messages in, and the ones a
//! search of the local index makes of what it finds.

use super::message::{MessageMetadata, Recipient};
use serde::{Deserialize, Serialize};

/// One of a conversation's labels, with how many of its messages carry it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ConversationLabel {
    /// The label's ID.
    #[serde(rename = "ID")]
    pub id: String,
    /// How many of the conversation's messages carry the label.
    #[serde(default)]
    pub context_num_messages: Option<i64>,
    /// How many of those are unread.
    #[serde(default)]
    pub context_num_unread: Option<i64>,
    /// When the newest of them came, in Unix seconds.
    #[serde(default)]
    pub context_time: Option<i64>,
}

/// A conversation, summed up from its messages.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Conversation {
    /// The conversation's ID.
    #[serde(rename = "ID")]
    pub id: String,
    /// Its subject.
    #[serde(default)]
    pub subject: String,
    /// When its newest message came, in Unix seconds.
    #[serde(default)]
    pub time: i64,
    /// How many messages it has.
    #[serde(default)]
    pub num_messages: i64,
    /// How many of them are unread.
    #[serde(default)]
    pub num_unread: i64,
    /// How many attachments its messages have between them.
    #[serde(default)]
    pub num_attachments: i64,
    /// Everyone who wrote in it, each once.
    #[serde(default)]
    pub senders: Vec<Recipient>,
    /// Everyone it went to, each once.
    #[serde(default)]
    pub recipients: Vec<Recipient>,
    /// Its labels, each with how many of its messages carry it.
    #[serde(default)]
    pub labels: Vec<ConversationLabel>,
    /// When it expires, in Unix seconds, if it does.
    #[serde(default)]
    pub expiration_time: Option<i64>,
}

/// The conversations `messages` belong to, in the order the first message of
/// each comes, made of what those messages say: a search of the local index
/// finds messages, and offline there is nothing else to list them by.
pub fn conversations_of(messages: Vec<MessageMetadata>) -> Vec<Conversation> {
    let mut conversations: Vec<Conversation> = Vec::new();
    for message in messages {
        if message.conversation_id.is_empty() {
            continue;
        }
        let at = conversations
            .iter()
            .position(|conversation| conversation.id == message.conversation_id)
            .unwrap_or_else(|| {
                conversations.push(Conversation {
                    id: message.conversation_id.clone(),
                    subject: message.subject.clone(),
                    ..Conversation::default()
                });
                conversations.len() - 1
            });
        let conversation = &mut conversations[at];
        let unread = i64::from(message.unread > 0);
        conversation.num_messages += 1;
        conversation.num_unread += unread;
        conversation.num_attachments += message.num_attachments;
        conversation.time = conversation.time.max(message.time);
        if !conversation
            .senders
            .iter()
            .any(|sender| sender.address == message.sender.address)
        {
            conversation.senders.push(message.sender.clone());
        }
        for id in &message.label_ids {
            match conversation.labels.iter_mut().find(|label| &label.id == id) {
                Some(label) => {
                    label.context_num_messages = Some(label.context_num_messages.unwrap_or(0) + 1);
                    label.context_num_unread = Some(label.context_num_unread.unwrap_or(0) + unread);
                }
                None => conversation.labels.push(ConversationLabel {
                    id: id.clone(),
                    context_num_messages: Some(1),
                    context_num_unread: Some(unread),
                    context_time: Some(message.time),
                }),
            }
        }
    }
    conversations
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_conversation_reads_its_counts_and_those_of_each_label() {
        let conversation: Conversation = serde_json::from_value(serde_json::json!({
            "ID": "conv-3",
            "Subject": "Trip plans",
            "NumMessages": 3,
            "NumUnread": 1,
            "Senders": [{"Name": "Ana", "Address": "ana@proton.me"}],
            "Labels": [
                {"ID": "0", "ContextNumMessages": 2, "ContextNumUnread": 1},
                {"ID": "10"}
            ]
        }))
        .unwrap();

        assert_eq!(conversation.id, "conv-3");
        assert_eq!((conversation.num_messages, conversation.num_unread), (3, 1));
        let [sender] = conversation.senders.as_slice() else {
            panic!("one sender, got {:?}", conversation.senders);
        };
        assert_eq!(sender.address, "ana@proton.me");
        let labels: Vec<_> = conversation
            .labels
            .iter()
            .map(|label| (label.id.as_str(), label.context_num_messages))
            .collect();
        assert_eq!(labels, [("0", Some(2)), ("10", None)]);
        // What Proton leaves out reads as nothing.
        assert!(conversation.recipients.is_empty());
        assert_eq!(conversation.expiration_time, None);
    }

    fn message(id: &str, conversation: &str, sender: &str, unread: i64) -> MessageMetadata {
        MessageMetadata {
            id: id.into(),
            conversation_id: conversation.into(),
            subject: format!("About {conversation}"),
            sender: Recipient::new(sender),
            unread,
            time: 1_700_000_000 + id.len() as i64,
            label_ids: vec!["0".into()],
            ..MessageMetadata::default()
        }
    }

    #[test]
    fn messages_found_make_their_conversations_in_the_order_found() {
        let messages = vec![
            message("m1", "c-b", "alex@example.test", 1),
            message("m2", "c-a", "sam@example.test", 0),
            message("m-three", "c-b", "sam@example.test", 0),
            message("m4", "", "lost@example.test", 1),
        ];
        let conversations = conversations_of(messages);
        let ids: Vec<&str> = conversations.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["c-b", "c-a"]);
        let first = &conversations[0];
        assert_eq!((first.num_messages, first.num_unread), (2, 1));
        assert_eq!(first.time, 1_700_000_007);
        assert_eq!(first.senders.len(), 2);
        assert_eq!(first.labels[0].context_num_unread, Some(1));
        assert_eq!(first.subject, "About c-b");
    }
}
