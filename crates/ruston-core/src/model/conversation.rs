//! Conversation models.

use super::message::{MessageMetadata, Recipient};
use serde::{Deserialize, Serialize};

/// A label applied to a conversation, with per-label context counts.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ConversationLabel {
    /// Label ID.
    #[serde(rename = "ID")]
    pub id: String,
    /// Number of messages in the conversation under this label.
    #[serde(default)]
    pub context_num_messages: Option<i64>,
    /// Number of unread messages in the conversation under this label.
    #[serde(default)]
    pub context_num_unread: Option<i64>,
    /// Unix timestamp of the latest message under this label.
    #[serde(default)]
    pub context_time: Option<i64>,
}

/// A conversation (message thread) with its summary metadata.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Conversation {
    /// Conversation ID.
    #[serde(rename = "ID")]
    pub id: String,
    /// Subject line.
    #[serde(default)]
    pub subject: String,
    /// Unix timestamp of the most recent message.
    #[serde(default)]
    pub time: i64,
    /// Total number of messages in the conversation.
    #[serde(default)]
    pub num_messages: i64,
    /// Number of unread messages.
    #[serde(default)]
    pub num_unread: i64,
    /// Total number of attachments across the conversation.
    #[serde(default)]
    pub num_attachments: i64,
    /// Distinct senders in the conversation.
    #[serde(default)]
    pub senders: Vec<Recipient>,
    /// Distinct recipients in the conversation.
    #[serde(default)]
    pub recipients: Vec<Recipient>,
    /// Labels applied to the conversation, with per-label counts.
    #[serde(default)]
    pub labels: Vec<ConversationLabel>,
    /// Unix timestamp at which the conversation expires, if set.
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
    fn deserialize_conversation() {
        let json = serde_json::json!({
            "ID": "c1", "Subject": "thread", "NumMessages": 3, "NumUnread": 1,
            "Senders": [{"Name":"A","Address":"a@proton.me"}],
            "Labels": [{"ID":"0"}]
        });
        let c: Conversation = serde_json::from_value(json).unwrap();
        assert_eq!(c.id, "c1");
        assert_eq!(c.num_messages, 3);
        assert_eq!(c.senders.len(), 1);
        assert_eq!(c.labels[0].id, "0");
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
