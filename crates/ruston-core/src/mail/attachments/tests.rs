use super::*;
use crate::crypto::{AddressKeys, StoredKey};
use crate::mail::test_support;
use crate::model::enums::key_flag;
use crate::model::message::Message;
use base64::{Engine, engine::general_purpose::STANDARD};
use proton_crypto::crypto::{
    ArmorerSync, DataEncoding, KeyGenerator, KeyGeneratorAlgorithm, KeyGeneratorSync,
    PGPProviderSync,
};
use secrecy::SecretString;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn attachment_client(server: &MockServer) -> (Client, Attachment) {
    attachment_client_with_key_flags(server, key_flag::NOT_COMPROMISED | key_flag::NOT_OBSOLETE)
        .await
}

/// A client for sender@proton.me, whose one key Proton reports with
/// `flags`, and a signed attachment from that sender.
async fn attachment_client_with_key_flags(server: &MockServer, flags: u32) -> (Client, Attachment) {
    let provider = crypto::provider();
    let key = provider
        .new_key_generator()
        .with_user_id("Test", "sender@proton.me")
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
    let address = AddressKeys {
        address_id: "address".into(),
        email: "sender@proton.me".into(),
        keys: vec![StoredKey {
            armored,
            passphrase: SecretString::from("pass"),
            unlocked: None,
        }],
    };
    let upload = crypto::encrypt_attachment(&provider, &address, b"attachment payload").unwrap();
    let signature = String::from_utf8(
        provider
            .armorer()
            .armor_signature(&upload.signature)
            .unwrap(),
    )
    .unwrap();
    Mock::given(method("GET"))
        .and(path("/mail/v4/attachments/attachment"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(upload.data_packet))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/core/v4/keys/all"))
        .and(query_param("Email", "sender@proton.me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "Code": 1000, "Address": { "Keys": [{ "PublicKey": public, "Flags": flags }] }
        })))
        .mount(server)
        .await;
    let client = test_support::client(server, vec![address]);
    (
        client,
        Attachment {
            id: "attachment".into(),
            name: "file.txt".into(),
            key_packets: Some(STANDARD.encode(upload.key_packet)),
            signature: Some(signature),
            ..Default::default()
        },
    )
}

#[tokio::test]
async fn an_attachment_too_large_to_open_is_not_downloaded() {
    let server = MockServer::start().await;
    let (client, mut attachment) = attachment_client(&server).await;
    let message = Message {
        meta: crate::model::message::MessageMetadata {
            sender: crate::model::message::Recipient::new("sender@proton.me"),
            address_id: "address".into(),
            ..Default::default()
        },
        ..Default::default()
    };
    attachment.size = api::attachments::MAX_ATTACHMENT_RESPONSE_BYTES as u64 + 1;

    let error = client
        .download_listed_attachment(&message.meta, &attachment)
        .await
        .unwrap_err();
    assert!(
        matches!(error, Error::AttachmentTooLarge { limit } if limit == MAX_ATTACHMENT_PLAINTEXT_BYTES),
        "{error}"
    );
    assert!(server.received_requests().await.unwrap().is_empty());

    // Up to the response limit it is fetched as before.
    attachment.size = api::attachments::MAX_ATTACHMENT_RESPONSE_BYTES as u64;
    assert!(
        client
            .download_listed_attachment(&message.meta, &attachment)
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn attachment_with_a_malformed_signature_is_not_returned() {
    let server = MockServer::start().await;
    let (client, mut attachment) = attachment_client(&server).await;
    attachment.signature = Some("not a PGP signature".into());
    let message = Message {
        meta: crate::model::message::MessageMetadata {
            sender: crate::model::message::Recipient::new("sender@proton.me"),
            address_id: "address".into(),
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(
        client
            .download_listed_attachment(&message.meta, &attachment)
            .await
            .is_err(),
        "plaintext must not be returned without checking a present signature"
    );
}

fn message_with_attachment(attachment: Attachment) -> Message {
    Message {
        meta: crate::model::message::MessageMetadata {
            id: "message".into(),
            sender: crate::model::message::Recipient::new("sender@proton.me"),
            address_id: "address".into(),
            ..Default::default()
        },
        attachments: vec![attachment],
        ..Default::default()
    }
}

#[tokio::test]
async fn verified_and_unsigned_attachments_remain_downloadable() {
    let server = MockServer::start().await;
    let (client, mut attachment) = attachment_client(&server).await;
    let message = message_with_attachment(attachment.clone());
    for signature in [attachment.signature.clone(), None, Some(String::new())] {
        attachment.signature = signature;
        let (name, bytes) = client
            .download_listed_attachment(&message.meta, &attachment)
            .await
            .unwrap();
        assert_eq!(name, "file.txt");
        assert_eq!(bytes, b"attachment payload");
    }
    // Unsigned downloads neither fetch nor imply verification keys.
    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.url.path() == "/core/v4/keys/all")
            .count(),
        1
    );
}

#[tokio::test]
async fn an_attachment_listed_by_a_read_downloads_without_its_message() {
    let server = MockServer::start().await;
    let (client, attachment) = attachment_client(&server).await;
    let message = message_with_attachment(attachment.clone());
    Mock::given(method("GET"))
        .and(path("/mail/v4/messages/message"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;

    let (name, bytes) = client
        .download_listed_attachment(&message.meta, &attachment)
        .await
        .unwrap();

    assert_eq!(
        (name.as_str(), bytes.as_slice()),
        ("file.txt", &b"attachment payload"[..])
    );
}

#[tokio::test]
async fn signed_attachment_without_sender_keys_is_rejected_before_download() {
    let server = MockServer::start().await;
    let (client, attachment) = attachment_client(&server).await;
    let mut message = message_with_attachment(attachment);
    message.meta.sender.address = "unknown@example.com".into();
    // Proton answers an address with no keys of its own.
    Mock::given(method("GET"))
        .and(path("/core/v4/keys/all"))
        .and(query_param("Email", "unknown@example.com"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "Code": 1000, "Address": { "Keys": [] }, "RecipientType": 2
        })))
        .mount(&server)
        .await;
    assert!(matches!(
        client
            .download_listed_attachment(&message.meta, &message.attachments[0])
            .await,
        Err(Error::AttachmentVerificationFailed)
    ));
    assert!(
        !server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .any(|request| request.url.path().starts_with("/mail/v4/attachments/"))
    );
}

#[tokio::test]
async fn a_compromised_sender_key_never_verifies() {
    let server = MockServer::start().await;
    let (client, attachment) =
        attachment_client_with_key_flags(&server, key_flag::NOT_OBSOLETE).await;
    let mut message = message_with_attachment(attachment);
    assert!(matches!(
        client
            .download_listed_attachment(&message.meta, &message.attachments[0])
            .await,
        Err(Error::AttachmentVerificationFailed)
    ));

    // Nor does it vouch for a body the key signed.
    let provider = crypto::provider();
    let address = client.keys().primary_address().unwrap();
    message.body = crypto::encrypt_self_draft(&provider, address, "body").unwrap();
    message.mime_type = "text/plain".into();
    let read = client.open_message(message).await.unwrap();
    assert_eq!(read.body, "body");
    assert_eq!(read.verdict, crypto::Verdict::Unverified);
}

#[tokio::test]
async fn a_failed_sender_key_lookup_is_reported_as_itself() {
    let server = MockServer::start().await;
    let (client, attachment) = attachment_client(&server).await;
    Mock::given(method("GET"))
        .and(path("/core/v4/keys/all"))
        .respond_with(ResponseTemplate::new(503))
        .with_priority(1)
        .mount(&server)
        .await;
    let mut message = message_with_attachment(attachment);

    // Not a verification failure: the lookup can be retried.
    assert!(matches!(
        client
            .download_listed_attachment(&message.meta, &message.attachments[0])
            .await,
        Err(Error::Api(ref error)) if error.http_status == 503
    ));
    assert!(
        !server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .any(|request| request.url.path().starts_with("/mail/v4/attachments/"))
    );

    // A body still opens, without claiming a verified signature.
    let provider = crypto::provider();
    let address = client.keys().primary_address().unwrap();
    message.body = crypto::encrypt_self_draft(&provider, address, "body").unwrap();
    message.mime_type = "text/plain".into();
    let read = client.open_message(message).await.unwrap();
    assert_eq!(read.body, "body");
    assert_eq!(read.verdict, crypto::Verdict::Unverified);
}

#[tokio::test]
async fn invalid_signatures_never_reach_download_callbacks_or_exported_files() {
    let server = MockServer::start().await;
    let (client, mut attachment) = attachment_client(&server).await;
    let provider = crypto::provider();
    let address = client.keys().primary_address().unwrap();
    // The encrypted data is intact, but this otherwise valid signature was
    // made over different plaintext. Decryption alone cannot detect that.
    let other = crypto::encrypt_attachment(&provider, address, b"different payload").unwrap();
    attachment.signature = Some(
        String::from_utf8(
            provider
                .armorer()
                .armor_signature(&other.signature)
                .unwrap(),
        )
        .unwrap(),
    );
    let mut message = message_with_attachment(attachment);
    message.body = crypto::encrypt_self_draft(&provider, address, "body").unwrap();
    message.mime_type = "text/plain".into();
    Mock::given(method("GET"))
        .and(path("/mail/v4/messages/message"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"Code": 1000, "Message": message})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/mail/v4/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            serde_json::json!({"Code": 1000, "Total": 1, "Messages": [message.meta]}),
        ))
        .mount(&server)
        .await;
    assert!(matches!(
        client.download_attachment("message", "attachment").await,
        Err(Error::AttachmentVerificationFailed)
    ));
    let mut delivered = false;
    let result = client
        .for_each_attachment("message", true, |_, _| {
            delivered = true;
            Ok(())
        })
        .await;
    assert!(matches!(result, Err(Error::AttachmentVerificationFailed)));
    assert!(!delivered);
    assert!(matches!(
        client.download_all_attachments("message", true).await,
        Err(Error::AttachmentVerificationFailed)
    ));
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let folder = std::env::temp_dir().join(format!(
        "ruston-attachment-verification-{}-{unique}",
        std::process::id()
    ));
    let result = client.export_folder("inbox", &folder, 1).await;
    let remaining = std::fs::read_dir(&folder).unwrap().count();
    std::fs::remove_dir_all(&folder).unwrap();
    assert!(matches!(result, Err(Error::AttachmentVerificationFailed)));
    assert_eq!(
        remaining, 0,
        "failed verification must remove the partial EML"
    );
}

/// A new directory name under the system temp directory.
fn export_directory(label: &str) -> std::path::PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("ruston-{label}-{}-{unique}", std::process::id()))
}

/// Serves `messages`, in this order, as every folder's listing, and each
/// one in full.
async fn serve_folder(server: &MockServer, messages: &[Message]) {
    for message in messages {
        Mock::given(method("GET"))
            .and(path(format!("/mail/v4/messages/{}", message.meta.id)))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"Code": 1000, "Message": message})),
            )
            .mount(server)
            .await;
    }
    let listed: Vec<_> = messages.iter().map(|message| &message.meta).collect();
    Mock::given(method("GET"))
        .and(path("/mail/v4/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "Code": 1000, "Total": messages.len(), "Messages": listed
        })))
        .mount(server)
        .await;
}

#[tokio::test]
async fn messages_after_one_that_cannot_be_exported_are_still_exported() {
    let server = MockServer::start().await;
    let (client, attachment) = attachment_client(&server).await;
    let provider = crypto::provider();
    let address = client.keys().primary_address().unwrap();
    let body = crypto::encrypt_self_draft(&provider, address, "body").unwrap();
    let message = |id: &str, attachment: Attachment, body: &str| {
        let mut message = message_with_attachment(attachment);
        message.meta.id = id.into();
        message.body = body.into();
        message.mime_type = "text/plain".into();
        message
    };
    // This attachment's signature was made over other plaintext.
    let other = crypto::encrypt_attachment(&provider, address, b"different payload").unwrap();
    let mut forged = attachment.clone();
    forged.signature = Some(
        String::from_utf8(
            provider
                .armorer()
                .armor_signature(&other.signature)
                .unwrap(),
        )
        .unwrap(),
    );
    serve_folder(
        &server,
        &[
            message("forged", forged, &body),
            message("garbled", attachment.clone(), "not an encrypted body"),
            message("good", attachment, &body),
        ],
    )
    .await;
    let folder = export_directory("export-skip");

    let report = client
        .export_folder_report("inbox", &folder, 10)
        .await
        .unwrap();

    let files: Vec<_> = std::fs::read_dir(&folder)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    std::fs::remove_dir_all(&folder).unwrap();
    assert_eq!((report.written, report.already_exported), (1, 0));
    let skipped: Vec<_> = report
        .skipped
        .iter()
        .map(|skipped| skipped.meta.id.as_str())
        .collect();
    assert_eq!(skipped, ["forged", "garbled"]);
    assert!(matches!(
        report.skipped[0].error,
        Error::AttachmentVerificationFailed
    ));
    assert!(matches!(report.skipped[1].error, Error::Crypto(_)));
    // Only the finished file is left, under its final name ("good" in hex).
    assert_eq!(files, ["message-676f6f64.eml"]);
}

#[tokio::test]
async fn an_export_stops_at_its_maximum_inside_a_page() {
    let server = MockServer::start().await;
    let (client, attachment) = attachment_client(&server).await;
    let address = client.keys().primary_address().unwrap();
    let body = crypto::encrypt_self_draft(&crypto::provider(), address, "body").unwrap();
    let message = |id: &str| {
        let mut message = message_with_attachment(attachment.clone());
        message.meta.id = id.into();
        message.body = body.clone();
        message.mime_type = "text/plain".into();
        message
    };
    serve_folder(&server, &[message("first"), message("second")]).await;
    let folder = export_directory("export-maximum");

    let report = client.export_folder_report("inbox", &folder, 1).await;
    std::fs::remove_dir_all(&folder).unwrap();

    let report = report.unwrap();
    assert_eq!((report.written, report.already_exported), (1, 0));
    let fetched: Vec<String> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|request| request.url.path().to_owned())
        .filter(|path| path.starts_with("/mail/v4/messages/"))
        .collect();
    assert_eq!(fetched, ["/mail/v4/messages/first"]);
}

#[tokio::test]
async fn a_rerun_resumes_without_fetching_exported_messages_again() {
    let server = MockServer::start().await;
    let (client, attachment) = attachment_client(&server).await;
    let address = client.keys().primary_address().unwrap();
    let mut message = message_with_attachment(attachment);
    message.body = crypto::encrypt_self_draft(&crypto::provider(), address, "body").unwrap();
    message.mime_type = "text/plain".into();
    serve_folder(&server, &[message]).await;
    let folder = export_directory("export-resume");

    let first = client.export_folder("inbox", &folder, 10).await;
    let second = client.export_folder("inbox", &folder, 10).await;
    let report = client.export_folder_report("inbox", &folder, 10).await;
    // "message" in hex.
    let exported = folder.join("message-6d657373616765.eml");
    let written = exported.is_file();
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(&exported).unwrap().permissions().mode() & 0o777
    };
    std::fs::remove_dir_all(&folder).unwrap();

    assert!(written);
    assert_eq!(first.unwrap(), 1);
    assert_eq!(second.unwrap(), 0);
    let report = report.unwrap();
    assert_eq!((report.written, report.already_exported), (0, 1));
    assert!(report.skipped.is_empty());
    let fetches = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|request| request.url.path() == "/mail/v4/messages/message")
        .count();
    assert_eq!(fetches, 1, "only the first export fetches the message");
    #[cfg(unix)]
    assert_eq!(mode, 0o600);
}

#[test]
fn untrusted_paths_and_invalid_characters_get_plain_names() {
    for (input, expected) in [
        ("../../etc/passwd", "passwd"),
        (r"..\windows\file.txt", "file.txt"),
        ("/tmp/report.pdf", "report.pdf"),
        ("C:report.pdf", "C_report.pdf"),
        (r"C:\Windows\evil.exe", "evil.exe"),
        (r"\\server\share\file", "file"),
        ("bad\nname.txt", "bad_name.txt"),
        ("bad\0name.txt", "bad_name.txt"),
        ("bad?name.txt", "bad_name.txt"),
        ("Reunión 10:30.ics", "Reunión 10_30.ics"),
        ("notes.txt:hidden", "notes.txt_hidden"),
        (r#"<"a"|b*>.txt"#, "__a__b__.txt"),
        ("...", "attachment"),
        ("   ", "attachment"),
        ("", "attachment"),
        ("Q3 report.pdf", "Q3 report.pdf"),
    ] {
        assert_eq!(safe_attachment_name(input), expected);
    }
}

#[test]
fn names_end_where_windows_would_end_them() {
    // Windows drops trailing dots and spaces when it creates a file.
    for (input, expected) in [
        ("report.pdf .", "report.pdf"),
        ("report.pdf. . ", "report.pdf"),
        (" . report.pdf", "report.pdf"),
    ] {
        assert_eq!(safe_attachment_name(input), expected, "{input:?}");
    }
}

#[test]
fn long_names_are_cut_keeping_their_extension() {
    let long = format!("{}.pdf", "a".repeat(300));
    let name = safe_attachment_name(&long);
    assert_eq!(name.len(), MAX_NAME_BYTES);
    assert!(name.ends_with("aaa.pdf"));

    // A cut never splits a character.
    let accented = format!("{}.docx", "ñ".repeat(200));
    let name = safe_attachment_name(&accented);
    assert!(name.len() <= MAX_NAME_BYTES && name.ends_with("ñ.docx"));

    // Without a short extension, the end of the name goes.
    let name = safe_attachment_name(&format!("{}.{}", "b".repeat(10), "c".repeat(300)));
    assert_eq!(name.len(), MAX_NAME_BYTES);
    assert!(name.starts_with("bbbbbbbbbb.ccc"));

    // A cut that lands on spaces does not leave them at the end.
    let spaced = format!("{}{}", "d".repeat(239), " ".repeat(20));
    assert_eq!(safe_attachment_name(&spaced), "d".repeat(239));
}

#[test]
fn invisible_direction_marks_cannot_disguise_an_extension() {
    for (input, expected) in [
        // Shown as "Invoice_rcs.pdf" by Finder and Explorer.
        ("Invoice_\u{202E}fdp.scr", "Invoice_fdp.scr"),
        ("report\u{2066}.pdf\u{2069}.exe", "report.pdf.exe"),
        ("\u{200F}\u{200E}\u{061C}photo.jpg", "photo.jpg"),
        ("\u{202A}\u{202B}\u{202C}\u{202D}notes.txt", "notes.txt"),
        ("\u{2067}\u{2068}minutes.txt", "minutes.txt"),
        ("a\u{200B}b\u{2060}c\u{FEFF}.txt", "abc.txt"),
        ("\u{202E}\u{200B}", "attachment"),
        // Joiners are part of correctly written words and emoji.
        ("می\u{200C}خواهم.txt", "می\u{200C}خواهم.txt"),
        ("👩\u{200D}💻.png", "👩\u{200D}💻.png"),
    ] {
        assert_eq!(safe_attachment_name(input), expected, "{input:?}");
    }
}

#[test]
fn windows_device_names_are_prefixed_on_every_platform() {
    for (input, expected) in [
        ("CON.txt", "_CON.txt"),
        ("con .txt", "_con .txt"),
        ("PRN", "_PRN"),
        ("AUX.h", "_AUX.h"),
        ("NUL.zip", "_NUL.zip"),
        ("COM1.txt", "_COM1.txt"),
        ("com9.bin", "_com9.bin"),
        ("LPT1.doc", "_LPT1.doc"),
        ("lpt9.pdf", "_lpt9.pdf"),
        ("CONIN$.log", "_CONIN$.log"),
        ("CONOUT$", "_CONOUT$"),
        ("CLOCK$", "_CLOCK$"),
        ("aux.tar.gz", "_aux.tar.gz"),
        ("nul", "_nul"),
        ("con.", "_con"),
        (r"C:\temp\CON.pdf", "_CON.pdf"),
        ("../../aux.png", "_aux.png"),
        ("COM¹.txt", "_COM¹.txt"),
        ("com².log", "_com².log"),
        ("LPT³", "_LPT³"),
        ("COM10.txt", "COM10.txt"),
        ("COM.txt", "COM.txt"),
        ("COM⁴.txt", "COM⁴.txt"),
        ("contact.pdf", "contact.pdf"),
        ("conversation.txt", "conversation.txt"),
        ("auxiliary.c", "auxiliary.c"),
        ("null.json", "null.json"),
        ("printer.png", "printer.png"),
    ] {
        assert_eq!(safe_attachment_name(input), expected);
    }
}

#[test]
#[cfg(windows)]
fn a_marked_download_carries_the_internet_zone() {
    let path = std::env::temp_dir().join(format!(
        "ruston-zone-{} informe ñ, 100%.pdf",
        std::process::id()
    ));
    std::fs::write(&path, b"%PDF").unwrap();
    mark_downloaded(&path).unwrap();
    let mut stream = path.as_os_str().to_owned();
    stream.push(":Zone.Identifier");
    assert_eq!(
        std::fs::read_to_string(stream).unwrap(),
        "[ZoneTransfer]\r\nZoneId=3\r\n"
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"%PDF");
    std::fs::remove_file(path).unwrap();
}

/// An empty directory of its own for one test.
fn save_folder(label: &str) -> PathBuf {
    let folder = export_directory(label);
    std::fs::create_dir_all(&folder).unwrap();
    folder
}

fn file_name(path: &Path) -> &str {
    path.file_name().unwrap().to_str().unwrap()
}

#[test]
fn a_saved_attachment_takes_the_first_free_name() {
    let folder = save_folder("free-name");
    let saved: Vec<_> = [b"one", b"two", b"red"]
        .iter()
        .map(|contents| save_under_free_name(&folder, "report.pdf", *contents).unwrap())
        .collect();
    let names: Vec<_> = saved.iter().map(|path| file_name(path)).collect();
    assert_eq!(names, ["report.pdf", "report (1).pdf", "report (2).pdf"]);
    // An earlier file keeps what it held.
    assert_eq!(std::fs::read(&saved[0]).unwrap(), b"one");

    // A name with no extension, or a device name made safe, is numbered
    // the same way.
    for (name, first, copy) in [
        ("notes", "notes", "notes (1)"),
        ("CON.pdf", "_CON.pdf", "_CON (1).pdf"),
    ] {
        let saved_first = save_under_free_name(&folder, name, b"x").unwrap();
        let saved_copy = save_under_free_name(&folder, name, b"y").unwrap();
        assert_eq!(file_name(&saved_first), first);
        assert_eq!(file_name(&saved_copy), copy);
    }
    std::fs::remove_dir_all(folder).unwrap();
}

#[test]
fn a_file_that_could_not_be_written_in_full_is_removed() {
    let folder = save_folder("half-written");
    let path = folder.join("report.pdf");

    let written = write_new_file_with(&path, |file| {
        file.write_all(b"the first half")?;
        Err(std::io::Error::other("the disk filled up"))
    });

    assert!(written.is_err());
    assert!(!path.exists(), "half a file stayed behind");
    std::fs::remove_dir_all(folder).unwrap();
}

#[test]
fn untrusted_attachment_names_stay_inside_the_folder() {
    let folder = save_folder("untrusted-names");
    for (untrusted, expected) in [
        ("../../outside.txt", "outside.txt"),
        ("/tmp/absolute.txt", "absolute.txt"),
        (r"..\windows\file.txt", "file.txt"),
        ("C:drive.txt", "C_drive.txt"),
        ("CON .txt", "_CON .txt"),
        ("...", "attachment"),
    ] {
        let path = save_under_free_name(&folder, untrusted, b"mail").unwrap();
        assert_eq!(path.parent(), Some(folder.as_path()));
        assert_eq!(file_name(&path), expected);
        assert_eq!(std::fs::read(path).unwrap(), b"mail");
    }
    std::fs::remove_dir_all(folder).unwrap();
}

#[test]
fn a_long_name_and_its_numbered_copy_can_both_be_saved() {
    let folder = save_folder("long-name");
    let long = format!("{}.pdf", "informe ".repeat(40));
    let first = save_under_free_name(&folder, &long, b"one").unwrap();
    let second = save_under_free_name(&folder, &long, b"two").unwrap();
    assert_ne!(first, second);
    for path in [&first, &second] {
        let name = file_name(path);
        assert!(name.len() <= 255 && name.ends_with(".pdf"), "{name}");
    }
    assert_eq!(std::fs::read(second).unwrap(), b"two");
    std::fs::remove_dir_all(folder).unwrap();
}

#[test]
fn the_last_copy_of_a_long_name_still_fits_a_file_system() {
    let name = safe_attachment_name(&format!("{}.pdf", "informe ".repeat(40)));
    let last = free_names(&name).take(FREE_NAME_ATTEMPTS).last().unwrap();
    assert!(last.ends_with(" (9999).pdf"), "{last}");
    assert!(last.len() <= 255, "{} bytes", last.len());
}

#[test]
fn concurrent_saves_claim_different_names_without_overwriting() {
    let folder = save_folder("concurrent-saves");
    std::fs::write(folder.join("report.txt"), b"existing").unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
    let saves: Vec<_> = (0_u8..8)
        .map(|contents| {
            let (folder, barrier) = (folder.clone(), barrier.clone());
            std::thread::spawn(move || {
                barrier.wait();
                save_under_free_name(&folder, "report.txt", &[contents]).unwrap()
            })
        })
        .collect();
    let mut paths = std::collections::HashSet::new();
    let mut contents = std::collections::HashSet::new();
    for save in saves {
        let path = save.join().unwrap();
        contents.insert(std::fs::read(&path).unwrap()[0]);
        assert!(paths.insert(path));
    }
    assert_eq!(contents.len(), 8);
    assert_eq!(
        std::fs::read(folder.join("report.txt")).unwrap(),
        b"existing"
    );
    std::fs::remove_dir_all(folder).unwrap();
}

#[test]
fn a_missing_folder_is_a_failure_not_a_taken_name() {
    let folder = save_folder("missing-folder");
    let missing = folder.join("missing");
    let error = save_under_free_name(&missing, "report.pdf", b"x").unwrap_err();
    let created = missing.exists();
    std::fs::remove_dir_all(folder).unwrap();
    assert_ne!(error.kind(), ErrorKind::AlreadyExists);
    assert!(!created);
}

#[cfg(unix)]
#[test]
fn a_dangling_symlink_is_never_followed() {
    let folder = save_folder("dangling-symlink");
    let outside = folder.with_extension("outside");
    let _ = std::fs::remove_file(&outside);
    std::os::unix::fs::symlink(&outside, folder.join("report.txt")).unwrap();
    let path = save_under_free_name(&folder, "report.txt", b"mail").unwrap();
    let escaped = outside.exists();
    std::fs::remove_dir_all(&folder).unwrap();
    let _ = std::fs::remove_file(&outside);
    assert!(!escaped, "the write followed a dangling symlink");
    assert_eq!(file_name(&path), "report (1).txt");
}

#[test]
#[cfg(windows)]
fn a_saved_attachment_is_marked_as_downloaded_from_the_internet() {
    let folder = save_folder("saved-zone");
    let mut stream = save_under_free_name(&folder, "factura.pdf", b"%PDF")
        .unwrap()
        .into_os_string();
    stream.push(":Zone.Identifier");
    assert!(
        std::fs::read_to_string(stream)
            .unwrap()
            .contains("ZoneId=3")
    );
    std::fs::remove_dir_all(folder).unwrap();
}
