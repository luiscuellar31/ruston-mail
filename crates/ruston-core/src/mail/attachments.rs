//! Attachment listing + download (decrypt).

use super::Client;
use crate::api;
use crate::crypto;
use crate::error::{Error, Result};
use crate::model::message::{Attachment, Message};
use std::path::{Component, Path};
use zeroize::Zeroizing;

/// Maximum combined plaintext returned by `download_all_attachments` (128 MiB).
pub const MAX_BULK_ATTACHMENT_BYTES: usize = 128 * 1024 * 1024;

/// The largest attachment that can be opened (50 MiB): proton-rpgp's default
/// reading limit, which proton-crypto applies to every decrypted plaintext.
pub const MAX_ATTACHMENT_PLAINTEXT_BYTES: usize = 50 * 1024 * 1024;

/// Return a portable plain filename for an untrusted attachment name.
/// Invisible direction marks are dropped; path components are removed;
/// invalid characters and empty names become `attachment`, and Windows device
/// names are prefixed with an underscore.
pub fn safe_attachment_name(name: &str) -> String {
    let name: String = name.chars().filter(|&c| !is_invisible_mark(c)).collect();
    let name = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .trim()
        .trim_start_matches('.')
        .trim_end_matches('.');
    let mut components = Path::new(name).components();
    let plain_name =
        matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none();

    if name.is_empty()
        || name
            .chars()
            .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
        || !plain_name
    {
        return "attachment".to_owned();
    }

    if is_windows_reserved(name) {
        format!("_{name}")
    } else {
        name.to_owned()
    }
}

/// Characters a file manager, terminal or notification draws as nothing.
/// Direction overrides and isolates can make `fdp.scr` read as `rcs.pdf`, or
/// reverse the address after a sender's name; the zero-width space, word
/// joiner and byte-order mark only hide parts of the text. Joiners (U+200C,
/// U+200D) stay: words in several scripts and emoji need them, and they
/// cannot reorder or hide anything.
pub fn is_invisible_mark(c: char) -> bool {
    matches!(
        c,
        '\u{061C}'
            | '\u{200B}'
            | '\u{200E}'
            | '\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FEFF}'
    )
}

fn is_windows_reserved(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).trim_end();
    if ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$", "CLOCK$"]
        .iter()
        .any(|reserved| stem.eq_ignore_ascii_case(reserved))
    {
        return true;
    }
    let bytes = stem.as_bytes();
    bytes.len() == 4
        && (bytes[..3].eq_ignore_ascii_case(b"COM") || bytes[..3].eq_ignore_ascii_case(b"LPT"))
        && bytes[3].is_ascii_digit()
}

impl Client {
    /// List a message's attachments (inline filtered unless `include_inline`).
    pub async fn list_attachments(
        &self,
        message_id: &str,
        include_inline: bool,
    ) -> Result<Vec<Attachment>> {
        let msg = api::messages::get_message(self.http(), message_id).await?;
        Ok(msg
            .attachments
            .into_iter()
            .filter(|a| include_inline || !a.is_inline())
            .collect())
    }

    /// Download + decrypt one attachment. Returns (filename, bytes).
    /// A nonempty detached signature must verify before plaintext is returned;
    /// unavailable sender keys or an invalid signature fail the download.
    pub async fn download_attachment(
        &self,
        message_id: &str,
        attachment_id: &str,
    ) -> Result<(String, Vec<u8>)> {
        let msg = api::messages::get_message(self.http(), message_id).await?;
        let att = msg
            .attachments
            .iter()
            .find(|a| a.id == attachment_id)
            .ok_or_else(|| Error::NotFound {
                kind: "attachment".into(),
            })?;
        self.download_attachment_from_message(&msg, att).await
    }

    pub(super) async fn download_attachment_from_message(
        &self,
        msg: &Message,
        att: &Attachment,
    ) -> Result<(String, Vec<u8>)> {
        let too_large = || Error::AttachmentTooLarge {
            limit: MAX_ATTACHMENT_PLAINTEXT_BYTES,
        };
        // Decryption would refuse it, so it is not worth downloading.
        if att.size > api::attachments::MAX_ATTACHMENT_RESPONSE_BYTES as u64 {
            return Err(too_large());
        }
        let key_packets = att
            .key_packets
            .as_deref()
            .ok_or_else(|| Error::Crypto("attachment has no key packets".into()))?;
        let addr = self
            .keys()
            .address(&msg.meta.address_id)
            .or_else(|| self.keys().primary_address())
            .ok_or_else(|| Error::Crypto("no address key for attachment".into()))?;

        // Resolve verifier keys before allocating and decrypting attachment data.
        // An absent or empty Signature denotes an unsigned attachment in Proton.
        let signature = att.signature.as_deref().filter(|sig| !sig.is_empty());
        let sender_pubs = if signature.is_some() {
            // A failed lookup is reported as itself, so it can be retried.
            let keys = self.sender_pubkeys(&msg.meta.sender.address).await?;
            if keys.is_empty() {
                return Err(Error::AttachmentVerificationFailed);
            }
            keys
        } else {
            Vec::new()
        };
        let data_packet = match api::attachments::get_attachment(self.http(), &att.id).await {
            Err(Error::ResponseTooLarge { .. }) => return Err(too_large()),
            result => result?,
        };
        // Decrypting and verifying up to 50 MiB is CPU work for the blocking pool.
        let (addr, key_packets) = (addr.clone(), key_packets.to_owned());
        let signature = signature.map(str::to_owned);
        let plain = super::off_thread(move || {
            let provider = crypto::provider();
            let mut plain = Zeroizing::new(crypto::decrypt_attachment(
                &provider,
                &addr,
                &key_packets,
                &data_packet,
            )?);
            drop(data_packet);
            // Never substitute the recipient's keys for the sender's verifier.
            if let Some(signature) = signature {
                crypto::verify_attachment_signature(&provider, &sender_pubs, &plain, &signature)?;
            }
            Ok(std::mem::take(&mut *plain))
        })
        .await?;
        Ok((att.name.clone(), plain))
    }

    /// Download attachments sequentially, handing each plaintext to `receive`.
    /// The callback completes before the next attachment is fetched.
    pub async fn for_each_attachment<F>(
        &self,
        message_id: &str,
        include_inline: bool,
        mut receive: F,
    ) -> Result<()>
    where
        F: FnMut(String, Vec<u8>) -> Result<()>,
    {
        let msg = api::messages::get_message(self.http(), message_id).await?;
        for att in &msg.attachments {
            if include_inline || !att.is_inline() {
                let (name, bytes) = self.download_attachment_from_message(&msg, att).await?;
                receive(name, bytes)?;
            }
        }
        Ok(())
    }

    /// Download all (non-inline) attachments of a message, up to 128 MiB total.
    /// Use `for_each_attachment` to process larger collections one at a time.
    pub async fn download_all_attachments(
        &self,
        message_id: &str,
        include_inline: bool,
    ) -> Result<Vec<(String, Vec<u8>)>> {
        let mut out = Vec::new();
        let mut total = 0usize;
        self.for_each_attachment(message_id, include_inline, |name, bytes| {
            if bytes.len() > MAX_BULK_ATTACHMENT_BYTES.saturating_sub(total) {
                return Err(Error::AttachmentBatchTooLarge {
                    limit: MAX_BULK_ATTACHMENT_BYTES,
                });
            }
            total += bytes.len();
            out.push((name, bytes));
            Ok(())
        })
        .await?;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::CacheIdentity;
    use crate::crypto::{AddressKeys, KeyStore, StoredKey};
    use crate::model::enums::key_flag;
    use crate::session::{MemoryStore, Paths};
    use crate::transport::HttpClient;
    use base64::{Engine, engine::general_purpose::STANDARD};
    use proton_crypto::crypto::{
        ArmorerSync, DataEncoding, KeyGenerator, KeyGeneratorAlgorithm, KeyGeneratorSync,
        PGPProviderSync,
    };
    use secrecy::SecretString;
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn attachment_client(server: &MockServer) -> (Client, Attachment) {
        attachment_client_with_key_flags(server, key_flag::NOT_COMPROMISED | key_flag::NOT_OBSOLETE)
            .await
    }

    /// A client for sender@proton.me, whose one key Proton reports with
    /// `flags`, and a signed attachment from that sender.
    async fn attachment_client_with_key_flags(
        server: &MockServer,
        flags: u32,
    ) -> (Client, Attachment) {
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
        let upload =
            crypto::encrypt_attachment(&provider, &address, b"attachment payload").unwrap();
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
        let client = Client {
            http: HttpClient::new(server.uri(), "Other"),
            keys: KeyStore {
                user_keys: Vec::new(),
                addresses: vec![address],
            },
            paths: Paths::with_base(std::env::temp_dir()),
            profile: "test".into(),
            session_uid: "test".into(),
            cache_identity: CacheIdentity::new(&server.uri(), "test").unwrap(),
            store: Arc::new(MemoryStore::default()),
            sender_cache: Mutex::new(super::super::SenderKeyCache::default()),
        };
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
            .download_attachment_from_message(&message, &attachment)
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
                .download_attachment_from_message(&message, &attachment)
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
                .download_attachment_from_message(&message, &attachment)
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
                .download_attachment_from_message(&message, &attachment)
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
                .download_attachment_from_message(&message, &message.attachments[0])
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
                .download_attachment_from_message(&message, &message.attachments[0])
                .await,
            Err(Error::AttachmentVerificationFailed)
        ));

        // Nor does it vouch for a body the key signed.
        let provider = crypto::provider();
        let address = client.keys().primary_address().unwrap();
        message.body = crypto::encrypt_self_draft(&provider, address, "body").unwrap();
        message.mime_type = "text/plain".into();
        let read = client.decrypt_message(&message).await.unwrap();
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
                .download_attachment_from_message(&message, &message.attachments[0])
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
        let read = client.decrypt_message(&message).await.unwrap();
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

    #[test]
    fn untrusted_paths_and_invalid_characters_get_plain_names() {
        for (input, expected) in [
            ("../../etc/passwd", "passwd"),
            (r"..\windows\file.txt", "file.txt"),
            ("/tmp/report.pdf", "report.pdf"),
            ("C:report.pdf", "attachment"),
            ("bad\nname.txt", "attachment"),
            ("bad\0name.txt", "attachment"),
            ("bad?name.txt", "attachment"),
            ("...", "attachment"),
            ("", "attachment"),
            ("Q3 report.pdf", "Q3 report.pdf"),
        ] {
            assert_eq!(safe_attachment_name(input), expected);
        }
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
            ("COM10.txt", "COM10.txt"),
        ] {
            assert_eq!(safe_attachment_name(input), expected);
        }
    }
}
