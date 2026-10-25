use super::*;
use crate::mail::test_support;
use crate::transport::{HttpClient, Request, Response};
use async_trait::async_trait;
use serde::de::DeserializeOwned;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn recipient_keys(keys: &[u32], unverified: &[u32]) -> api::keys::RecipientKeys {
    let with_flags = |flags: &[u32]| {
        flags
            .iter()
            .map(|flags| api::keys::ApiPublicKey {
                public_key: String::new(),
                flags: *flags,
                primary: 1,
                source: 0,
            })
            .collect()
    };
    api::keys::RecipientKeys {
        recipient_type: 1,
        keys: with_flags(keys),
        unverified: with_flags(unverified),
    }
}

#[test]
fn recipient_protection_follows_the_keys_sending_would_use() {
    use crate::model::enums::key_flag::{NOT_COMPROMISED, NOT_OBSOLETE};

    let usable = NOT_COMPROMISED | NOT_OBSOLETE;
    assert_eq!(
        RecipientProtection::of(&recipient_keys(&[usable], &[])),
        RecipientProtection::EndToEnd
    );
    // A verified Proton key wins over a published one.
    assert_eq!(
        RecipientProtection::of(&recipient_keys(&[usable], &[usable])),
        RecipientProtection::EndToEnd
    );
    assert_eq!(
        RecipientProtection::of(&recipient_keys(&[], &[usable])),
        RecipientProtection::Pgp
    );
    assert_eq!(
        RecipientProtection::of(&recipient_keys(&[], &[])),
        RecipientProtection::Unencrypted
    );
    // Obsolete keys may verify old mail but never encrypt new mail.
    assert_eq!(
        RecipientProtection::of(&recipient_keys(&[NOT_COMPROMISED], &[NOT_COMPROMISED])),
        RecipientProtection::Unencrypted
    );
    // Nor are keys on an address whose owner turned email encryption off.
    let off = usable | crate::model::enums::key_flag::EMAIL_NO_ENCRYPT;
    assert_eq!(
        RecipientProtection::of(&recipient_keys(&[off], &[])),
        RecipientProtection::Unencrypted
    );
}

#[tokio::test]
async fn an_address_that_turned_email_encryption_off_gets_a_cleartext_package() {
    use wiremock::matchers::query_param;

    let server = MockServer::start().await;
    let (me, _) = test_address("me@proton.me", "address-1");
    let (_, forwarding_public) = test_address("fwd@proton.me", "forwarding-address");
    Mock::given(method("POST"))
        .and(path("/mail/v4/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"Code": 1000, "Message": {"ID": "draft-1"}})),
        )
        .mount(&server)
        .await;
    // A Proton address that forwards elsewhere: its key is not for mail.
    Mock::given(method("GET"))
        .and(path("/core/v4/keys/all"))
        .and(query_param("Email", "fwd@proton.me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Code": 1000, "RecipientType": 1,
            "Address": {"Keys": [{"PublicKey": forwarding_public, "Flags": 7, "Primary": 1}]}
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/mail/v4/messages/draft-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"Code": 1000})))
        .expect(1)
        .mount(&server)
        .await;

    test_support::client(&server, vec![me])
        .send(&SendOptions {
            to: vec!["fwd@proton.me".into()],
            body: "Hello".into(),
            ..SendOptions::default()
        })
        .await
        .unwrap();

    let requests = server.received_requests().await.unwrap();
    let send = requests
        .iter()
        .find(|request| request.url.path() == "/mail/v4/messages/draft-1")
        .unwrap();
    let body: Value = serde_json::from_slice(&send.body).unwrap();
    let types: Vec<_> = body["Packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|package| package["Type"].as_i64().unwrap())
        .collect();
    assert_eq!(types, [package_type::CLEAR]);
    assert!(body["Packages"][0]["Addresses"]["fwd@proton.me"].is_object());
}

fn attachment_path() -> PathBuf {
    // Tests run in parallel and the clock can read the same for two of
    // them, so a counter keeps each file to itself.
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "ruston-outgoing-{}-{unique}-{}.txt",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ))
}

#[tokio::test]
async fn attachment_preflight_checks_individual_and_combined_limits() {
    let path = attachment_path();
    let file = File::create(&path).unwrap();
    file.set_len(MAX_OUTGOING_ATTACHMENT_BYTES as u64).unwrap();
    assert!(validate_attachments(&vec![path.clone(); 4]).await.is_ok());
    assert!(
        matches!(validate_attachments(&vec![path.clone(); 5]).await, Err(Error::AttachmentBatchTooLarge { limit }) if limit == MAX_OUTGOING_ATTACHMENT_TOTAL_BYTES)
    );
    file.set_len(MAX_OUTGOING_ATTACHMENT_BYTES as u64 + 1)
        .unwrap();
    assert!(
        matches!(validate_attachments(std::slice::from_ref(&path)).await, Err(Error::AttachmentTooLarge { limit }) if limit == MAX_OUTGOING_ATTACHMENT_BYTES)
    );
    drop(file);
    std::fs::remove_file(&path).unwrap();
    assert!(
        matches!(validate_attachments(std::slice::from_ref(&path)).await, Err(Error::AttachmentUnavailable { path: missing, .. }) if missing == path)
    );
    assert!(matches!(
        validate_attachments(&[std::env::temp_dir()]).await,
        Err(Error::AttachmentUnavailable { .. })
    ));
}

#[test]
fn attachment_reads_enforce_actual_lengths_after_metadata_changes() {
    let mut reader = std::io::Cursor::new(b"123456789");
    let error = read_attachment_bytes(&mut reader, 0, 4).unwrap_err();
    assert!(matches!(error, Error::AttachmentBatchTooLarge { .. }));
    assert_eq!(
        reader.position(),
        5,
        "do not read beyond the overflow sentinel"
    );
    let bytes = read_attachment_bytes(std::io::Cursor::new(b"1234"), 0, 4).unwrap();
    assert_eq!(&**bytes, b"1234");
    assert!(
        read_attachment_bytes(std::io::empty(), 0, 0)
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        read_attachment_bytes(
            std::io::empty(),
            MAX_OUTGOING_ATTACHMENT_BYTES as u64 + 1,
            MAX_OUTGOING_ATTACHMENT_TOTAL_BYTES
        ),
        Err(Error::AttachmentTooLarge { .. })
    ));
}

#[tokio::test]
async fn invalid_attachments_are_rejected_before_draft_creation() {
    let server = MockServer::start().await;
    let path = attachment_path();
    let file = File::create(&path).unwrap();
    file.set_len(MAX_OUTGOING_ATTACHMENT_BYTES as u64 + 1)
        .unwrap();
    let client = Client {
        http: HttpClient::new(server.uri(), "Other"),
        keys: crypto::keys::KeyStore { addresses: vec![] },
        paths: crate::session::Paths::with_base(path.with_extension("session")),
        profile: "test".into(),
        session_uid: "test".into(),
        cache_identity: crate::cache::CacheIdentity::new(&server.uri(), "outgoing-test").unwrap(),
        store: std::sync::Arc::new(crate::session::MemoryStore::new()),
        sender_cache: tokio::sync::Mutex::new(Default::default()),
    };
    let opts = SendOptions {
        attachments: vec![path.clone()],
        ..SendOptions::default()
    };
    assert!(matches!(
        client.send(&opts).await,
        Err(Error::AttachmentTooLarge { .. })
    ));
    assert!(server.received_requests().await.unwrap().is_empty());
    drop(file);
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn a_refused_recipient_stops_the_send_before_any_draft_or_upload() {
    use wiremock::matchers::query_param;
    let server = MockServer::start().await;
    let (me, _) = test_address("me@proton.me", "address-1");
    let attachment = attachment_path();
    std::fs::write(&attachment, b"quarterly numbers").unwrap();
    Mock::given(method("GET"))
        .and(path("/core/v4/keys/all"))
        .and(query_param("Email", "plain@example.com"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"Code": 1000, "RecipientType": 2})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/core/v4/keys/all"))
        .and(query_param("Email", "nobody@proton.me"))
        .respond_with(ResponseTemplate::new(422).set_body_json(json!({
            "Code": 33102, "Error": "address does not exist"
        })))
        .mount(&server)
        .await;

    let result = test_support::client(&server, vec![me])
        .send(&SendOptions {
            to: vec!["plain@example.com".into(), "nobody@proton.me".into()],
            body: "Numbers attached".into(),
            attachments: vec![attachment.clone()],
            ..SendOptions::default()
        })
        .await;

    assert!(
        matches!(&result, Err(Error::Api(api)) if api.code == 33102),
        "{result:?}"
    );
    let paths: Vec<_> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|request| request.url.path().to_owned())
        .collect();
    assert!(
        paths.iter().all(|path| path == "/core/v4/keys/all"),
        "no draft, upload or clean-up: {paths:?}"
    );
    std::fs::remove_file(attachment).unwrap();
}

#[tokio::test]
async fn recipients_keys_are_asked_for_together() {
    let server = MockServer::start().await;
    let (me, _) = test_address("me@proton.me", "address-1");
    // Each lookup takes a second: one after the other, three seconds.
    Mock::given(method("GET"))
        .and(path("/core/v4/keys/all"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"Code": 1000, "RecipientType": 2}))
                .set_delay(Duration::from_secs(1)),
        )
        .expect(3)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/mail/v4/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"Code": 1000, "Message": {"ID": "draft-1"}})),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/mail/v4/messages/draft-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"Code": 1000})))
        .expect(1)
        .mount(&server)
        .await;

    let started = std::time::Instant::now();
    test_support::client(&server, vec![me])
        .send(&SendOptions {
            to: vec!["ada@example.org".into(), "bob@example.org".into()],
            cc: vec!["cy@example.org".into()],
            body: "Hello".into(),
            ..SendOptions::default()
        })
        .await
        .unwrap();

    assert!(
        started.elapsed() < Duration::from_millis(2500),
        "took {:?}",
        started.elapsed()
    );
    server.verify().await;
}

#[tokio::test]
async fn blocking_attachment_jobs_keep_their_slot_after_cancellation() {
    // Isolate this saturation test from other tests preparing real files.
    static SLOTS: Semaphore = Semaphore::const_new(2);
    let current = std::thread::current().id();
    let (started, waiting) = tokio::sync::oneshot::channel();
    let (release, held) = std::sync::mpsc::channel();
    let operation = tokio::spawn(attachment_work(&SLOTS, move || {
        assert_ne!(std::thread::current().id(), current);
        started.send(()).unwrap();
        held.recv_timeout(Duration::from_secs(5))
            .expect("the test never released the job");
        Ok(())
    }));
    waiting.await.unwrap();
    operation.abort();
    assert!(operation.await.unwrap_err().is_cancelled());
    let spare = SLOTS.acquire().await.unwrap();
    let mut next = Box::pin(attachment_work(&SLOTS, || Ok(std::thread::current().id())));
    assert!(
        futures::poll!(&mut next).is_pending(),
        "the detached worker still owns a slot"
    );
    release.send(()).unwrap();
    let (worker, permit) = tokio::time::timeout(Duration::from_secs(5), next)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(worker, current);
    drop(permit);
    drop(spare);
}

#[tokio::test]
async fn prepared_attachment_packets_round_trip_and_keep_the_upload_slot() {
    use proton_crypto::crypto::{
        DataEncoding, KeyGenerator, KeyGeneratorAlgorithm, KeyGeneratorSync, PGPProviderSync,
    };
    let addr = tokio::task::spawn_blocking(|| {
        let provider = crypto::provider();
        let key = provider
            .new_key_generator()
            .with_user_id("Test", "sender@example.test")
            .with_algorithm(KeyGeneratorAlgorithm::ECC)
            .generate()
            .unwrap();
        let armored = provider
            .private_key_export(&key, "test-passphrase", DataEncoding::Armor)
            .unwrap();
        AddressKeys {
            address_id: "test-address".into(),
            email: "sender@example.test".into(),
            keys: vec![crypto::keys::StoredKey {
                armored: String::from_utf8(armored.as_ref().to_vec()).unwrap(),
                passphrase: secrecy::SecretString::from("test-passphrase"),
                unlocked: None,
            }],
        }
    })
    .await
    .unwrap();
    let path = attachment_path();
    let plain = b"attachment payload\x00\xfe";
    std::fs::write(&path, plain).unwrap();
    let prepared = prepare_attachment(
        path.clone(),
        addr.clone(),
        "draft-1".into(),
        MAX_OUTGOING_ATTACHMENT_TOTAL_BYTES,
    )
    .await
    .unwrap();
    assert_eq!(prepared.size, plain.len());
    let crate::transport::Body::Bytes(body) = &prepared.request.body else {
        panic!("expected buffered multipart")
    };
    let boundary = prepared
        .request
        .content_type
        .as_ref()
        .unwrap()
        .split("boundary=")
        .nth(1)
        .unwrap();
    let part = |name: &str| {
        let marker = format!("name=\"{name}\"");
        let start = body
            .windows(marker.len())
            .position(|bytes| bytes == marker.as_bytes())
            .unwrap()
            + marker.len();
        let start = start
            + body[start..]
                .windows(4)
                .position(|bytes| bytes == b"\r\n\r\n")
                .unwrap()
            + 4;
        let delimiter = format!("\r\n--{boundary}");
        let end = start
            + body[start..]
                .windows(delimiter.len())
                .position(|bytes| bytes == delimiter.as_bytes())
                .unwrap();
        &body[start..end]
    };
    let decoded = crypto::decrypt_attachment(
        &crypto::provider(),
        &addr,
        &STANDARD.encode(part("KeyPackets")),
        part("DataPacket"),
    )
    .unwrap();
    assert_eq!(decoded, plain);
    assert!(!part("Signature").is_empty());
    assert_eq!(part("MessageID"), b"draft-1");
    // A ready request retains its admission slot while waiting for upload.
    let spare = ATTACHMENT_SLOTS.acquire().await.unwrap();
    let mut next = Box::pin(ATTACHMENT_SLOTS.acquire());
    assert!(futures::poll!(&mut next).is_pending());
    drop(prepared);
    drop(
        tokio::time::timeout(Duration::from_secs(5), next)
            .await
            .unwrap()
            .unwrap(),
    );
    drop(spare);
    std::fs::remove_file(path).unwrap();
}

/// A fresh address key, returned with its armored public key.
fn test_address(email: &str, address_id: &str) -> (AddressKeys, String) {
    use proton_crypto::crypto::{
        DataEncoding, KeyGenerator, KeyGeneratorAlgorithm, KeyGeneratorSync, PGPProviderSync,
    };
    let provider = crypto::provider();
    let key = provider
        .new_key_generator()
        .with_user_id("Test", email)
        .with_algorithm(KeyGeneratorAlgorithm::ECC)
        .generate()
        .unwrap();
    let armored = provider
        .private_key_export(&key, "test-passphrase", DataEncoding::Armor)
        .unwrap();
    let public = provider.private_key_to_public_key(&key).unwrap();
    let public = provider
        .public_key_export(&public, DataEncoding::Armor)
        .unwrap();
    (
        AddressKeys {
            address_id: address_id.into(),
            email: email.into(),
            keys: vec![crypto::keys::StoredKey {
                armored: String::from_utf8(armored.as_ref().to_vec()).unwrap(),
                passphrase: secrecy::SecretString::from("test-passphrase"),
                unlocked: None,
            }],
        },
        String::from_utf8(public.as_ref().to_vec()).unwrap(),
    )
}

#[tokio::test]
async fn forwarded_attachments_are_keyed_for_every_recipient() {
    use wiremock::matchers::query_param;

    let server = MockServer::start().await;
    let (me, _) = test_address("me@proton.me", "address-1");
    let (friend, friend_public) = test_address("friend@proton.me", "friend-address");
    let provider = crypto::provider();
    let original = crypto::encrypt_attachment(&provider, &me, b"forwarded payload").unwrap();
    let key_packets = STANDARD.encode(&original.key_packet);
    let parent_id = format!("{}==", "p".repeat(60));

    Mock::given(method("GET"))
        .and(path(format!("/mail/v4/messages/{parent_id}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Code": 1000,
            "Message": {
                "ID": parent_id, "AddressID": "address-1", "Subject": "Report",
                "Sender": {"Name": "Boss", "Address": "boss@proton.me"},
                "Attachments": [{"ID": "parent-att", "Name": "report.pdf", "KeyPackets": key_packets}]
            }
        })))
        .mount(&server)
        .await;
    // Proton copies the original's attachment into the draft under a new ID,
    // keyed with the packet the draft request supplied for it.
    Mock::given(method("POST"))
        .and(path("/mail/v4/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Code": 1000,
            "Message": {
                "ID": "draft-1",
                "Attachments": [{"ID": "draft-att", "Name": "report.pdf", "KeyPackets": key_packets}]
            }
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/core/v4/keys/all"))
        .and(query_param("Email", "friend@proton.me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Code": 1000, "RecipientType": 1,
            "Address": {"Keys": [{"PublicKey": friend_public, "Flags": 3, "Primary": 1}]}
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/core/v4/keys/all"))
        .and(query_param("Email", "plain@example.com"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"Code": 1000, "RecipientType": 2})),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/mail/v4/messages/draft-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"Code": 1000})))
        .expect(1)
        .mount(&server)
        .await;

    let client = test_support::client(&server, vec![me]);
    let opts = SendOptions {
        to: vec!["friend@proton.me".into(), "plain@example.com".into()],
        body: "FYI".into(),
        ..SendOptions::default()
    };
    client.forward(&parent_id, &opts).await.unwrap();

    let requests = server.received_requests().await.unwrap();
    let send = requests
        .iter()
        .find(|request| request.url.path() == "/mail/v4/messages/draft-1")
        .unwrap();
    let body: Value = serde_json::from_slice(&send.body).unwrap();
    let packages = body["Packages"].as_array().unwrap();
    let package = |kind: i64| {
        packages
            .iter()
            .find(|package| package["Type"] == kind)
            .unwrap()
    };

    let internal = &package(package_type::PM)["Addresses"]["friend@proton.me"];
    let wrapped = internal["AttachmentKeyPackets"]["draft-att"]
        .as_str()
        .expect("the Proton recipient gets the forwarded attachment's key");
    let opened =
        crypto::decrypt_attachment(&provider, &friend, wrapped, &original.data_packet).unwrap();
    assert_eq!(opened, b"forwarded payload");

    let clear = &package(package_type::CLEAR)["AttachmentKeys"]["draft-att"];
    assert_eq!(
        clear["Key"],
        STANDARD.encode(original.material.key.as_slice())
    );
    assert_eq!(clear["Algorithm"], "aes256");
}

/// The body a send package carries, opened with `material`.
fn read_package(package: &Value, material: &SessionKeyMaterial) -> String {
    use proton_crypto::crypto::{
        DataEncoding, Decryptor, DecryptorSync, PGPProviderSync, VerifiedData,
    };

    let provider = crypto::provider();
    let key = provider
        .session_key_import(&material.key, material.algo)
        .unwrap();
    let data = STANDARD.decode(package["Body"].as_str().unwrap()).unwrap();
    let text = provider
        .new_decryptor()
        .with_session_key(key)
        .decrypt(&data, DataEncoding::Bytes)
        .unwrap();
    // Bodies are signed as text, which canonicalizes line ends.
    String::from_utf8(text.as_bytes().to_vec())
        .unwrap()
        .replace("\r\n", "\n")
}

#[tokio::test]
async fn each_recipient_of_a_mixed_send_gets_the_copy_their_keys_allow() {
    use wiremock::matchers::query_param;

    let server = MockServer::start().await;
    let (me, _) = test_address("me@proton.me", "address-1");
    let (friend, friend_public) = test_address("friend@proton.me", "friend-address");
    let (hidden, hidden_public) = test_address("hidden@proton.me", "hidden-address");
    let (outsider, outsider_public) = test_address("pgp@example.org", "outsider-address");
    let provider = crypto::provider();
    let attachment = crypto::encrypt_attachment(&provider, &me, b"attached").unwrap();
    Mock::given(method("POST"))
        .and(path("/mail/v4/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Code": 1000,
            "Message": {
                "ID": "draft-1",
                "Attachments": [{
                    "ID": "draft-att",
                    "Name": "notes.txt",
                    "KeyPackets": STANDARD.encode(&attachment.key_packet),
                }]
            }
        })))
        .mount(&server)
        .await;
    for (email, public) in [
        ("friend@proton.me", &friend_public),
        ("hidden@proton.me", &hidden_public),
    ] {
        Mock::given(method("GET"))
            .and(path("/core/v4/keys/all"))
            .and(query_param("Email", email))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "Code": 1000, "RecipientType": 1,
                "Address": {"Keys": [{"PublicKey": public, "Flags": 3, "Primary": 1}]}
            })))
            .mount(&server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/core/v4/keys/all"))
        .and(query_param("Email", "pgp@example.org"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Code": 1000, "RecipientType": 2,
            "Unverified": {"Keys": [{"PublicKey": outsider_public, "Flags": 3}]}
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/core/v4/keys/all"))
        .and(query_param("Email", "plain@example.com"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"Code": 1000, "RecipientType": 2})),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/mail/v4/messages/draft-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"Code": 1000})))
        .expect(1)
        .mount(&server)
        .await;
    let html = "<p>The <b>plan</b></p>";

    test_support::client(&server, vec![me.clone()])
        .send(&SendOptions {
            to: vec!["friend@proton.me".into()],
            cc: vec!["pgp@example.org".into()],
            bcc: vec!["hidden@proton.me".into(), "plain@example.com".into()],
            body: html.into(),
            html: true,
            ..SendOptions::default()
        })
        .await
        .unwrap();

    let requests = server.received_requests().await.unwrap();
    let sent = |at: &str| -> Value {
        let request = requests
            .iter()
            .find(|request| request.method.as_str() == "POST" && request.url.path() == at)
            .unwrap();
        serde_json::from_slice(&request.body).unwrap()
    };
    let draft = sent("/mail/v4/messages");
    let bcc: Vec<_> = draft["Message"]["BCCList"]
        .as_array()
        .unwrap()
        .iter()
        .map(|recipient| recipient["Address"].as_str().unwrap())
        .collect();
    assert_eq!(bcc, ["hidden@proton.me", "plain@example.com"]);

    let request = sent("/mail/v4/messages/draft-1");
    let packages = request["Packages"].as_array().unwrap();
    let types: Vec<_> = packages
        .iter()
        .map(|package| package["Type"].as_i64().unwrap())
        .collect();
    assert_eq!(
        types,
        [
            package_type::PM,
            package_type::PGP_INLINE,
            package_type::CLEAR
        ]
    );
    let package = |kind: i64| {
        packages
            .iter()
            .find(|package| package["Type"] == kind)
            .unwrap()
    };
    // Who `package` addresses, and what `recipient` opens there: the body
    // and the attachment.
    let open = |package: &Value, recipient: &AddressKeys| {
        let entry = &package["Addresses"][recipient.email.as_str()];
        assert_eq!(entry["Type"], package["Type"]);
        let material = crypto::decrypt_session_key(
            &provider,
            recipient,
            entry["BodyKeyPacket"].as_str().unwrap(),
        )
        .unwrap();
        let wrapped = entry["AttachmentKeyPackets"]["draft-att"].as_str().unwrap();
        let attached =
            crypto::decrypt_attachment(&provider, recipient, wrapped, &attachment.data_packet)
                .unwrap();
        (read_package(package, &material), attached)
    };
    let addressed = |package: &Value| {
        let mut addresses: Vec<_> = package["Addresses"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        addresses.sort();
        addresses
    };

    // Proton recipients, the hidden one too, read the HTML as written.
    let internal = package(package_type::PM);
    assert_eq!(internal["MIMEType"], "text/html");
    assert_eq!(
        addressed(internal),
        ["friend@proton.me", "hidden@proton.me"]
    );
    for recipient in [&friend, &hidden] {
        assert_eq!(
            open(internal, recipient),
            (html.to_owned(), b"attached".to_vec())
        );
    }
    // The package's own key packet is the sender's.
    let body_key =
        crypto::decrypt_session_key(&provider, &me, internal["BodyKeyPacket"].as_str().unwrap())
            .unwrap();
    assert_eq!(read_package(internal, &body_key), html);

    // A published key gets the text version.
    let pgp = package(package_type::PGP_INLINE);
    assert_eq!(pgp["MIMEType"], "text/plain");
    assert_eq!(addressed(pgp), ["pgp@example.org"]);
    assert_eq!(
        open(pgp, &outsider),
        ("The plan".to_owned(), b"attached".to_vec())
    );

    // Without a key, Proton is handed the keys of the same body.
    let clear = package(package_type::CLEAR);
    assert_eq!(clear["MIMEType"], "text/html");
    assert_eq!(addressed(clear), ["plain@example.com"]);
    assert_eq!(
        clear["Addresses"]["plain@example.com"]["Type"],
        package_type::CLEAR
    );
    assert_eq!(clear["Body"], internal["Body"]);
    assert_eq!(
        clear["BodyKey"]["Key"],
        STANDARD.encode(body_key.key.as_slice())
    );
    assert_eq!(
        clear["AttachmentKeys"]["draft-att"]["Key"],
        STANDARD.encode(attachment.material.key.as_slice())
    );
}

#[test]
fn packages_come_in_a_fixed_order_with_password_entries_as_built() {
    let (me, _) = test_address("me@proton.me", "address-1");
    let (_, friend_public) = test_address("friend@proton.me", "friend-address");
    let (_, outsider_public) = test_address("pgp@example.org", "outsider-address");
    let provider = crypto::provider();
    let body = EncryptedBody::encrypt(&provider, &me, "Hello").unwrap();
    let password_entry = json!({"Type": package_type::EO, "Token": "token"});
    let opts = SendOptions {
        body: "Hello".into(),
        ..SendOptions::default()
    };

    let packages = build_packages(
        &provider,
        &me,
        &opts,
        &body,
        &[],
        vec![
            ("plain@example.com", Delivery::Clear),
            (
                "ana@example.com",
                Delivery::Password(password_entry.clone()),
            ),
            ("pgp@example.org", Delivery::PgpInline(&outsider_public)),
            ("friend@proton.me", Delivery::EndToEnd(&friend_public)),
        ],
    )
    .unwrap();

    let types: Vec<_> = packages
        .iter()
        .map(|package| package["Type"].as_i64().unwrap())
        .collect();
    assert_eq!(
        types,
        [
            package_type::PM,
            package_type::PGP_INLINE,
            package_type::EO,
            package_type::CLEAR
        ]
    );
    assert_eq!(packages[2]["Addresses"]["ana@example.com"], password_entry);
    assert!(packages[2].get("BodyKeyPacket").is_none());
    // A plain body reaches PGP/Inline recipients as written.
    assert_eq!(packages[1]["Body"], packages[0]["Body"]);
    // Without attachments, nothing names any.
    assert!(
        packages[0]["Addresses"]["friend@proton.me"]
            .get("AttachmentKeyPackets")
            .is_none()
    );
    assert!(packages[3].get("AttachmentKeys").is_none());

    let none = build_packages(&provider, &me, &opts, &body, &[], Vec::new()).unwrap();
    assert!(none.is_empty());
}

#[tokio::test]
async fn an_html_body_reaches_pgp_inline_recipients_as_plain_text() {
    use wiremock::matchers::query_param;

    let server = MockServer::start().await;
    let (me, _) = test_address("me@proton.me", "address-1");
    let (friend, friend_public) = test_address("friend@proton.me", "friend-address");
    let (outsider, outsider_public) = test_address("pgp@example.org", "outsider-address");
    Mock::given(method("POST"))
        .and(path("/mail/v4/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"Code": 1000, "Message": {"ID": "draft-1"}})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/core/v4/keys/all"))
        .and(query_param("Email", "friend@proton.me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Code": 1000, "RecipientType": 1,
            "Address": {"Keys": [{"PublicKey": friend_public, "Flags": 3, "Primary": 1}]}
        })))
        .mount(&server)
        .await;
    // A key published outside Proton, as through WKD.
    Mock::given(method("GET"))
        .and(path("/core/v4/keys/all"))
        .and(query_param("Email", "pgp@example.org"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Code": 1000, "RecipientType": 2,
            "Unverified": {"Keys": [{"PublicKey": outsider_public, "Flags": 3}]}
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/mail/v4/messages/draft-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"Code": 1000})))
        .expect(1)
        .mount(&server)
        .await;
    let html = "<p>Hi Alex,<br>How are you?</p>\n<p>See you <b>Thursday</b>.</p>";

    test_support::client(&server, vec![me])
        .send(&SendOptions {
            to: vec!["friend@proton.me".into(), "pgp@example.org".into()],
            subject: "Plans".into(),
            body: html.into(),
            html: true,
            ..SendOptions::default()
        })
        .await
        .unwrap();

    let requests = server.received_requests().await.unwrap();
    let send = requests
        .iter()
        .find(|request| request.url.path() == "/mail/v4/messages/draft-1")
        .unwrap();
    let body: Value = serde_json::from_slice(&send.body).unwrap();
    let packages = body["Packages"].as_array().unwrap();
    let provider = crypto::provider();
    // What `recipient` reads in the package of `kind`, and its stated type.
    let open = |kind: i64, recipient: &AddressKeys| {
        let package = packages
            .iter()
            .find(|package| package["Type"] == kind)
            .unwrap();
        let entry = &package["Addresses"][recipient.email.as_str()];
        let material = crypto::decrypt_session_key(
            &provider,
            recipient,
            entry["BodyKeyPacket"].as_str().unwrap(),
        )
        .unwrap();
        (
            read_package(package, &material),
            package["MIMEType"].as_str().unwrap().to_owned(),
        )
    };

    assert_eq!(
        open(package_type::PM, &friend),
        (html.to_owned(), "text/html".to_owned())
    );
    assert_eq!(
        open(package_type::PGP_INLINE, &outsider),
        (
            "Hi Alex,\nHow are you?\n\nSee you Thursday.".to_owned(),
            "text/plain".to_owned()
        )
    );
}

#[tokio::test]
async fn password_protected_recipients_get_keys_that_open_with_the_password() {
    use proton_crypto::crypto::{
        DataEncoding, Decryptor, DecryptorSync, PGPProviderSync, VerifiedData,
    };
    use wiremock::matchers::query_param;

    // A modulus signed with Proton's key, from proton-srp's own tests.
    const MODULUS: &str = "-----BEGIN PGP SIGNED MESSAGE-----\nHash: SHA256\n\nW2z5HBi8RvsfYzZTS7qBaUxxPhsfHJFZpu3Kd6s1JafNrCCH9rfvPLrfuqocxWPgWDH2R8neK7PkNvjxto9TStuY5z7jAzWRvFWN9cQhAKkdWgy0JY6ywVn22+HFpF4cYesHrqFIKUPDMSSIlWjBVmEJZ/MusD44ZT29xcPrOqeZvwtCffKtGAIjLYPZIEbZKnDM1Dm3q2K/xS5h+xdhjnndhsrkwm9U9oyA2wxzSXFL+pdfj2fOdRwuR5nW0J2NFrq3kJjkRmpO/Genq1UW+TEknIWAb6VzJJJA244K/H8cnSx2+nSNZO3bbo6Ys228ruV9A8m6DhxmS+bihN3ttQ==\n-----BEGIN PGP SIGNATURE-----\nVersion: ProtonMail\nComment: https://protonmail.com\n\nwl4EARYIABAFAlwB1j0JEDUFhcTpUY8mAAD8CgEAnsFnF4cF0uSHKkXa1GIa\nGO86yMV4zDZEZcDSJo0fgr8A/AlupGN9EdHlsrZLmTA1vhIx+rOgxdEff28N\nkvNM7qIK\n=q6vu\n-----END PGP SIGNATURE-----";
    let server = MockServer::start().await;
    let (me, _) = test_address("me@proton.me", "address-1");
    let provider = crypto::provider();
    let attachment = crypto::encrypt_attachment(&provider, &me, b"attached").unwrap();
    Mock::given(method("POST"))
        .and(path("/mail/v4/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Code": 1000,
            "Message": {
                "ID": "draft-1",
                "Attachments": [{
                    "ID": "draft-att",
                    "Name": "notes.txt",
                    "KeyPackets": STANDARD.encode(&attachment.key_packet),
                }]
            }
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/core/v4/keys/all"))
        .and(query_param("Email", "ana@example.com"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"Code": 1000, "RecipientType": 2})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/core/v4/auth/modulus"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Code": 1000, "Modulus": MODULUS, "ModulusID": "modulus-1"
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/mail/v4/messages/draft-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"Code": 1000})))
        .expect(1)
        .mount(&server)
        .await;

    let client = test_support::client(&server, vec![me]);
    let opts = SendOptions {
        to: vec!["ana@example.com".into()],
        body: "the plan".into(),
        ..SendOptions::default()
    };
    client
        .send_eo(&opts, "correct horse", Some("our film"))
        .await
        .unwrap();

    let requests = server.received_requests().await.unwrap();
    let send = requests
        .iter()
        .find(|request| request.url.path() == "/mail/v4/messages/draft-1")
        .unwrap();
    let body: Value = serde_json::from_slice(&send.body).unwrap();
    let package = body["Packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|package| package["Type"] == package_type::EO)
        .unwrap();
    let data_packet = STANDARD.decode(package["Body"].as_str().unwrap()).unwrap();
    let open_key = |packet: &Value| {
        let packet = STANDARD.decode(packet.as_str().unwrap()).unwrap();
        provider
            .new_decryptor()
            .with_passphrase("correct horse")
            .decrypt_session_key(packet)
            .unwrap()
    };
    let entry = &package["Addresses"]["ana@example.com"];
    assert_eq!(entry["Type"], package_type::EO);
    assert_eq!(entry["PasswordHint"], "our film");
    assert_eq!(entry["Auth"]["ModulusID"], "modulus-1");
    let token = provider
        .new_decryptor()
        .with_passphrase("correct horse")
        .decrypt(entry["EncToken"].as_str().unwrap(), DataEncoding::Armor)
        .unwrap();
    assert_eq!(
        token.as_bytes(),
        entry["Token"].as_str().unwrap().as_bytes()
    );
    let opened = provider
        .new_decryptor()
        .with_session_key(open_key(&entry["BodyKeyPacket"]))
        .decrypt(&data_packet, DataEncoding::Bytes)
        .unwrap();
    assert_eq!(opened.as_bytes(), b"the plan");
    let attachment_key = open_key(&entry["AttachmentKeyPackets"]["draft-att"]);
    let (key, _) = provider.session_key_export(&attachment_key).unwrap();
    assert_eq!(key.as_ref(), attachment.material.key.as_slice());
}

#[test]
fn replies_go_to_reply_to_or_the_sender_and_reply_all_skips_own_addresses() {
    let meta = MessageMetadata {
        sender: Recipient::new("alex@corp.example"),
        to_list: vec![
            Recipient::new("Work@Acme.com"),
            Recipient::new("bob@corp.example"),
        ],
        cc_list: vec![Recipient::new("carol@corp.example")],
        ..Default::default()
    };
    let own = vec!["work@acme.com".to_owned()];

    let plain = reply_recipients(&meta, &[], &own);
    assert_eq!(plain.to, ["alex@corp.example"]);
    assert_eq!(plain.cc, ["bob@corp.example", "carol@corp.example"]);

    let redirected = reply_recipients(&meta, &[Recipient::new("attacker@evil.example")], &own);
    assert_eq!(redirected.to, ["attacker@evil.example"]);
    assert_eq!(redirected.cc, plain.cc);
    assert!(reply_redirected(
        &meta,
        &[Recipient::new("attacker@evil.example")]
    ));
    assert!(!reply_redirected(&meta, &[]));
    assert!(!reply_redirected(
        &meta,
        &[Recipient::new("Alex@Corp.example")]
    ));
}

#[test]
fn a_reply_to_your_own_sent_message_goes_to_its_recipients() {
    use crate::model::enums::message_flag::{RECEIVED, SENT};

    let sent = MessageMetadata {
        sender: Recipient::new("me@proton.me"),
        to_list: vec![Recipient::new("bob@corp.example")],
        cc_list: vec![
            Recipient::new("carol@corp.example"),
            Recipient::new("Me@Proton.me"),
        ],
        flags: SENT,
        ..Default::default()
    };
    let own = vec!["me@proton.me".to_owned()];
    // Proton sets the sender as Reply-To on mail the account sent.
    let reply_tos = [Recipient::new("me@proton.me")];

    let answer = reply_recipients(&sent, &reply_tos, &own);
    assert_eq!(answer.to, ["bob@corp.example"]);
    assert_eq!(answer.cc, ["carol@corp.example"]);
    assert!(
        !reply_redirected(&sent, &reply_tos),
        "your own message redirects nothing"
    );

    // A note to yourself, sent and received, still answers yourself.
    let note = MessageMetadata {
        to_list: vec![Recipient::new("me@proton.me")],
        cc_list: Vec::new(),
        flags: SENT | RECEIVED,
        ..sent
    };
    assert_eq!(
        reply_recipients(&note, &reply_tos, &own).to,
        ["me@proton.me"]
    );
}

#[tokio::test]
async fn answers_leave_from_the_address_the_original_reached() {
    let server = MockServer::start().await;
    let (personal, _) = test_address("me@proton.me", "address-1");
    let (work, _) = test_address("work@acme.com", "work-address");
    let parent_id = format!("{}==", "p".repeat(60));
    Mock::given(method("GET"))
        .and(path(format!("/mail/v4/messages/{parent_id}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Code": 1000,
            "Message": {
                "ID": parent_id, "AddressID": "work-address", "Subject": "Invoice",
                "Sender": {"Name": "Boss", "Address": "boss@corp.example"},
                "ToList": [{"Name": "Work", "Address": "work@acme.com"}]
            }
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/mail/v4/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"Code": 1000, "Message": {"ID": "draft-1"}})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/core/v4/keys/all"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"Code": 1000, "RecipientType": 2})),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/mail/v4/messages/draft-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"Code": 1000})))
        .mount(&server)
        .await;

    let client = test_support::client(&server, vec![personal, work]);
    let reply = SendOptions {
        body: "Thanks".into(),
        ..SendOptions::default()
    };
    client.reply(&parent_id, false, &reply).await.unwrap();
    let forward = SendOptions {
        to: vec!["friend@example.com".into()],
        body: "FYI".into(),
        ..SendOptions::default()
    };
    client.forward(&parent_id, &forward).await.unwrap();
    // A sender chosen explicitly still wins.
    let chosen = SendOptions {
        from: Some("me@proton.me".into()),
        ..reply
    };
    client.reply(&parent_id, false, &chosen).await.unwrap();

    let senders: Vec<String> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|request| request.url.path() == "/mail/v4/messages")
        .map(|request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            body["Message"]["Sender"]["Address"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(senders, ["work@acme.com", "work@acme.com", "me@proton.me"]);
}

#[tokio::test]
async fn answers_tell_proton_how_they_answer_and_mark_the_subject_once() {
    let server = MockServer::start().await;
    let (me, _) = test_address("me@proton.me", "address-1");
    let parent_id = format!("{}==", "p".repeat(60));
    Mock::given(method("GET"))
        .and(path(format!("/mail/v4/messages/{parent_id}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Code": 1000,
            "Message": {
                "ID": parent_id, "AddressID": "address-1",
                "Subject": "Reunión del lunes",
                "Sender": {"Name": "Boss", "Address": "boss@corp.example"},
                "ToList": [{"Name": "Me", "Address": "me@proton.me"}],
                "CCList": [{"Name": "Team", "Address": "team@corp.example"}]
            }
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/mail/v4/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"Code": 1000, "Message": {"ID": "draft-1"}})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/core/v4/keys/all"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"Code": 1000, "RecipientType": 2})),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/mail/v4/messages/draft-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"Code": 1000})))
        .mount(&server)
        .await;

    let client = test_support::client(&server, vec![me]);
    let answer = SendOptions {
        body: "Thanks".into(),
        ..SendOptions::default()
    };
    client.reply(&parent_id, false, &answer).await.unwrap();
    client.reply(&parent_id, true, &answer).await.unwrap();
    let forward = SendOptions {
        to: vec!["friend@example.com".into()],
        ..answer
    };
    client.forward(&parent_id, &forward).await.unwrap();

    let drafts: Vec<(Value, Value)> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|request| request.url.path() == "/mail/v4/messages")
        .map(|request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            (body["Action"].clone(), body["Message"]["Subject"].clone())
        })
        .collect();
    // Proton's actions: 0 replies, 1 replies to all, 2 forwards.
    assert_eq!(
        drafts,
        [
            (json!(0), json!("Re: Reunión del lunes")),
            (json!(1), json!("Re: Reunión del lunes")),
            (json!(2), json!("Fw: Reunión del lunes")),
        ]
    );
}

#[tokio::test]
async fn a_reply_goes_where_its_writer_chose_or_else_where_the_original_asks() {
    let server = MockServer::start().await;
    let (me, _) = test_address("me@proton.me", "address-1");
    let parent_id = format!("{}==", "p".repeat(60));
    Mock::given(method("GET"))
        .and(path(format!("/mail/v4/messages/{parent_id}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Code": 1000,
            "Message": {
                "ID": parent_id, "AddressID": "address-1", "Subject": "Plans",
                "Sender": {"Name": "Boss", "Address": "boss@corp.example"},
                "ToList": [{"Name": "Me", "Address": "me@proton.me"}],
                "CCList": [{"Name": "Team", "Address": "team@corp.example"}]
            }
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/mail/v4/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"Code": 1000, "Message": {"ID": "draft-1"}})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/core/v4/keys/all"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"Code": 1000, "RecipientType": 2})),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/mail/v4/messages/draft-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"Code": 1000})))
        .mount(&server)
        .await;

    let client = test_support::client(&server, vec![me]);
    let unaddressed = SendOptions {
        body: "Thanks".into(),
        ..SendOptions::default()
    };
    client.reply(&parent_id, true, &unaddressed).await.unwrap();
    // Someone added, someone left out: the writer's list is the one used.
    let chosen = SendOptions {
        to: vec!["carol@corp.example".into()],
        bcc: vec!["dave@corp.example".into()],
        ..unaddressed.clone()
    };
    client.reply(&parent_id, true, &chosen).await.unwrap();
    let copy_only = SendOptions {
        cc: vec!["erin@corp.example".into()],
        ..unaddressed
    };
    client.reply(&parent_id, false, &copy_only).await.unwrap();

    let addressed: Vec<[Vec<String>; 3]> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|request| request.url.path() == "/mail/v4/messages")
        .map(|request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            ["ToList", "CCList", "BCCList"].map(|list| {
                body["Message"][list]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|recipient| recipient["Address"].as_str().unwrap().to_owned())
                    .collect()
            })
        })
        .collect();
    let list = |addresses: &[&str]| -> Vec<String> {
        addresses
            .iter()
            .map(|address| (*address).to_owned())
            .collect()
    };
    assert_eq!(
        addressed,
        [
            [
                list(&["boss@corp.example"]),
                list(&["team@corp.example"]),
                list(&[])
            ],
            [
                list(&["carol@corp.example"]),
                list(&[]),
                list(&["dave@corp.example"])
            ],
            [list(&[]), list(&["erin@corp.example"]), list(&[])],
        ]
    );
}

#[tokio::test]
async fn an_answer_quotes_the_original_it_already_fetched() {
    use wiremock::matchers::query_param;

    let server = MockServer::start().await;
    let (me, _) = test_address("me@proton.me", "address-1");
    let provider = crypto::provider();
    let parent_id = format!("{}==", "p".repeat(60));
    let body = crypto::encrypt_self_draft(&provider, &me, "Numbers attached").unwrap();
    Mock::given(method("GET"))
        .and(path(format!("/mail/v4/messages/{parent_id}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Code": 1000,
            "Message": {
                "ID": parent_id, "AddressID": "address-1", "Subject": "Report",
                "Sender": {"Name": "Boss", "Address": "boss@proton.me"},
                "Body": body, "MIMEType": "text/plain"
            }
        })))
        .expect(1)
        .mount(&server)
        .await;
    // Quoting needs no verdict, so the original's sender is never asked for.
    Mock::given(method("GET"))
        .and(path("/core/v4/keys/all"))
        .and(query_param("Email", "boss@proton.me"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"Code": 1000, "RecipientType": 2})),
        )
        .expect(0)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/core/v4/keys/all"))
        .and(query_param("Email", "friend@example.com"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"Code": 1000, "RecipientType": 2})),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/mail/v4/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"Code": 1000, "Message": {"ID": "draft-1"}})),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/mail/v4/messages/draft-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"Code": 1000})))
        .mount(&server)
        .await;

    let client = test_support::client(&server, vec![me.clone()]);
    let forward = SendOptions {
        to: vec!["friend@example.com".into()],
        body: "FYI".into(),
        ..SendOptions::default()
    };
    client.forward(&parent_id, &forward).await.unwrap();

    let requests = server.received_requests().await.unwrap();
    let draft = requests
        .iter()
        .find(|request| request.url.path() == "/mail/v4/messages")
        .unwrap();
    let draft: Value = serde_json::from_slice(&draft.body).unwrap();
    let (text, _) = crypto::decrypt_body(
        &provider,
        &me,
        &[],
        draft["Message"]["Body"].as_str().unwrap(),
    )
    .unwrap();
    // The draft is signed as text, which canonicalizes line ends.
    let text = text.replace("\r\n", "\n");
    assert!(text.starts_with("FYI\n\n"), "{text}");
    assert!(text.contains("> Numbers attached"), "{text}");
}

/// The text of the draft that answering a message from the boss writes,
/// where `parent_body` is the original's body as Proton returns it: a
/// reply when `reply`, else a forward to a friend.
async fn answer_text(
    reply: bool,
    quote: Quote,
    parent_body: impl FnOnce(&AddressKeys) -> String,
) -> String {
    let server = MockServer::start().await;
    let (me, _) = test_address("me@proton.me", "address-1");
    let parent_id = format!("{}==", "p".repeat(60));
    Mock::given(method("GET"))
        .and(path(format!("/mail/v4/messages/{parent_id}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Code": 1000,
            "Message": {
                "ID": parent_id, "AddressID": "address-1", "Subject": "Report",
                "Sender": {"Name": "Boss", "Address": "boss@proton.me"},
                "Body": parent_body(&me), "MIMEType": "text/plain"
            }
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/core/v4/keys/all"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"Code": 1000, "RecipientType": 2})),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/mail/v4/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"Code": 1000, "Message": {"ID": "draft-1"}})),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/mail/v4/messages/draft-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"Code": 1000})))
        .mount(&server)
        .await;

    let client = test_support::client(&server, vec![me.clone()]);
    if reply {
        let thanks = SendOptions {
            body: "Thanks".into(),
            quote,
            ..SendOptions::default()
        };
        client.reply(&parent_id, false, &thanks).await.unwrap();
    } else {
        let forward = SendOptions {
            to: vec!["friend@example.com".into()],
            body: "FYI".into(),
            quote,
            ..SendOptions::default()
        };
        client.forward(&parent_id, &forward).await.unwrap();
    }

    let requests = server.received_requests().await.unwrap();
    let draft = requests
        .iter()
        .find(|request| request.url.path() == "/mail/v4/messages")
        .unwrap();
    let draft: Value = serde_json::from_slice(&draft.body).unwrap();
    let (text, _) = crypto::decrypt_body(
        &crypto::provider(),
        &me,
        &[],
        draft["Message"]["Body"].as_str().unwrap(),
    )
    .unwrap();
    // The draft is signed as text, which canonicalizes line ends.
    text.replace("\r\n", "\n")
}

#[tokio::test]
async fn a_reply_quotes_the_original_under_its_own_text() {
    let text = answer_text(true, Quote::Original, |me| {
        crypto::encrypt_self_draft(&crypto::provider(), me, "Numbers attached").unwrap()
    })
    .await;
    assert!(text.starts_with("Thanks\n\n"), "{text}");
    assert!(text.contains("> Numbers attached"), "{text}");
}

#[tokio::test]
async fn an_answer_can_leave_the_original_out() {
    for reply in [true, false] {
        let text = answer_text(reply, Quote::Omitted, |me| {
            crypto::encrypt_self_draft(&crypto::provider(), me, "Numbers attached").unwrap()
        })
        .await;
        let own = if reply { "Thanks" } else { "FYI" };
        assert_eq!(text, own, "the original went with it");
    }
}

#[tokio::test]
async fn an_original_that_does_not_open_is_answered_without_a_quote() {
    for reply in [true, false] {
        let text = answer_text(reply, Quote::Original, |_| {
            "not an encrypted message".to_owned()
        })
        .await;
        let own = if reply { "Thanks" } else { "FYI" };
        assert_eq!(text, own, "the answer still goes, unquoted");
    }
}

#[tokio::test]
async fn a_draft_attachment_without_a_key_stops_the_send() {
    let server = MockServer::start().await;
    let (me, _) = test_address("me@proton.me", "address-1");
    Mock::given(method("POST"))
        .and(path("/mail/v4/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Code": 1000,
            "Message": {"ID": "draft-1", "Attachments": [{"ID": "draft-att"}]}
        })))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/mail/v4/messages/delete"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"Code": 1000})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/core/v4/keys/all"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"Code": 1000, "RecipientType": 2})),
        )
        .mount(&server)
        .await;

    let client = test_support::client(&server, vec![me]);
    let opts = SendOptions {
        to: vec!["friend@proton.me".into()],
        ..SendOptions::default()
    };
    assert!(matches!(client.send(&opts).await, Err(Error::Crypto(_))));
    server.verify().await;
    assert!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|request| request.url.path() != "/mail/v4/messages/draft-1"),
        "an attachment nobody could open is never sent"
    );
}

struct OversizedResponse;

struct ShortDeadline(HttpClient);

#[async_trait]
impl Doer for ShortDeadline {
    async fn do_raw(&self, req: Request) -> Result<Response> {
        self.0.do_raw(req.timeout(Duration::from_millis(100))).await
    }

    async fn decode<T: DeserializeOwned>(&self, req: Request) -> Result<T> {
        self.0.decode(req.timeout(Duration::from_millis(100))).await
    }
}

#[async_trait]
impl Doer for OversizedResponse {
    async fn do_raw(&self, _req: Request) -> Result<Response> {
        Err(Error::ResponseTooLarge { limit: 1 })
    }

    async fn decode<T: DeserializeOwned>(&self, _req: Request) -> Result<T> {
        Err(Error::ResponseTooLarge { limit: 1 })
    }
}

#[tokio::test]
async fn oversized_send_response_leaves_send_unconfirmed() {
    let error = submit_send(&OversizedResponse, "draft-1", json!({"Packages": []}))
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        Error::SendUnconfirmed { message_id, source }
            if message_id == "draft-1" && matches!(*source, Error::ResponseTooLarge { .. })
    ));
}

#[tokio::test]
async fn incomplete_final_response_leaves_send_unconfirmed() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0; 4096];
        let size = socket.read(&mut request).await.unwrap();
        assert!(request[..size].starts_with(b"POST /mail/v4/messages/draft-1 "));
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n{\"Code\":1000}")
            .await
            .unwrap();
    });

    let http = HttpClient::new(format!("http://{addr}"), "Other");
    let error = submit_send(&http, "draft-1", json!({"Packages": []}))
        .await
        .unwrap_err();
    server.await.unwrap();
    assert!(matches!(
        error,
        Error::SendUnconfirmed { message_id, source }
            if message_id == "draft-1" && matches!(*source, Error::Http(_))
    ));
}

#[tokio::test]
async fn timed_out_final_send_leaves_send_unconfirmed() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (received, waiting) = tokio::sync::oneshot::channel::<()>();
    let (release, held) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let expected = b"POST /mail/v4/messages/draft-1 ";
        let mut prefix = vec![0; expected.len()];
        socket.read_exact(&mut prefix).await.unwrap();
        assert_eq!(prefix, expected);
        received.send(()).unwrap();
        let _ = held.await;
    });

    let http = ShortDeadline(HttpClient::new(format!("http://{addr}"), "Other"));
    let body = json!({"Packages": []});
    let send = tokio::spawn(async move { submit_send(&http, "draft-1", body).await });
    waiting.await.unwrap();
    let error = tokio::time::timeout(Duration::from_secs(2), send)
        .await
        .expect("send should finish after the transport deadline")
        .unwrap()
        .unwrap_err();
    let _ = release.send(());
    server.await.unwrap();
    assert!(matches!(
        error,
        Error::SendUnconfirmed { message_id, source }
            if message_id == "draft-1"
                && matches!(*source, Error::Http(ref error) if error.is_timeout())
    ));
}

#[tokio::test]
async fn explicit_send_rejection_remains_a_regular_api_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/mail/v4/messages/draft-1"))
        .respond_with(ResponseTemplate::new(422).set_body_json(json!({
            "Code": 1200, "Error": "invalid recipients"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let http = HttpClient::new(server.uri(), "Other");
    let error = submit_send(&http, "draft-1", json!({"Packages": []}))
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Api(api) if api.http_status == 422));
}

#[tokio::test]
async fn uncertain_send_does_not_delete_the_draft() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/mail/v4/messages/delete"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"Code": 1000})))
        .expect(1)
        .mount(&server)
        .await;
    let http = HttpClient::new(server.uri(), "Other");
    let unknown = Error::SendUnconfirmed {
        message_id: "draft-1".into(),
        source: Box::new(Error::Other("response lost".into())),
    };

    cleanup_failed_draft(&http, "draft-1", &unknown).await;
    assert!(server.received_requests().await.unwrap().is_empty());
    cleanup_failed_draft(&http, "draft-1", &Error::Other("rejected".into())).await;
    server.verify().await;
}

#[test]
fn dedupe_case_insensitive() {
    let a = vec!["A@x.com".to_string(), "b@x.com".to_string()];
    let b = vec!["a@x.com".to_string(), "c@x.com".to_string()];
    let out = dedupe(&[&a, &b]);
    assert_eq!(out, vec!["A@x.com", "b@x.com", "c@x.com"]);
}

#[test]
fn subject_prefix() {
    assert_eq!(reply_subject("Hello"), "Re: Hello");
    assert_eq!(reply_subject("Re: Hello"), "Re: Hello");
    assert_eq!(reply_subject("RE: Hello"), "RE: Hello");
    // Words that only begin like a prefix answer nothing yet.
    for subject in ["Reunión", "Resumen", "Recordatorio", "Request"] {
        assert_eq!(reply_subject(subject), format!("Re: {subject}"));
    }
    assert_eq!(forward_subject("Report"), "Fw: Report");
    assert_eq!(forward_subject("FW: Report"), "FW: Report");
    assert_eq!(forward_subject("Fwd: Report"), "Fwd: Report");
    assert_eq!(forward_subject("Fwiw, it works"), "Fw: Fwiw, it works");
}

#[test]
fn html_quote_escapes_sender_and_plain_text_body() {
    let mut parent = crate::model::message::Message::default();
    parent.meta.sender.name = "Eve <img src=x>".into();
    parent.meta.sender.address = "a&b@example.test".into();
    let quoted = quote_block(
        &parent,
        "<img src=\"https://example.test/t\"> & hi",
        "text/plain",
        true,
    );

    assert!(quoted.contains("Eve &lt;img src=x&gt; &lt;a&amp;b@example.test&gt;"));
    assert!(quoted.contains("&lt;img src=&quot;https://example.test/t&quot;&gt; &amp; hi"));
    assert!(!quoted.contains("<img"));
}

#[test]
fn html_quote_preserves_sanitized_html_body() {
    let parent = crate::model::message::Message::default();
    let quoted = quote_block(&parent, "<b>Original</b>", "text/html; charset=UTF-8", true);
    assert!(quoted.contains(
        "<blockquote class=\"protonmail_quote\" type=\"cite\"><b>Original</b></blockquote>"
    ));
}

#[test]
fn plain_quote_keeps_original_text() {
    let parent = crate::model::message::Message::default();
    let quoted = quote_block(&parent, "<b>Original</b>", "text/plain", false);
    assert!(quoted.contains("> <b>Original</b>\n"));
}

#[test]
fn plain_quote_of_an_html_original_reads_as_text() {
    let parent = crate::model::message::Message::default();
    let quoted = quote_block(
        &parent,
        "<p><b>Numbers</b> attached</p><p>Second line</p>",
        "text/html; charset=UTF-8",
        false,
    );
    assert!(quoted.contains("> Numbers attached\n"), "{quoted}");
    assert!(quoted.contains("> Second line\n"), "{quoted}");
    assert!(!quoted.contains('<'), "{quoted}");
}

#[test]
fn mime_guess() {
    assert_eq!(guess_mime(std::path::Path::new("a.pdf")), "application/pdf");
    assert_eq!(
        guess_mime(std::path::Path::new("a.unknownext")),
        "application/octet-stream"
    );
}
