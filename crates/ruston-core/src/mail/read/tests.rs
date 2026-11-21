use super::*;
use crate::mail::test_support;
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
    use crate::crypto::StoredKey;
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
    let client = test_support::client(server, vec![address.clone()]);
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
async fn an_undecryptable_message_leaves_the_rest_of_the_thread_readable() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, ResponseTemplate};

    let server = wiremock::MockServer::start().await;
    let (client, address, public) = conversation_client(&server);
    let messages = [
        ("alice@example.org", 10),
        ("bob@example.org", 20),
        ("alice@example.org", 30),
    ];
    serve_conversation(&server, &address, &public, &messages, Duration::ZERO).await;
    // The middle message is encrypted to a key this account does not hold.
    let (_, stranger, _) = conversation_client(&server);
    let body = crypto::encrypt_self_draft(&crypto::provider(), &stranger, "secret").unwrap();
    Mock::given(method("GET"))
        .and(path("/mail/v4/messages/m1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "Code": 1000,
            "Message": {
                "ID": "m1", "Time": 20, "AddressID": "address",
                "Sender": {"Address": "bob@example.org"}, "LabelIDs": ["0"],
                "ReplyTos": [{"Address": "replies@example.org"}],
                "Body": body, "MIMEType": "text/plain"
            }
        })))
        .with_priority(1)
        .mount(&server)
        .await;

    let (_, read) = client.read_conversation_each("c1").await.unwrap();

    let [
        ConversationMessage::Opened(first),
        ConversationMessage::Unreadable(unreadable),
        ConversationMessage::Opened(third),
    ] = &read[..]
    else {
        panic!("expected only the middle message to be unreadable");
    };
    assert_eq!(first.body, "body 0");
    assert_eq!(unreadable.meta.id, "m1");
    assert_eq!(unreadable.meta.sender.address, "bob@example.org");
    assert_eq!(unreadable.reply_tos[0].address, "replies@example.org");
    assert!(
        matches!(unreadable.error, Error::Crypto(_)),
        "{:?}",
        unreadable.error
    );
    assert_eq!(third.body, "body 2");
    // Reading the thread as a whole still reports the message it could not open.
    assert!(matches!(
        client.read_conversation("c1").await,
        Err(Error::Crypto(_))
    ));
}

#[tokio::test]
async fn a_message_on_its_own_that_cannot_be_opened_comes_back_unreadable() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, ResponseTemplate};

    let server = wiremock::MockServer::start().await;
    let (client, address, _) = conversation_client(&server);
    let (_, stranger, _) = conversation_client(&server);
    let provider = crypto::provider();
    for (id, key) in [("own", &address), ("stranger", &stranger)] {
        let body = crypto::encrypt_self_draft(&provider, key, "secret").unwrap();
        Mock::given(method("GET"))
            .and(path(format!("/mail/v4/messages/{id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Code": 1000,
                "Message": {
                    "ID": id, "Time": 20, "AddressID": "address",
                    "Sender": {"Address": "bob@example.org"}, "LabelIDs": ["0"],
                    "Body": body, "MIMEType": "text/plain"
                }
            })))
            .mount(&server)
            .await;
    }

    let ConversationMessage::Opened(opened) =
        client.read_message_or_unreadable("own").await.unwrap()
    else {
        panic!("the account's own key opens it");
    };
    assert_eq!(opened.body, "secret");

    let ConversationMessage::Unreadable(unreadable) =
        client.read_message_or_unreadable("stranger").await.unwrap()
    else {
        panic!("no key of the account opens it");
    };
    assert_eq!(unreadable.meta.id, "stranger");
    assert_eq!(unreadable.meta.sender.address, "bob@example.org");
    assert!(
        matches!(unreadable.error, Error::Crypto(_)),
        "{:?}",
        unreadable.error
    );
    // Read on its own, it is still an error.
    assert!(matches!(
        client.read_message("stranger").await,
        Err(Error::Crypto(_))
    ));
}

#[tokio::test]
async fn a_failed_body_download_still_fails_the_whole_thread() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, ResponseTemplate};

    for status in [503, 401] {
        let server = wiremock::MockServer::start().await;
        let (client, address, public) = conversation_client(&server);
        let messages = [("alice@example.org", 10), ("bob@example.org", 20)];
        serve_conversation(&server, &address, &public, &messages, Duration::ZERO).await;
        Mock::given(method("GET"))
            .and(path("/mail/v4/messages/m1"))
            .respond_with(
                ResponseTemplate::new(status)
                    .insert_header("Retry-After", "0")
                    .set_body_json(serde_json::json!({"Code": status})),
            )
            .with_priority(1)
            .mount(&server)
            .await;

        // A request that failed says nothing about the message, so it is
        // never shown as one that could not be decrypted.
        let Err(error) = client.read_conversation_each("c1").await else {
            panic!("a failed download must fail the read");
        };
        if status == 401 {
            assert!(matches!(error, Error::Unauthorized), "{error:?}");
        } else {
            assert!(
                matches!(&error, Error::Api(api) if api.http_status == 503),
                "{error:?}"
            );
        }
    }
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

#[tokio::test]
async fn a_search_covers_the_whole_day_it_names() {
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, ResponseTemplate};

    let server = wiremock::MockServer::start().await;
    let (client, _, _) = conversation_client(&server);
    // 2024-01-15, from its first second to its last, in UTC.
    Mock::given(method("GET"))
        .and(path("/mail/v4/messages"))
        .and(query_param("Begin", "1705276800"))
        .and(query_param("End", "1705363199"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "Code": 1000, "Total": 1, "Messages": [{"ID": "evening", "Time": 1_705_359_600}]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let hits = client
        .search_messages(&SearchOpts {
            after: Some("2024-01-15".into()),
            before: Some("2024-01-15".into()),
            ..Default::default()
        })
        .await
        .unwrap();

    assert_eq!(hits.len(), 1);
}

#[tokio::test]
async fn an_invalid_search_date_fails_before_any_request() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, ResponseTemplate};

    let server = wiremock::MockServer::start().await;
    let (client, _, _) = conversation_client(&server);
    Mock::given(method("GET"))
        .and(path("/mail/v4/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"Code": 1000, "Total": 0, "Messages": []})),
        )
        .expect(0)
        .mount(&server)
        .await;

    for date in ["15/01/2024", "2024-02-30", "yesterday", ""] {
        for opts in [
            SearchOpts {
                after: Some(date.into()),
                ..Default::default()
            },
            SearchOpts {
                before: Some(date.into()),
                ..Default::default()
            },
        ] {
            assert!(
                matches!(client.search_messages(&opts).await, Err(Error::Other(_))),
                "{opts:?} was searched"
            );
        }
    }
    // A range that ends before it starts finds nothing, which is no answer.
    let backwards = SearchOpts {
        after: Some("2024-02-01".into()),
        before: Some("2024-01-31".into()),
        ..Default::default()
    };
    assert!(matches!(
        client.search_conversations(&backwards).await,
        Err(Error::Other(ref reason)) if reason == "invalid date range: 2024-02-01 is after 2024-01-31"
    ));
    let one_day = SearchOpts {
        before: Some("2024-02-01".into()),
        ..backwards
    };
    assert!(client.search_query(&one_day).is_ok());
}

#[test]
fn search_dates_are_whole_days_in_utc() {
    let day = parse_date("2024-01-15").unwrap();
    assert_eq!(
        (day_start(day), day_end(day)),
        (1_705_276_800, 1_705_363_199)
    );
    assert!(parse_date("2024-02-29").is_ok());
    // Spaces around it and single digits are taken, as they always were.
    for same in [" 2024-01-15 ", "2024-1-15"] {
        assert_eq!(parse_date(same).unwrap(), day, "{same}");
    }
    assert!(parse_date("0001-01-01").is_ok() && parse_date("9999-12-31").is_ok());
    for invalid in [
        "2023-02-29",
        "nonsense",
        "2024-01-15T10:00",
        "2024-01-15x",
        "2024-13-01",
        "2024-00-10",
        "20240115",
        "2024/01/15",
        "10000-01-01",
        "",
    ] {
        assert!(parse_date(invalid).is_err(), "{invalid}");
        assert!(validate_search_date(invalid).is_err(), "{invalid}");
    }
    assert!(validate_search_date("2024-01-15").is_ok());
}

#[test]
fn every_part_of_a_mixed_body_shows_and_an_alternative_shows_once() {
    let mixed = "Content-Type: multipart/mixed; boundary=b\r\n\r\n\
            --b\r\nContent-Type: text/plain\r\n\r\nFirst part\r\n\
            --b\r\nContent-Type: image/png\r\nContent-Disposition: inline\r\n\
            Content-Transfer-Encoding: base64\r\n\r\niVBORw0KGgo=\r\n\
            --b\r\nContent-Type: text/plain\r\n\r\nSecond part\r\n--b--\r\n";
    let (body, _, _) = extract_mime(mixed.as_bytes());
    assert!(body.contains("First part"), "{body}");
    assert!(body.contains("Second part"), "{body}");

    let alternative = "Content-Type: multipart/alternative; boundary=a\r\n\r\n\
            --a\r\nContent-Type: text/plain\r\n\r\nPlain version\r\n\
            --a\r\nContent-Type: text/html\r\n\r\n<p>Rich version</p>\r\n--a--\r\n";
    let (body, mime_type, _) = extract_mime(alternative.as_bytes());
    assert_eq!(mime_type, "text/html");
    assert!(body.contains("Rich version"), "{body}");
    assert!(!body.contains("Plain version"), "{body}");
}

#[test]
fn a_pgp_mime_body_names_its_attachments_by_position() {
    let mixed = "Content-Type: multipart/mixed; boundary=b\r\n\r\n\
    --b\r\nContent-Type: text/plain\r\n\r\nSee the report.\r\n\
    --b\r\nContent-Type: application/pdf; name=report.pdf\r\n\
    Content-Disposition: attachment; filename=report.pdf\r\n\
    Content-Transfer-Encoding: base64\r\n\r\nJVBERi0xLjQK\r\n\
    --b\r\nContent-Type: image/png\r\nContent-Disposition: inline\r\n\
    Content-ID: <logo>\r\nContent-Transfer-Encoding: base64\r\n\r\n\
    iVBORw0KGgo=\r\n--b--\r\n";
    let (body, _, attachments) = extract_mime(mixed.as_bytes());
    assert!(body.contains("See the report."), "{body}");
    let listed: Vec<_> = attachments
        .iter()
        .map(|a| {
            (
                a.id.as_str(),
                a.name.as_str(),
                a.mime_type.as_deref(),
                a.is_inline(),
                a.size,
            )
        })
        .collect();
    assert_eq!(
        listed,
        [
            ("mime-1", "report.pdf", Some("application/pdf"), false, 9),
            ("mime-2", "attachment-2", Some("image/png"), true, 8),
        ]
    );
    // The body's signature covers them; they carry none of their own.
    assert!(
        attachments
            .iter()
            .all(|a| a.key_packets.is_none() && a.signature.is_none())
    );

    let part = |position| mime_attachment(mixed.as_bytes(), position);
    assert_eq!(part(1), Some(("report.pdf".into(), b"%PDF-1.4\n".to_vec())));
    assert_eq!(
        part(2).map(|(name, bytes)| (name, bytes.len())),
        Some(("attachment-2".into(), 8))
    );
    assert_eq!(part(0), None);
    assert_eq!(part(3), None);
}

#[tokio::test]
async fn a_pgp_mime_part_reads_in_its_own_charset() {
    use proton_crypto::crypto::{
        DataEncoding, Encryptor, EncryptorSync, PGPMessage, PGPProviderSync,
    };

    let server = wiremock::MockServer::start().await;
    let (_, address, public) = conversation_client(&server);
    let provider = crypto::provider();
    let public = provider
        .public_key_import(public.as_bytes(), DataEncoding::Armor)
        .unwrap();
    // An 8-bit Latin-1 part, as a PGP/MIME sender may write it: its
    // bytes are not UTF-8 until its charset decodes them.
    let mime = b"Content-Type: multipart/mixed; boundary=b\r\n\r\n\
            --b\r\nContent-Type: text/plain; charset=iso-8859-1\r\n\
            Content-Transfer-Encoding: 8bit\r\n\r\nCaf\xe9 au lait\r\n--b--\r\n";
    let armored = provider
        .new_encryptor()
        .with_encryption_key(&public)
        .encrypt(mime)
        .unwrap()
        .armor()
        .unwrap();
    let message = Message {
        body: String::from_utf8(armored).unwrap(),
        mime_type: "multipart/mixed".into(),
        ..Message::default()
    };

    let full = open_body(&address, message, &[]).unwrap();

    assert_eq!(full.body.trim_end(), "Café au lait");
}

#[test]
fn only_a_long_unspaced_reference_ending_in_padding_reads_as_an_id() {
    // A message ID is long base64 ending in "==".
    let id = format!("{}==", "Bp4".repeat(29));
    assert!(looks_like_id(&id));
    let unpadded = "Bp4".repeat(30);
    let spaced = format!("{} ==", "Bp4".repeat(29));
    for reference in [
        "quarterly report",
        "abc==",
        unpadded.as_str(),
        spaced.as_str(),
    ] {
        assert!(!looks_like_id(reference), "{reference}");
    }
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
        test_support::wait_for(&http.started, 3, "body fetches").await;
        assert_eq!(http.active.load(Ordering::SeqCst), 3);

        // Complete newer responses first. The oldest is still awaited,
        // so no fourth request should begin yet.
        http.gates[2].add_permits(1);
        test_support::wait_for(&http.completed, 1, "body fetches").await;
        assert_eq!(http.started.load(Ordering::SeqCst), 3);
        http.gates[1].add_permits(1);
        test_support::wait_for(&http.completed, 2, "body fetches").await;
        assert_eq!(http.started.load(Ordering::SeqCst), 3);

        http.gates[0].add_permits(1);
        test_support::wait_for(&http.started, 4, "body fetches").await;
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
