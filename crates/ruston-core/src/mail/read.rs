//! Reading: list / search / read messages and conversations (with decryption).

use super::Client;
use crate::api::{self, messages::ListQuery};
use crate::crypto::{self, Verdict};
use crate::error::{Error, Result};
use crate::model::Conversation;
use crate::model::enums::resolve_folder;
use crate::model::message::{Attachment, Message, MessageMetadata, Recipient};
use crate::transport::Doer;
use futures::{Stream, StreamExt, TryStreamExt, stream};

/// Keep conversation reads responsive without flooding Proton or buffering many bodies.
const CONVERSATION_BODY_FETCH_CONCURRENCY: usize = 3;
/// How many sender key lookups run at once when a conversation opens.
const SENDER_KEY_CONCURRENCY: usize = 3;

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
    /// The Reply-To addresses the message asks replies to go to, if any.
    pub reply_tos: Vec<Recipient>,
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
        Ok(self.search_ref(r).await?.id)
    }

    /// Resolve a reference as [`resolve_ref`](Self::resolve_ref) does, to the
    /// message's metadata. A search hit already carries it, so only an exact
    /// ID is fetched.
    pub async fn resolve_ref_metadata(&self, r: &str) -> Result<MessageMetadata> {
        if looks_like_id(r) {
            return self.message_metadata(r).await;
        }
        self.search_ref(r).await
    }

    /// The one message a search of all mail for `r` finds.
    async fn search_ref(&self, r: &str) -> Result<MessageMetadata> {
        let mut hits = self
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
            1 => Ok(hits.remove(0)),
            n => Err(Error::Ambiguous(n)),
        }
    }

    pub(super) async fn decrypt_message(&self, m: &Message) -> Result<FullMessage> {
        self.open_message(m.clone()).await
    }

    /// Decrypts `m`, checking its signature against its sender's keys. A
    /// failed lookup leaves the body unverified.
    async fn open_message(&self, m: Message) -> Result<FullMessage> {
        let sender_pubs = match self.sender_pubkeys(&m.meta.sender.address).await {
            Ok(keys) => keys,
            // The body still opens, without claiming a verified signature.
            Err(e) => {
                tracing::warn!(target: "ruston_core::mail", error = %e, "sender key lookup failed — body left unverified");
                Vec::new()
            }
        };
        self.open_message_with(m, sender_pubs).await
    }

    /// Decrypts `m`, checking its signature against `sender_pubs`; with none,
    /// the body opens as unverified. Decrypting, parsing and sanitizing run
    /// off the async workers.
    pub(super) async fn open_message_with(
        &self,
        m: Message,
        sender_pubs: Vec<String>,
    ) -> Result<FullMessage> {
        let addr = self
            .keys()
            .address(&m.meta.address_id)
            .or_else(|| self.keys().primary_address())
            .ok_or_else(|| Error::Crypto("no address key for message".into()))?
            .clone();
        super::off_thread(move || open_body(&addr, m, &sender_pubs)).await
    }

    /// Asks for the keys of every distinct sender at once, a few at a time,
    /// so no body waits for another sender's lookup. A lookup that fails is
    /// tried again, and reported, by the body that needs it.
    async fn prefetch_sender_keys(&self, messages: &[Message]) {
        let mut senders: Vec<&str> = Vec::new();
        for message in messages {
            let sender = message.meta.sender.address.as_str();
            if !sender.is_empty()
                && !senders
                    .iter()
                    .any(|known| known.eq_ignore_ascii_case(sender))
            {
                senders.push(sender);
            }
        }
        stream::iter(senders)
            .for_each_concurrent(SENDER_KEY_CONCURRENCY, |sender| async move {
                let _ = self.sender_pubkeys(sender).await;
            })
            .await;
    }

    /// Fetch one message's metadata (subject, sender, labels) without
    /// decrypting its body, e.g. to show what an action is about to change.
    pub async fn message_metadata(&self, id: &str) -> Result<MessageMetadata> {
        Ok(api::messages::get_message(self.http(), id).await?.meta)
    }

    /// Fetch and decrypt a single message for display.
    pub async fn read_message(&self, id: &str) -> Result<FullMessage> {
        tracing::debug!(target: "ruston_core::mail", message_id = %id, "read_message: fetching + decrypting");
        let msg = api::messages::get_message(self.http(), id).await?;
        self.open_message(msg).await
    }

    /// Fetch and decrypt a message for a caller that keeps only its text: the
    /// sender's keys are not looked up, and the verdict is always unverified.
    pub(super) async fn read_message_unverified(&self, id: &str) -> Result<FullMessage> {
        let msg = api::messages::get_message(self.http(), id).await?;
        self.open_message_with(msg, Vec::new()).await
    }

    /// Fetch a conversation and decrypt all of its messages (oldest first).
    pub async fn read_conversation(&self, id: &str) -> Result<(Conversation, Vec<FullMessage>)> {
        let (conv, mut msgs) = api::conversations::get_conversation(self.http(), id).await?;
        msgs.sort_by_key(|m| m.meta.time);
        // The senders are known now: fetch their keys together up front.
        self.prefetch_sender_keys(&msgs).await;
        // Bodies download and decrypt at the same time, three of each at
        // most, and still come out in conversation order.
        let out = fetch_conversation_bodies(self.http(), msgs)
            .map_ok(|message| self.open_message(message))
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
    let (body, verdict) = crypto::decrypt_body(&provider, addr, sender_pubs, &m.body)?;
    let (body, mime_type) = if m.mime_type.starts_with("multipart/") {
        extract_mime(&body)
    } else {
        let mime_type = if m.mime_type.is_empty() {
            "text/plain".into()
        } else {
            m.mime_type
        };
        (body, mime_type)
    };
    // Sanitize HTML bodies (strip scripts / active content) for safe rendering.
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
        attachments: m.attachments,
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

    /// A client for me@proton.me against `server`, and the armored public key
    /// its messages are signed with.
    fn conversation_client(
        server: &wiremock::MockServer,
    ) -> (Client, crypto::keys::AddressKeys, String) {
        use crate::cache::CacheIdentity;
        use crate::crypto::{KeyStore, StoredKey};
        use crate::session::{MemoryStore, Paths};
        use crate::transport::HttpClient;
        use proton_crypto::crypto::{
            DataEncoding, KeyGenerator, KeyGeneratorAlgorithm, KeyGeneratorSync, PGPProviderSync,
        };
        use secrecy::SecretString;

        let provider = crypto::provider();
        let key = provider
            .new_key_generator()
            .with_user_id("Me", "me@proton.me")
            .with_algorithm(KeyGeneratorAlgorithm::ECC)
            .generate()
            .unwrap();
        let armored = String::from_utf8(
            provider
                .private_key_export(&key, "pass", DataEncoding::Armor)
                .unwrap()
                .as_ref()
                .to_vec(),
        )
        .unwrap();
        let public = provider.private_key_to_public_key(&key).unwrap();
        let public = String::from_utf8(
            provider
                .public_key_export(&public, DataEncoding::Armor)
                .unwrap()
                .as_ref()
                .to_vec(),
        )
        .unwrap();
        let address = crypto::keys::AddressKeys {
            address_id: "address".into(),
            email: "me@proton.me".into(),
            keys: vec![StoredKey {
                armored,
                passphrase: SecretString::from("pass"),
                unlocked: None,
            }],
        };
        let client = Client {
            http: HttpClient::new(server.uri(), "Other"),
            keys: KeyStore {
                user_keys: Vec::new(),
                addresses: vec![address.clone()],
            },
            paths: Paths::with_base(std::env::temp_dir()),
            profile: "test".into(),
            session_uid: "test".into(),
            cache_identity: CacheIdentity::new(&server.uri(), "test").unwrap(),
            store: std::sync::Arc::new(MemoryStore::default()),
            sender_cache: tokio::sync::Mutex::new(super::super::SenderKeyCache::default()),
        };
        (client, address, public)
    }

    /// Serves conversation `c1` with one message per `(sender, time)`, each
    /// body signed by `address`, and answers every sender's key lookup with
    /// `public` after `delay`, expecting exactly one lookup per sender.
    async fn serve_conversation(
        server: &wiremock::MockServer,
        address: &crypto::keys::AddressKeys,
        public: &str,
        messages: &[(&str, i64)],
        delay: Duration,
    ) {
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, ResponseTemplate};

        let provider = crypto::provider();
        let meta = |index: usize, (sender, time): (&str, i64)| {
            serde_json::json!({
                "ID": format!("m{index}"), "Time": time, "AddressID": "address",
                "Sender": {"Address": sender}, "LabelIDs": ["0"]
            })
        };
        // Listed out of order: the conversation is read oldest first.
        let listed: Vec<_> = messages
            .iter()
            .enumerate()
            .rev()
            .map(|(index, message)| meta(index, *message))
            .collect();
        Mock::given(method("GET"))
            .and(path("/mail/v4/conversations/c1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Code": 1000, "Conversation": {"ID": "c1", "Subject": "Plans"}, "Messages": listed
            })))
            .mount(server)
            .await;
        for (index, message) in messages.iter().enumerate() {
            let mut full = meta(index, *message);
            full["Body"] = crypto::encrypt_self_draft(&provider, address, &format!("body {index}"))
                .unwrap()
                .into();
            full["MIMEType"] = "text/plain".into();
            Mock::given(method("GET"))
                .and(path(format!("/mail/v4/messages/m{index}")))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(serde_json::json!({"Code": 1000, "Message": full})),
                )
                .mount(server)
                .await;
        }
        let mut senders: Vec<&str> = messages.iter().map(|(sender, _)| *sender).collect();
        senders.sort_unstable();
        senders.dedup();
        for sender in senders {
            Mock::given(method("GET"))
                .and(path("/core/v4/keys/all"))
                .and(query_param("Email", sender))
                .respond_with(ResponseTemplate::new(200).set_delay(delay).set_body_json(
                    serde_json::json!({
                        "Code": 1000, "Address": {"Keys": [{"PublicKey": public, "Flags": 3}]}
                    }),
                ))
                .expect(1)
                .mount(server)
                .await;
        }
    }

    #[tokio::test]
    async fn a_conversation_opens_in_order_with_one_key_lookup_per_sender() {
        let server = wiremock::MockServer::start().await;
        let (client, address, public) = conversation_client(&server);
        let messages = [
            ("alice@example.org", 10),
            ("bob@example.org", 20),
            ("alice@example.org", 30),
        ];
        serve_conversation(&server, &address, &public, &messages, Duration::ZERO).await;

        let (conversation, read) = client.read_conversation("c1").await.unwrap();

        assert_eq!(conversation.subject, "Plans");
        let bodies: Vec<&str> = read.iter().map(|message| message.body.as_str()).collect();
        assert_eq!(bodies, ["body 0", "body 1", "body 2"]);
        assert!(
            read.iter()
                .all(|message| message.verdict == Verdict::Verified)
        );
        // Each sender's keys were asked for once, as the mocks expect.
    }

    #[tokio::test]
    async fn the_senders_keys_are_asked_for_together() {
        let server = wiremock::MockServer::start().await;
        let (client, address, public) = conversation_client(&server);
        let messages = [
            ("alice@example.org", 10),
            ("bob@example.org", 20),
            ("carol@example.org", 30),
        ];
        // Each lookup takes a second: one after the other, three seconds.
        serve_conversation(
            &server,
            &address,
            &public,
            &messages,
            Duration::from_secs(1),
        )
        .await;

        let started = std::time::Instant::now();
        let (_, read) = client.read_conversation("c1").await.unwrap();

        assert_eq!(read.len(), 3);
        assert!(
            started.elapsed() < Duration::from_millis(2500),
            "took {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn a_search_reference_brings_its_metadata_without_a_second_request() {
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, ResponseTemplate};

        let server = wiremock::MockServer::start().await;
        let (client, _, _) = conversation_client(&server);
        Mock::given(method("GET"))
            .and(path("/mail/v4/messages"))
            .and(query_param("Keyword", "quarterly"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Code": 1000, "Total": 1,
                "Messages": [{
                    "ID": "hit", "Subject": "Quarterly numbers",
                    "Sender": {"Address": "boss@example.org"}
                }]
            })))
            .mount(&server)
            .await;
        // The search answer already says what the hit is.
        Mock::given(method("GET"))
            .and(path("/mail/v4/messages/hit"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&server)
            .await;
        let id = format!("{}==", "i".repeat(60));
        Mock::given(method("GET"))
            .and(path(format!("/mail/v4/messages/{id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Code": 1000, "Message": {"ID": id, "Subject": "Named by its ID"}
            })))
            .expect(1)
            .mount(&server)
            .await;

        let hit = client.resolve_ref_metadata("quarterly").await.unwrap();
        assert_eq!(
            (hit.id.as_str(), hit.subject.as_str()),
            ("hit", "Quarterly numbers")
        );
        assert_eq!(hit.sender.address, "boss@example.org");
        let named = client.resolve_ref_metadata(&id).await.unwrap();
        assert_eq!((named.id, named.subject), (id, "Named by its ID".into()));
        assert_eq!(client.resolve_ref("quarterly").await.unwrap(), "hit");
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
