//! What the reader shows of a conversation: each message's headers, its
//! signature's verdict and its body as `conversations read` shows it, with
//! every line safe to draw. It is built once, in the task that loads the
//! conversation, since turning a large HTML body into text takes a while.

use crate::render;
use crate::terminal::{screen_line, screen_text};
use ruston_core::{Conversation, ConversationMessage, MessageMetadata};

/// What stands between two messages.
const SEPARATOR: &str = "────────────────────────";

/// A conversation as the reader shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Reading {
    pub(crate) subject: String,
    /// Its lines, before the pane wraps them to its width.
    pub(crate) lines: Vec<String>,
}

impl Reading {
    pub(crate) fn new(conversation: &Conversation, messages: &[ConversationMessage]) -> Self {
        let mut lines = Vec::new();
        for (index, message) in messages.iter().enumerate() {
            if index > 0 {
                lines.extend([String::new(), SEPARATOR.to_owned(), String::new()]);
            }
            match message {
                ConversationMessage::Opened(message) => {
                    headers(
                        &mut lines,
                        &message.meta,
                        render::verdict_name(message.verdict),
                    );
                    lines.push(String::new());
                    let body = screen_text(&render::thread_body(message));
                    lines.extend(body.lines().map(str::to_owned));
                }
                ConversationMessage::Unreadable(message) => {
                    headers(&mut lines, &message.meta, "not decrypted");
                    lines.push(String::new());
                    lines.push(screen_line(&format!(
                        "(This message could not be decrypted: {})",
                        message.error
                    )));
                }
            }
        }
        Self {
            subject: screen_line(render::subject_or_empty(&conversation.subject)),
            lines,
        }
    }
}

/// The headers of a message, as `conversations read` names them.
fn headers(lines: &mut Vec<String>, meta: &MessageMetadata, verdict: &str) {
    lines.push(screen_line(&format!(
        "From:    {}",
        render::fmt_addr(&meta.sender)
    )));
    lines.push(screen_line(&format!(
        "To:      {}",
        render::fmt_addrs(&meta.to_list)
    )));
    if !meta.cc_list.is_empty() {
        lines.push(screen_line(&format!(
            "Cc:      {}",
            render::fmt_addrs(&meta.cc_list)
        )));
    }
    lines.push(format!("Date:    {}", render::fmt_time(meta.time)));
    lines.push(format!("Verdict: {verdict}"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use ruston_core::{Error, FullMessage, Recipient, UnreadableMessage, Verdict};

    fn meta(sender: &str) -> MessageMetadata {
        MessageMetadata {
            sender: Recipient::new(sender),
            to_list: vec![Recipient::new("me@example.test")],
            time: 1_714_000_000,
            ..MessageMetadata::default()
        }
    }

    #[test]
    fn each_message_shows_its_headers_verdict_and_body_safely() {
        let conversation = Conversation {
            subject: "Plans\x1b[2J\nfor May".into(),
            ..Conversation::default()
        };
        let messages = [
            ConversationMessage::Opened(FullMessage {
                meta: meta("alex@example.test"),
                body: "Hi,\r\n\tsee you\x1b]52;c;payload\x07 there".into(),
                mime_type: "text/plain".into(),
                verdict: Verdict::Verified,
                attachments: Vec::new(),
                reply_tos: Vec::new(),
            }),
            ConversationMessage::Unreadable(UnreadableMessage {
                meta: meta("sam@example.test"),
                reply_tos: Vec::new(),
                error: Error::Other("no key opens it".into()),
            }),
        ];

        let reading = Reading::new(&conversation, &messages);
        assert_eq!(reading.subject, "Plans[2J for May");
        let text = reading.lines.join("\n");
        assert!(
            text.chars().all(|c| !c.is_control() || c == '\n'),
            "{text:?}"
        );
        for expected in [
            "From:    alex@example.test",
            "To:      me@example.test",
            "Date:    2024-04-24 23:06:40 UTC",
            "Verdict: verified",
            "Hi,\n    see you",
            SEPARATOR,
            "From:    sam@example.test",
            "Verdict: not decrypted",
            "(This message could not be decrypted: no key opens it)",
        ] {
            assert!(text.contains(expected), "{expected:?} missing:\n{text}");
        }
    }
}
