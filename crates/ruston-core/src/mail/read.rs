//! Listing, searching and reading mail: messages and conversations, each
//! body decrypted and its signature checked.

use super::Client;
use crate::api::{self, messages::ListQuery};
use crate::crypto::{self, Verdict};
use crate::error::{Error, Result};
use crate::model::enums::resolve_folder;
use crate::model::message::{Attachment, Message, MessageMetadata, Recipient};
use crate::model::{Conversation, MimeType};
use crate::transport::Doer;
use futures::{Stream, StreamExt, TryStreamExt, stream};
use jiff::civil::Date;
use std::collections::HashMap;

/// Keep conversation reads responsive without flooding Proton or buffering many bodies.
const CONVERSATION_BODY_FETCH_CONCURRENCY: usize = 3;
/// How many sender key lookups run at once when a conversation opens.
const SENDER_KEY_CONCURRENCY: usize = 3;
/// How many messages a search finds when its options give no limit.
const DEFAULT_SEARCH_LIMIT: u32 = 25;
/// How many messages the search for a reference asks for: one is the
/// answer, and more than one says the reference is ambiguous.
const REFERENCE_SEARCH_LIMIT: u32 = 20;
/// The shortest reference taken for a message ID, which also ends in `==`
/// and has no space; anything else is words to search for.
const MIN_MESSAGE_ID_LEN: usize = 60;

/// A message opened to be read: its body decrypted, its signature checked.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FullMessage {
    /// What a listing tells about it.
    pub meta: MessageMetadata,
    /// The body as it is shown: the part of a MIME body meant for reading,
    /// and sanitized when it is HTML.
    pub body: String,
    /// The MIME type of `body`, such as `text/html` or `text/plain`.
    pub mime_type: String,
    /// What checking the body's signature found.
    pub verdict: Verdict,
    /// Its attachments, those inside a PGP/MIME body included.
    pub attachments: Vec<Attachment>,
    /// The Reply-To addresses the message asks replies to go to, if any.
    pub reply_tos: Vec<Recipient>,
}

/// One message of a conversation read with
/// [`read_conversation_each`](Client::read_conversation_each).
#[derive(Debug)]
pub enum ConversationMessage {
    /// The message opened.
    Opened(FullMessage),
    /// The message downloaded but could not be opened.
    Unreadable(UnreadableMessage),
}

/// A message of a conversation that downloaded but could not be opened, for
/// example because no key the account holds decrypts it.
#[derive(Debug)]
pub struct UnreadableMessage {
    /// What a listing tells about it.
    pub meta: MessageMetadata,
    /// The Reply-To addresses the message asks replies to go to, if any.
    pub reply_tos: Vec<Recipient>,
    /// Why the message could not be opened.
    pub error: Error,
}

/// What a search on Proton's side looks for. Each field that is set narrows
/// it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SearchOpts {
    /// Text for Proton's search to find.
    pub keyword: Option<String>,
    /// Only messages from this address.
    pub from: Option<String>,
    /// Only messages to this address.
    pub to: Option<String>,
    /// Only messages whose subject holds this text.
    pub subject: Option<String>,
    /// Only messages from this day on, written `YYYY-MM-DD`, in UTC. Any
    /// other form fails the search before a request.
    pub after: Option<String>,
    /// Only messages up to this day, written `YYYY-MM-DD`, in UTC, all of
    /// that day included. Any other form fails the search before a
    /// request.
    pub before: Option<String>,
    /// Only this folder or label, by name or ID; all mail when `None`.
    pub folder: Option<String>,
    /// Only unread messages, when set.
    pub unread: bool,
    /// How many messages it finds at most; 25 when `None`.
    pub limit: Option<u32>,
}

/// Checks that `date` is a `YYYY-MM-DD` date, as [`SearchOpts`] needs, so a
/// caller can refuse it before signing in.
pub fn validate_search_date(date: &str) -> Result<()> {
    parse_date(date).map(drop)
}

/// The first and the last second a search covers, as [`SearchOpts`]'s
/// `after` and `before` name its days: whole days in UTC. A range that ends
/// before it starts is refused, since Proton would find nothing in it, which
/// reads as no mail.
pub fn search_window(
    after: Option<&str>,
    before: Option<&str>,
) -> Result<(Option<i64>, Option<i64>)> {
    let after = after.map(parse_date).transpose()?;
    let before = before.map(parse_date).transpose()?;
    if let (Some(first), Some(last)) = (after, before)
        && first > last
    {
        return Err(Error::Other(format!(
            "invalid date range: {first} is after {last}"
        )));
    }
    Ok((after.map(day_start), before.map(day_end)))
}

fn parse_date(s: &str) -> Result<Date> {
    Date::strptime("%Y-%m-%d", s.trim())
        .map_err(|_| Error::Other(format!("invalid date '{s}': expected YYYY-MM-DD")))
}

/// The first second of `day` in UTC.
fn day_start(day: Date) -> i64 {
    day.duration_since(Date::constant(1970, 1, 1)).as_secs()
}

/// The last second of `day` in UTC: the API's `End` includes its timestamp.
fn day_end(day: Date) -> i64 {
    day_start(day) + 86_399
}

/// The query for one page of `folder`, of its messages or its conversations.
fn folder_page(folder: &str, page: u32, page_size: u32, unread: bool) -> ListQuery {
    ListQuery {
        unread,
        ..ListQuery::folder_page(resolve_folder(folder), page, page_size)
    }
}

fn looks_like_id(reference: &str) -> bool {
    reference.len() >= MIN_MESSAGE_ID_LEN && reference.ends_with("==") && !reference.contains(' ')
}

/// What is read of a decrypted MIME body, and its type: every part of its HTML
/// version, in order, or of its text version when it has no HTML one. A body
/// of several parts, such as text, an image and more text, is all of them.
/// It is parsed from its bytes, so each part is decoded in its own charset.
/// The attachments inside it come along, as [`mime_attachments`] names them.
fn extract_mime(raw: &[u8]) -> (String, String, Vec<Attachment>) {
    let Some(msg) = mail_parser::MessageParser::default().parse(raw) else {
        return (
            String::from_utf8_lossy(raw).into_owned(),
            MimeType::PlainText.as_str().to_string(),
            Vec::new(),
        );
    };
    let attachments = mime_attachments(&msg);
    let html: Vec<_> = (0..msg.html_body_count())
        .filter_map(|part| msg.body_html(part))
        .collect();
    if !html.is_empty() {
        return (
            html.join("\n"),
            MimeType::Html.as_str().to_string(),
            attachments,
        );
    }
    let text: Vec<_> = (0..msg.text_body_count())
        .filter_map(|part| msg.body_text(part))
        .collect();
    if !text.is_empty() {
        return (
            text.join("\n"),
            MimeType::PlainText.as_str().to_string(),
            attachments,
        );
    }
    (
        String::from_utf8_lossy(raw).into_owned(),
        MimeType::PlainText.as_str().to_string(),
        attachments,
    )
}

/// The attachments inside a PGP/MIME body, which Proton cannot see, named by
/// their position in it: `mime-1`, `mime-2` and on. They have no key packets
/// or signature of their own; the body's signature covers them.
fn mime_attachments(msg: &mail_parser::Message) -> Vec<Attachment> {
    use mail_parser::MimeHeaders as _;
    msg.attachments()
        .enumerate()
        .map(|(index, part)| Attachment {
            id: super::attachments::mime_attachment_id(index + 1),
            name: mime_attachment_name(part, index + 1),
            size: part.contents().len() as u64,
            mime_type: part.content_type().map(|kind| match kind.subtype() {
                Some(subtype) => format!("{}/{subtype}", kind.ctype()),
                None => kind.ctype().to_owned(),
            }),
            disposition: part
                .content_disposition()
                .map(|disposition| disposition.ctype().to_ascii_lowercase()),
            headers: part
                .content_id()
                .map(|id| ("content-id".to_owned(), id.into()))
                .into_iter()
                .collect(),
            ..Attachment::default()
        })
        .collect()
}

/// The name and bytes of the attachment at `position`, from 1, inside a
/// PGP/MIME body, as [`mime_attachments`] names it.
pub(super) fn mime_attachment(raw: &[u8], position: usize) -> Option<(String, Vec<u8>)> {
    let msg = mail_parser::MessageParser::default().parse(raw)?;
    let part = msg.attachment(u32::try_from(position.checked_sub(1)?).ok()?)?;
    Some((
        mime_attachment_name(part, position),
        part.contents().to_vec(),
    ))
}

/// A part's file name, or one made from its position when it has none.
fn mime_attachment_name(part: &mail_parser::MessagePart, position: usize) -> String {
    use mail_parser::MimeHeaders as _;
    part.attachment_name()
        .filter(|name| !name.trim().is_empty())
        .map_or_else(|| format!("attachment-{position}"), str::to_owned)
}

impl Client {
    /// One page of a folder's messages, newest first, with how many it holds:
    /// unread ones only when `unread` is set.
    pub async fn list_messages(
        &self,
        folder: &str,
        page: u32,
        page_size: u32,
        unread: bool,
    ) -> Result<(u32, Vec<MessageMetadata>)> {
        let q = folder_page(folder, page, page_size, unread);
        api::messages::list_messages(self.http(), &q).await
    }

    /// One page of a folder's conversations, newest first, with how many it
    /// holds: unread ones only when `unread` is set.
    pub async fn list_conversations(
        &self,
        folder: &str,
        page: u32,
        page_size: u32,
        unread: bool,
    ) -> Result<(u32, Vec<Conversation>)> {
        let q = folder_page(folder, page, page_size, unread);
        api::conversations::list_conversations(self.http(), &q).await
    }

    fn search_query(&self, opts: &SearchOpts) -> Result<ListQuery> {
        let (begin, end) = search_window(opts.after.as_deref(), opts.before.as_deref())?;
        let folder = opts.folder.as_deref().unwrap_or("all");
        Ok(ListQuery {
            label_id: Some(resolve_folder(folder)),
            page_size: Some(opts.limit.unwrap_or(DEFAULT_SEARCH_LIMIT)),
            unread: opts.unread,
            keyword: opts.keyword.clone(),
            from: opts.from.clone(),
            to: opts.to.clone(),
            subject: opts.subject.clone(),
            begin,
            end,
            ..Default::default()
        })
    }

    /// The messages Proton's search finds for `opts`.
    pub async fn search_messages(&self, opts: &SearchOpts) -> Result<Vec<MessageMetadata>> {
        let q = self.search_query(opts)?;
        Ok(api::messages::list_messages(self.http(), &q).await?.1)
    }

    /// The conversations Proton's search finds for `opts`.
    pub async fn search_conversations(&self, opts: &SearchOpts) -> Result<Vec<Conversation>> {
        let q = self.search_query(opts)?;
        Ok(api::conversations::list_conversations(self.http(), &q)
            .await?
            .1)
    }

    /// Search one zero-based page of conversations, returning the server total.
    pub async fn search_conversations_page(
        &self,
        opts: &SearchOpts,
        page: u32,
        page_size: u32,
    ) -> Result<(u32, Vec<Conversation>)> {
        let mut q = self.search_query(opts)?;
        q.page = Some(page);
        q.page_size = Some(page_size);
        api::conversations::list_conversations(self.http(), &q).await
    }

    /// The ID of the message `reference` names: the reference itself when it
    /// reads as an ID, or else that of the one message a search of all mail
    /// for it finds. Finding none, or several, is an error.
    pub async fn resolve_ref(&self, reference: &str) -> Result<String> {
        if looks_like_id(reference) {
            return Ok(reference.to_string());
        }
        Ok(self.search_ref(reference).await?.id)
    }

    /// Resolve a reference as [`resolve_ref`](Self::resolve_ref) does, to the
    /// message's metadata. A search hit already carries it, so only an exact
    /// ID is fetched.
    pub async fn resolve_ref_metadata(&self, reference: &str) -> Result<MessageMetadata> {
        if looks_like_id(reference) {
            return self.message_metadata(reference).await;
        }
        self.search_ref(reference).await
    }

    /// The one message a search of all mail for `reference` finds.
    async fn search_ref(&self, reference: &str) -> Result<MessageMetadata> {
        let mut hits = self
            .search_messages(&SearchOpts {
                keyword: Some(reference.to_string()),
                folder: Some("all".into()),
                limit: Some(REFERENCE_SEARCH_LIMIT),
                ..Default::default()
            })
            .await?;
        match hits.len() {
            0 => Err(Error::NotFound {
                kind: "message".into(),
            }),
            1 => Ok(hits.remove(0)),
            n => Err(Error::Ambiguous(n)),
        }
    }

    /// Decrypts `message`, checking its signature against its sender's keys.
    /// A failed lookup leaves the body unverified.
    pub(super) async fn open_message(&self, message: Message) -> Result<FullMessage> {
        let sender_pubs = self.verifier_keys(&message.meta.sender.address).await;
        self.open_message_with(message, sender_pubs).await
    }

    /// The sender's keys to check a body's signature with. A failed lookup
    /// leaves none: the body still opens, without claiming a verified
    /// signature.
    pub(super) async fn verifier_keys(&self, sender: &str) -> Vec<String> {
        match self.sender_pubkeys(sender).await {
            Ok(keys) => keys,
            Err(e) => {
                tracing::warn!(target: "ruston_core::mail", error = %e, "sender key lookup failed — body left unverified");
                Vec::new()
            }
        }
    }

    /// Decrypts `message`, checking its signature against `sender_pubs`; with
    /// none, the body opens as unverified. Decrypting, parsing and
    /// sanitizing run off the async workers.
    pub(super) async fn open_message_with(
        &self,
        message: Message,
        sender_pubs: Vec<String>,
    ) -> Result<FullMessage> {
        let addr = self
            .keys_for_message(&message.meta.address_id)
            .ok_or_else(|| Error::Crypto("no address key for message".into()))?
            .clone();
        super::off_thread(move || open_body(&addr, message, &sender_pubs)).await
    }

    /// Opens `message` or, when it cannot be opened, returns its headers and
    /// the reason as [`ConversationMessage::Unreadable`]. A failed key lookup
    /// only leaves a body unverified, so a failure to open is about this
    /// message alone.
    ///
    /// `sender_pubs` are the sender's keys when they are already known, even
    /// to be none; without them they are asked for.
    async fn open_or_unreadable(
        &self,
        message: Message,
        sender_pubs: Option<Vec<String>>,
    ) -> ConversationMessage {
        let meta = message.meta.clone();
        let reply_tos = message.reply_tos.clone();
        let opened = match sender_pubs {
            Some(sender_pubs) => self.open_message_with(message, sender_pubs).await,
            None => self.open_message(message).await,
        };
        match opened {
            Ok(message) => ConversationMessage::Opened(message),
            Err(error) => {
                tracing::debug!(target: "ruston_core::mail", message_id = %meta.id, error = %error, "message could not be opened");
                ConversationMessage::Unreadable(UnreadableMessage {
                    meta,
                    reply_tos,
                    error,
                })
            }
        }
    }

    /// The keys a message that reached `address_id` opens with: that
    /// address's, or the primary address's when the account holds none for
    /// it. Each caller says what it could not open without them.
    pub(super) fn keys_for_message(&self, address_id: &str) -> Option<&crypto::keys::AddressKeys> {
        self.keys()
            .address(address_id)
            .or_else(|| self.keys().primary_address())
    }

    /// The keys of every distinct sender, asked for at once, a few at a time,
    /// so no body waits for another sender's lookup. Each body takes its
    /// sender's from here, none included: most senders outside Proton have
    /// none, and are not asked about again for each of their messages. A
    /// lookup that fails is left out, to be tried again, and reported, by the
    /// body that needs it.
    async fn sender_keys_of(&self, messages: &[Message]) -> HashMap<String, Vec<String>> {
        // Each address as its sender wrote it, under the one spelling the
        // bodies look it up by.
        let mut senders: Vec<(String, String)> = Vec::new();
        for message in messages {
            let written = &message.meta.sender.address;
            let sender = written.to_ascii_lowercase();
            if !sender.is_empty() && senders.iter().all(|(known, _)| *known != sender) {
                senders.push((sender, written.clone()));
            }
        }
        stream::iter(senders)
            .map(|(sender, written)| async move {
                let keys = self.sender_pubkeys(&written).await.ok()?;
                Some((sender, keys))
            })
            .buffer_unordered(SENDER_KEY_CONCURRENCY)
            .filter_map(std::future::ready)
            .collect()
            .await
    }

    /// Fetch one message's metadata (subject, sender, labels) without
    /// decrypting its body, e.g. to show what an action is about to change.
    pub async fn message_metadata(&self, id: &str) -> Result<MessageMetadata> {
        Ok(api::messages::get_message(self.http(), id).await?.meta)
    }

    /// Downloads one message and opens it to be read, checking its signature
    /// against its sender's keys.
    pub async fn read_message(&self, id: &str) -> Result<FullMessage> {
        tracing::debug!(target: "ruston_core::mail", message_id = %id, "read_message: fetching + decrypting");
        let msg = api::messages::get_message(self.http(), id).await?;
        self.open_message(msg).await
    }

    /// Fetch and open one message, as
    /// [`read_conversation_each`](Self::read_conversation_each) does each of
    /// a conversation's: one that downloads but cannot be opened comes back
    /// as [`ConversationMessage::Unreadable`], with its headers, rather than
    /// as an error. A failed download still fails.
    pub async fn read_message_or_unreadable(&self, id: &str) -> Result<ConversationMessage> {
        let msg = api::messages::get_message(self.http(), id).await?;
        Ok(self.open_or_unreadable(msg, None).await)
    }

    /// Fetch and decrypt a message for a caller that keeps only its text: the
    /// sender's keys are not looked up, and the verdict is always unverified.
    pub(super) async fn read_message_unverified(&self, id: &str) -> Result<FullMessage> {
        let msg = api::messages::get_message(self.http(), id).await?;
        self.open_message_with(msg, Vec::new()).await
    }

    /// Downloads a conversation and opens each of its messages, oldest first.
    /// Fails if any message cannot be opened; see
    /// [`read_conversation_each`](Self::read_conversation_each) to keep the others.
    pub async fn read_conversation(&self, id: &str) -> Result<(Conversation, Vec<FullMessage>)> {
        let (conv, messages) = self.read_conversation_each(id).await?;
        let messages = messages
            .into_iter()
            .map(|message| match message {
                ConversationMessage::Opened(message) => Ok(message),
                ConversationMessage::Unreadable(unreadable) => Err(unreadable.error),
            })
            .collect::<Result<_>>()?;
        Ok((conv, messages))
    }

    /// Fetch a conversation and decrypt each of its messages (oldest first).
    /// A message that downloads but cannot be opened comes back as
    /// [`ConversationMessage::Unreadable`], and the rest still open. A failed
    /// request still fails the whole read.
    pub async fn read_conversation_each(
        &self,
        id: &str,
    ) -> Result<(Conversation, Vec<ConversationMessage>)> {
        let (conv, mut msgs) = api::conversations::get_conversation(self.http(), id).await?;
        msgs.sort_by_key(|m| m.meta.time);
        // The senders are known now: fetch their keys together up front.
        let keys = &self.sender_keys_of(&msgs).await;
        // Bodies download and decrypt at the same time, three of each at
        // most, and still come out in conversation order.
        let out = fetch_conversation_bodies(self.http(), msgs)
            .map_ok(|message| async move {
                let sender = message.meta.sender.address.to_ascii_lowercase();
                let sender_pubs = keys.get(&sender).cloned();
                Ok::<_, Error>(self.open_or_unreadable(message, sender_pubs).await)
            })
            .try_buffered(CONVERSATION_BODY_FETCH_CONCURRENCY)
            .try_collect()
            .await?;
        Ok((conv, out))
    }

    /// List the metadata of every message in a conversation (oldest first),
    /// without fetching message bodies or decrypting anything.
    pub async fn conversation_messages(&self, id: &str) -> Result<Vec<MessageMetadata>> {
        let (_, messages) = api::conversations::get_conversation(self.http(), id).await?;
        Ok(metadata_oldest_first(messages))
    }
}

/// Decrypts, parses and sanitizes one message: CPU work, kept off the async
/// workers by the callers.
fn open_body(
    addr: &crypto::keys::AddressKeys,
    m: Message,
    sender_pubs: &[String],
) -> Result<FullMessage> {
    let provider = crypto::provider();
    let (body, verdict) =
        crypto::message::decrypt_body_bytes(&provider, addr, sender_pubs, &m.body)?;
    let mut attachments = m.attachments;
    let (body, mime_type) = if m.mime_type.starts_with("multipart/") {
        let (body, mime_type, inside) = extract_mime(&body);
        attachments.extend(inside);
        (body, mime_type)
    } else {
        let mime_type = if m.mime_type.is_empty() {
            MimeType::PlainText.as_str().into()
        } else {
            m.mime_type
        };
        (crypto::message::utf8_text(body), mime_type)
    };
    // Both frontends show what comes back, so an HTML body is made safe here,
    // once for either.
    let body = if crate::html::is_html_mime(&mime_type) {
        crate::html::sanitize(&body)
    } else {
        body
    };
    Ok(FullMessage {
        meta: m.meta,
        body,
        mime_type,
        verdict,
        attachments,
        reply_tos: m.reply_tos,
    })
}

fn fetch_conversation_bodies<D: Doer>(
    http: &D,
    messages: Vec<Message>,
) -> impl Stream<Item = Result<Message>> + '_ {
    stream::iter(messages)
        .map(move |message| async move {
            // The conversation response may already include a body.
            if message.body.is_empty() {
                api::messages::get_message(http, &message.meta.id).await
            } else {
                Ok(message)
            }
        })
        // buffered preserves conversation order while limiting in-flight requests.
        .buffered(CONVERSATION_BODY_FETCH_CONCURRENCY)
}

fn metadata_oldest_first(mut messages: Vec<Message>) -> Vec<MessageMetadata> {
    messages.sort_by_key(|m| m.meta.time);
    messages.into_iter().map(|m| m.meta).collect()
}

#[cfg(test)]
mod tests;
