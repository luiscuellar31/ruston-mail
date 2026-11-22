//! Answering a message: whom a reply goes to, the subjects of replies and
//! forwards, the quote under them and the attachments a forward carries.

use crate::crypto;
use crate::crypto::keys::{AddressKeys, KeyStore};
use crate::error::Result;
use crate::model::enums::message_flag;
use crate::model::message::{Message, MessageMetadata, Recipient};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Map, Value};
use std::borrow::Cow;

/// Where [`Client::reply`](crate::mail::Client::reply) sends an answer to a message.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReplyRecipients {
    /// The Reply-To addresses the message asks for, or its sender. For a
    /// message the account sent, its To recipients.
    pub to: Vec<String>,
    /// What Reply all adds: the message's other recipients, without the
    /// account's own addresses.
    pub cc: Vec<String>,
}

/// Mail the account sent, and did not also receive: a note to self is
/// answered like any other message.
fn sent_by_account(meta: &MessageMetadata) -> bool {
    message_flag::has(meta.flags, message_flag::SENT)
        && !message_flag::has(meta.flags, message_flag::RECEIVED)
        && !meta.to_list.is_empty()
}

/// The recipients of a reply to the message described by `meta` and its
/// `reply_tos`, leaving out `own` addresses (compared without case). Frontends
/// can show these before sending; [`Client::reply`](crate::mail::Client::reply) uses the same answer.
pub fn reply_recipients(
    meta: &MessageMetadata,
    reply_tos: &[Recipient],
    own: &[String],
) -> ReplyRecipients {
    let not_own = |address: &String| {
        !own.iter()
            .any(|mine| mine.trim().eq_ignore_ascii_case(address.trim()))
    };
    // Answering mail the account sent continues with the people it went to;
    // its sender and Reply-To are the account itself.
    if sent_by_account(meta) {
        return ReplyRecipients {
            to: meta.to_list.iter().map(|r| r.address.clone()).collect(),
            cc: meta
                .cc_list
                .iter()
                .map(|r| r.address.clone())
                .filter(not_own)
                .collect(),
        };
    }

    let to = if reply_tos.is_empty() {
        vec![meta.sender.address.clone()]
    } else {
        reply_tos.iter().map(|r| r.address.clone()).collect()
    };
    let cc = meta
        .to_list
        .iter()
        .chain(&meta.cc_list)
        .map(|r| r.address.clone())
        .filter(not_own)
        .collect();
    ReplyRecipients { to, cc }
}

/// Whether the message's Reply-To sends replies somewhere other than its
/// sender, which a frontend should point out. Mail the account sent
/// redirects nothing.
pub fn reply_redirected(meta: &MessageMetadata, reply_tos: &[Recipient]) -> bool {
    let sender = meta.sender.address.trim();
    !sent_by_account(meta)
        && reply_tos
            .iter()
            .any(|r| !r.address.trim().eq_ignore_ascii_case(sender))
}

pub(super) struct Answering<'a> {
    /// The original's ID.
    pub(super) parent_id: &'a str,
    /// Which [`message_action`] the answer is, so that Proton marks the
    /// original replied to or forwarded once it goes.
    pub(super) action: u8,
    /// For a forward, the original's attachments it carries.
    pub(super) carried: Option<Carried>,
}

/// Attachments a forward carries from the original: each one's ID and key
/// packets, which the keys of the address the original reached open.
pub(super) struct Carried {
    pub(super) address_id: String,
    pub(super) key_packets: Vec<(String, String)>,
}

impl Carried {
    /// The attachments' session keys sealed again to the sender's `addr`, by
    /// attachment ID, as a draft lists them. They are opened with the keys
    /// of the address the original reached, or with `addr`'s when the
    /// account no longer has that address.
    pub(super) fn rekeyed<P: proton_crypto::crypto::PGPProviderSync>(
        &self,
        provider: &P,
        keys: &KeyStore,
        addr: &AddressKeys,
    ) -> Result<Value> {
        let reached = keys.address(&self.address_id).unwrap_or(addr);
        self.key_packets
            .iter()
            .map(|(id, packets)| {
                let sealed =
                    crypto::rewrap_attachment_session_key(provider, reached, addr, packets)?;
                Ok((id.clone(), Value::from(STANDARD.encode(sealed))))
            })
            .collect::<Result<Map<String, Value>>>()
            .map(Value::Object)
    }
}

/// The subject of a reply to a message titled `subject`, as
/// [`Client::reply`](crate::mail::Client::reply) writes it.
pub fn reply_subject(subject: &str) -> String {
    with_prefix(subject, "Re: ", &["re:"])
}

/// The subject of a forward of a message titled `subject`, which may already
/// carry the "Fwd:" other clients write, as [`Client::forward`](crate::mail::Client::forward) writes it.
pub fn forward_subject(subject: &str) -> String {
    with_prefix(subject, "Fw: ", &["fw:", "fwd:"])
}

/// `subject` with `prefix` in front, unless it starts with one of `marks`,
/// given in lower case with their colon: "RE: Reunión" already answers
/// something, while "Reunión" only begins like it.
fn with_prefix(subject: &str, prefix: &str, marks: &[&str]) -> String {
    let start = subject.trim_start().to_ascii_lowercase();
    if marks.iter().any(|mark| start.starts_with(mark)) {
        subject.to_string()
    } else {
        format!("{prefix}{subject}")
    }
}

pub(super) fn quote_block(parent: &Message, body: &str, original_mime: &str, html: bool) -> String {
    let sender = &parent.meta.sender;
    let who = if sender.name.is_empty() {
        sender.address.clone()
    } else {
        format!("{} <{}>", sender.name, sender.address)
    };
    if html {
        let who = crate::html::escape_text(&who);
        let body = if crate::html::is_html_mime(original_mime) {
            Cow::Borrowed(body)
        } else {
            Cow::Owned(crate::html::escape_text(body))
        };
        format!(
            "<div class=\"protonmail_quote\">On {who} wrote:<blockquote class=\"protonmail_quote\" type=\"cite\">{body}</blockquote></div>"
        )
    } else {
        // A plain-text answer quotes what an HTML original reads as, not its
        // markup.
        let body = if crate::html::is_html_mime(original_mime) {
            Cow::Owned(crate::html::to_text(body))
        } else {
            Cow::Borrowed(body)
        };
        let quoted: String = body.lines().map(|l| format!("> {l}\n")).collect();
        format!("------- Original Message -------\nOn {who} wrote:\n{quoted}")
    }
}
