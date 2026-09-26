//! Reading: list / search / read messages and conversations (with decryption).

use super::Client;
use crate::api::{self, messages::ListQuery};
use crate::crypto::{self, Verdict};
use crate::error::{Error, Result};
use crate::model::Conversation;
use crate::model::enums::resolve_folder;
use crate::model::message::{Attachment, Message, MessageMetadata};
use crate::transport::Doer;
use futures::{Stream, StreamExt, TryStreamExt, stream};

/// Keep conversation reads responsive without flooding Proton or buffering many bodies.
const CONVERSATION_BODY_FETCH_CONCURRENCY: usize = 3;

/// A decrypted message ready for display.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FullMessage {
    /// Message metadata (headers, flags, labels).
    pub meta: MessageMetadata,
    /// Decrypted body content.
    pub body: String,
    /// MIME type of the body (`text/html` or `text/plain`).
    pub mime_type: String,
    /// Signature-verification verdict for the body.
    pub verdict: Verdict,
    /// The message's attachments.
    pub attachments: Vec<Attachment>,
}

/// Search options.
#[derive(Debug, Default, Clone)]
pub struct SearchOpts {
    /// Free-text keyword to match.
    pub keyword: Option<String>,
    /// Filter by sender address.
    pub from: Option<String>,
    /// Filter by recipient address.
    pub to: Option<String>,
    /// Filter by subject text.
    pub subject: Option<String>,
    /// Only messages on or after this date (`YYYY-MM-DD`).
    pub after: Option<String>,
    /// Only messages on or before this date (`YYYY-MM-DD`).
    pub before: Option<String>,
    /// Restrict to this folder (defaults to all mail).
    pub folder: Option<String>,
    /// Only unread messages.
    pub unread: bool,
    /// Maximum number of results to return.
    pub limit: Option<u32>,
}

fn parse_date(s: &str) -> Option<i64> {
    let d = chrono::NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok()?;
    Some(d.and_hms_opt(0, 0, 0)?.and_utc().timestamp())
}

fn looks_like_id(r: &str) -> bool {
    r.len() >= 60 && r.ends_with("==") && !r.contains(' ')
}

/// Extract the display part from a decrypted MIME body.
fn extract_mime(raw: &str) -> (String, String) {
    let parsed = mail_parser::MessageParser::default().parse(raw.as_bytes());
    if let Some(msg) = parsed {
        if let Some(html) = msg.body_html(0) {
            return (html.into_owned(), "text/html".to_string());
        }
        if let Some(text) = msg.body_text(0) {
            return (text.into_owned(), "text/plain".to_string());
        }
    }
    (raw.to_string(), "text/plain".to_string())
}

impl Client {
    /// List a folder's messages (newest first), returning the total count and the page.
    pub async fn list_messages(
        &self,
        folder: &str,
        page: u32,
        page_size: u32,
        unread: bool,
    ) -> Result<(u32, Vec<MessageMetadata>)> {
        let q = ListQuery {
            label_id: Some(resolve_folder(folder)),
            page: Some(page),
            page_size: Some(page_size),
            unread,
            ..Default::default()
        };
        api::messages::list_messages(self.http(), &q).await
    }

    /// List a folder's conversations (newest first), returning the total count and the page.
    pub async fn list_conversations(
        &self,
        folder: &str,
        page: u32,
        page_size: u32,
        unread: bool,
    ) -> Result<(u32, Vec<Conversation>)> {
        let q = ListQuery {
            label_id: Some(resolve_folder(folder)),
            page: Some(page),
            page_size: Some(page_size),
            unread,
            ..Default::default()
        };
        api::conversations::list_conversations(self.http(), &q).await
    }

    fn search_query(&self, opts: &SearchOpts) -> ListQuery {
        ListQuery {
            label_id: Some(resolve_folder(opts.folder.as_deref().unwrap_or("all"))),
            page_size: Some(opts.limit.unwrap_or(25)),
            unread: opts.unread,
            keyword: opts.keyword.clone(),
            from: opts.from.clone(),
            to: opts.to.clone(),
            subject: opts.subject.clone(),
            begin: opts.after.as_deref().and_then(parse_date),
            end: opts.before.as_deref().and_then(parse_date),
            ..Default::default()
        }
    }

    /// Search messages server-side using the given criteria.
    pub async fn search_messages(&self, opts: &SearchOpts) -> Result<Vec<MessageMetadata>> {
        let q = self.search_query(opts);
        Ok(api::messages::list_messages(self.http(), &q).await?.1)
    }

    /// Search conversations server-side using the given criteria.
    pub async fn search_conversations(&self, opts: &SearchOpts) -> Result<Vec<Conversation>> {
        let q = self.search_query(opts);
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
        let mut q = self.search_query(opts);
        q.page = Some(page);
        q.page_size = Some(page_size);
        api::conversations::list_conversations(self.http(), &q).await
    }

    /// Resolve a free-text reference to a message ID (exact ID, or unique search hit).
    pub async fn resolve_ref(&self, r: &str) -> Result<String> {
        if looks_like_id(r) {
            return Ok(r.to_string());
        }
        let hits = self
            .search_messages(&SearchOpts {
                keyword: Some(r.to_string()),
                folder: Some("all".into()),
                limit: Some(20),
                ..Default::default()
            })
            .await?;
        match hits.len() {
            0 => Err(Error::NotFound {
                kind: "message".into(),
            }),
            1 => Ok(hits[0].id.clone()),
            n => Err(Error::Ambiguous(n)),
        }
    }

    pub(super) async fn decrypt_message(&self, m: &Message) -> Result<FullMessage> {
        let provider = crypto::provider();
        let addr = self
            .keys()
            .address(&m.meta.address_id)
            .or_else(|| self.keys().primary_address())
            .ok_or_else(|| Error::Crypto("no address key for message".into()))?;
        let sender_pubs = self.sender_pubkeys(&m.meta.sender.address).await;
        let (body, verdict) = crypto::decrypt_body(&provider, addr, &sender_pubs, &m.body)?;
        let (body, mime_type) = if m.mime_type.starts_with("multipart/") {
            extract_mime(&body)
        } else {
            (
                body,
                if m.mime_type.is_empty() {
                    "text/plain".into()
                } else {
                    m.mime_type.clone()
                },
            )
        };
        // Sanitize HTML bodies (strip scripts / active content) for safe rendering.
        let body = if crate::html::is_html_mime(&mime_type) {
            crate::html::sanitize(&body)
        } else {
            body
        };
        Ok(FullMessage {
            meta: m.meta.clone(),
            body,
            mime_type,
            verdict,
            attachments: m.attachments.clone(),
        })
    }

    /// Fetch and decrypt a single message for display.
    pub async fn read_message(&self, id: &str) -> Result<FullMessage> {
        tracing::debug!(target: "ruston_core::mail", message_id = %id, "read_message: fetching + decrypting");
        let msg = api::messages::get_message(self.http(), id).await?;
        self.decrypt_message(&msg).await
    }

    /// Fetch a conversation and decrypt all of its messages (oldest first).
    pub async fn read_conversation(&self, id: &str) -> Result<(Conversation, Vec<FullMessage>)> {
        let (conv, mut msgs) = api::conversations::get_conversation(self.http(), id).await?;
        msgs.sort_by_key(|m| m.meta.time);
        let mut out = Vec::with_capacity(msgs.len());
        let fetched = fetch_conversation_bodies(self.http(), msgs);
        futures::pin_mut!(fetched);
        while let Some(full) = fetched.try_next().await? {
            out.push(self.decrypt_message(&full).await?);
        }
        Ok((conv, out))
    }

    /// List the metadata of every message in a conversation (oldest first),
    /// without fetching message bodies or decrypting anything.
    pub async fn conversation_messages(&self, id: &str) -> Result<Vec<MessageMetadata>> {
        let (_, messages) = api::conversations::get_conversation(self.http(), id).await?;
        Ok(metadata_oldest_first(messages))
    }
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
mod tests {
    use super::*;
    use crate::transport::{Request, Response};
    use serde::de::DeserializeOwned;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use tokio::sync::Semaphore;

    struct GatedDoer {
        started: AtomicUsize,
        completed: AtomicUsize,
        active: AtomicUsize,
        peak: AtomicUsize,
        gates: Vec<Semaphore>,
        fail_on: Option<usize>,
    }

    impl GatedDoer {
        fn new(gate_count: usize, fail_on: Option<usize>) -> Self {
            Self {
                started: AtomicUsize::new(0),
                completed: AtomicUsize::new(0),
                active: AtomicUsize::new(0),
                peak: AtomicUsize::new(0),
                gates: (0..gate_count).map(|_| Semaphore::new(0)).collect(),
                fail_on,
            }
        }

        async fn wait_for(counter: &AtomicUsize, count: usize) {
            tokio::time::timeout(Duration::from_secs(2), async {
                while counter.load(Ordering::SeqCst) < count {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            })
            .await
            .expect("body fetches did not make progress");
        }
    }

    #[async_trait::async_trait]
    impl Doer for GatedDoer {
        async fn do_raw(&self, _req: Request) -> Result<Response> {
            unreachable!("message lookup uses decode")
        }

        async fn decode<T: DeserializeOwned>(&self, req: Request) -> Result<T> {
            let id = req.path.rsplit('/').next().expect("message ID");
            let index: usize = id
                .strip_prefix('m')
                .expect("message ID prefix")
                .parse()
                .expect("message index");
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(active, Ordering::SeqCst);
            self.started.fetch_add(1, Ordering::SeqCst);

            let permit = self.gates[index].acquire().await.expect("open gate");
            permit.forget();
            self.active.fetch_sub(1, Ordering::SeqCst);
            self.completed.fetch_add(1, Ordering::SeqCst);
            if self.fail_on == Some(index) {
                return Err(Error::Other("body fetch failed".into()));
            }
            Ok(serde_json::from_value(serde_json::json!({
                "Message": { "ID": id }
            }))?)
        }
    }

    fn messages(count: usize, last_body_present: bool) -> Vec<Message> {
        (0..count)
            .map(|index| Message {
                meta: MessageMetadata {
                    id: format!("m{index}"),
                    ..Default::default()
                },
                body: if last_body_present && index == count - 1 {
                    "already present".into()
                } else {
                    String::new()
                },
                ..Default::default()
            })
            .collect()
    }

    #[test]
    fn date_parsing() {
        assert!(parse_date("2024-01-15").is_some());
        assert!(parse_date("nonsense").is_none());
    }

    #[test]
    fn id_detection() {
        assert!(looks_like_id(&format!("{}==", "a".repeat(70))));
        assert!(!looks_like_id("hello world"));
        assert!(!looks_like_id("short"));
    }

    #[test]
    fn conversation_metadata_is_oldest_first() {
        let message = |id: &str, time| Message {
            meta: MessageMetadata {
                id: id.into(),
                time,
                ..Default::default()
            },
            ..Default::default()
        };

        let metadata = metadata_oldest_first(vec![message("b", 20), message("a", 10)]);

        let ids: Vec<_> = metadata.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["a", "b"]);
    }

    #[tokio::test]
    async fn conversation_bodies_fetch_at_most_three_and_keep_order() {
        let http = GatedDoer::new(4, None);
        let read = async {
            let fetched = fetch_conversation_bodies(&http, messages(5, true));
            futures::pin_mut!(fetched);
            let mut ids = Vec::new();
            while let Some(message) = fetched.try_next().await.unwrap() {
                ids.push(message.meta.id);
            }
            ids
        };
        let release = async {
            GatedDoer::wait_for(&http.started, 3).await;
            assert_eq!(http.active.load(Ordering::SeqCst), 3);

            // Complete newer responses first. The oldest is still awaited,
            // so no fourth request should begin yet.
            http.gates[2].add_permits(1);
            GatedDoer::wait_for(&http.completed, 1).await;
            assert_eq!(http.started.load(Ordering::SeqCst), 3);
            http.gates[1].add_permits(1);
            GatedDoer::wait_for(&http.completed, 2).await;
            assert_eq!(http.started.load(Ordering::SeqCst), 3);

            http.gates[0].add_permits(1);
            GatedDoer::wait_for(&http.started, 4).await;
            http.gates[3].add_permits(1);
        };

        let (ids, ()) = tokio::join!(read, release);
        assert_eq!(ids, ["m0", "m1", "m2", "m3", "m4"]);
        assert_eq!(http.started.load(Ordering::SeqCst), 4);
        assert_eq!(http.peak.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn conversation_body_fetch_error_is_returned() {
        let http = GatedDoer::new(2, Some(1));
        for gate in &http.gates {
            gate.add_permits(1);
        }

        let result: Result<Vec<_>> = fetch_conversation_bodies(&http, messages(2, false))
            .try_collect()
            .await;
        assert!(matches!(result, Err(Error::Other(message)) if message == "body fetch failed"));
    }
}
