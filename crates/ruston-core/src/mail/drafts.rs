//! Draft lifecycle: save / update / list / delete (without sending).

use super::Client;
use super::send::{SendOptions, recipient_list, upload_attachments, validate_attachments};
use crate::api;
use crate::crypto::{self, AddressKeys};
use crate::error::{Error, Result};
use crate::model::enums::label_ids;
use crate::model::message::MessageMetadata;
use serde_json::json;

/// A draft written from `addr`, its body encrypted to that address's key:
/// what saving a draft writes, and what sending one starts from.
pub(super) fn draft_message_json(
    addr: &AddressKeys,
    opts: &SendOptions,
) -> Result<serde_json::Value> {
    let provider = crypto::provider();
    let armored = crypto::encrypt_self_draft(&provider, addr, &opts.body)?;
    Ok(json!({
        "Message": {
            "ToList": recipient_list(&opts.to),
            "CCList": recipient_list(&opts.cc),
            "BCCList": recipient_list(&opts.bcc),
            "Subject": opts.subject,
            "Sender": { "Address": addr.email, "Name": "" },
            "Body": armored,
            "MIMEType": opts.body_mime_type(),
        }
    }))
}

/// A delivery time and an expiration belong to sending, which saving a draft
/// is not: they are refused before anything is written, not dropped.
fn refuse_send_options(opts: &SendOptions) -> Result<()> {
    if opts.send_at.is_some() || opts.expires_in.is_some() {
        return Err(Error::Other(
            "a draft keeps no delivery time or expiration; they are set when it is sent".into(),
        ));
    }
    Ok(())
}

fn sender_not_found() -> Error {
    Error::NotFound {
        kind: "sender address".into(),
    }
}

impl Client {
    /// The keys of the address a new message is written from: `from`, or
    /// the account's primary address.
    pub(super) fn sender_keys(&self, from: Option<&str>) -> Result<&AddressKeys> {
        match from {
            Some(email) => self.keys().address_for_email(email),
            None => self.keys().primary_address(),
        }
        .ok_or_else(sender_not_found)
    }

    /// Create a draft (encrypted to self, stored in Drafts) with its
    /// attachments, without sending it. A draft whose attachments fail to
    /// upload is deleted again, so trying again leaves no part-made copy.
    pub async fn save_draft(&self, opts: &SendOptions) -> Result<String> {
        tracing::info!(target: "ruston_core::mail", to = opts.to.len(), attachments = opts.attachments.len(), "save_draft");
        refuse_send_options(opts)?;
        validate_attachments(&opts.attachments).await?;
        let addr = self.sender_keys(opts.from.as_deref())?;
        let body = draft_message_json(addr, opts)?;
        let created = api::messages::create_draft(self.http(), body).await?;
        let id = created.meta.id;
        if let Err(error) = upload_attachments(self.http(), addr, &id, &opts.attachments).await {
            let _ = api::messages::delete(self.http(), std::slice::from_ref(&id)).await;
            return Err(error);
        }
        Ok(id)
    }

    /// Replace an existing draft's content, adding `opts.attachments` to the
    /// attachments it has. It stays written from its own address unless
    /// `opts.from` names another. Any other message is refused. When an
    /// upload fails, the new content and the files before it stay.
    pub async fn update_draft(&self, id: &str, opts: &SendOptions) -> Result<()> {
        tracing::info!(target: "ruston_core::mail", draft_id = %id, attachments = opts.attachments.len(), "update_draft");
        refuse_send_options(opts)?;
        validate_attachments(&opts.attachments).await?;
        let draft = self.draft_metadata(id).await?;
        let addr = match &opts.from {
            Some(email) => self.keys().address_for_email(email),
            None => self.keys().address(&draft.address_id),
        }
        .ok_or_else(sender_not_found)?;
        // Its attachments are keyed to the address it was written from, and
        // another one could not open them.
        if addr.address_id != draft.address_id && draft.num_attachments > 0 {
            return Err(Error::Other(format!(
                "{id} has attachments, so it stays written from its own address; nothing was changed"
            )));
        }
        let body = draft_message_json(addr, opts)?;
        api::messages::update_draft(self.http(), id, body).await?;
        upload_attachments(self.http(), addr, id, &opts.attachments).await?;
        Ok(())
    }

    /// List drafts (the Drafts folder).
    pub async fn list_drafts(
        &self,
        page: u32,
        page_size: u32,
    ) -> Result<(u32, Vec<MessageMetadata>)> {
        self.list_messages("drafts", page, page_size, false).await
    }

    /// A draft's metadata. Any other message is refused, so a mistyped or
    /// copied ID cannot reach received or sent mail.
    pub async fn draft_metadata(&self, id: &str) -> Result<MessageMetadata> {
        let meta = self.message_metadata(id).await?;
        let draft = meta
            .label_ids
            .iter()
            .any(|label| label == label_ids::ALL_DRAFTS || label == label_ids::DRAFTS);
        if draft {
            Ok(meta)
        } else {
            Err(Error::Other(format!(
                "{id} is not a draft; nothing was changed"
            )))
        }
    }

    /// Delete drafts permanently. Every ID is checked first: if any of them
    /// is not a draft, nothing is deleted.
    pub async fn delete_draft(&self, ids: &[String]) -> Result<()> {
        for id in ids {
            self.draft_metadata(id).await?;
        }
        api::messages::delete(self.http(), ids).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::CacheIdentity;
    use crate::crypto::{KeyStore, StoredKey};
    use crate::model::message::Message;
    use crate::session::{MemoryStore, Paths};
    use crate::transport::HttpClient;
    use proton_crypto::crypto::{
        DataEncoding, KeyGenerator, KeyGeneratorAlgorithm, KeyGeneratorSync, PGPProviderSync,
    };
    use secrecy::SecretString;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const DRAFT_LABELS: &[&str] = &[label_ids::ALL_DRAFTS, label_ids::DRAFTS];

    fn metadata(id: &str, labels: &[&str]) -> MessageMetadata {
        MessageMetadata {
            id: id.into(),
            label_ids: labels.iter().map(|label| label.to_string()).collect(),
            ..Default::default()
        }
    }

    async fn mount_message(server: &MockServer, meta: MessageMetadata) {
        let id = meta.id.clone();
        let message = Message {
            meta,
            ..Default::default()
        };
        Mock::given(method("GET"))
            .and(path(format!("/mail/v4/messages/{id}")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"Code": 1000, "Message": message})),
            )
            .mount(server)
            .await;
    }

    /// One of the account's addresses, with a fresh key.
    fn address(email: &str, address_id: &str) -> AddressKeys {
        let provider = crypto::provider();
        let key = provider
            .new_key_generator()
            .with_user_id("Test", email)
            .with_algorithm(KeyGeneratorAlgorithm::ECC)
            .generate()
            .unwrap();
        let armored = provider
            .private_key_export(&key, "pass", DataEncoding::Armor)
            .unwrap();
        AddressKeys {
            address_id: address_id.into(),
            email: email.into(),
            keys: vec![StoredKey {
                armored: String::from_utf8(armored.as_ref().to_vec()).unwrap(),
                passphrase: SecretString::from("pass"),
                unlocked: None,
            }],
        }
    }

    fn client(server: &MockServer, addresses: Vec<AddressKeys>) -> Client {
        Client {
            http: HttpClient::new(server.uri(), "Other"),
            keys: KeyStore {
                user_keys: Vec::new(),
                addresses,
            },
            paths: Paths::with_base(std::env::temp_dir()),
            profile: "test".into(),
            session_uid: "test".into(),
            cache_identity: CacheIdentity::new(&server.uri(), "test").unwrap(),
            store: Arc::new(MemoryStore::default()),
            sender_cache: Mutex::new(super::super::SenderKeyCache::default()),
        }
    }

    /// A file to attach, named after the test that writes it.
    fn attachment(test: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("ruston-draft-{test}-{}.txt", std::process::id()));
        std::fs::write(&path, b"minutes").unwrap();
        path
    }

    fn requests_to<'a>(requests: &'a [wiremock::Request], at: &str) -> Vec<&'a wiremock::Request> {
        requests
            .iter()
            .filter(|request| request.url.path() == at)
            .collect()
    }

    async fn deletions(server: &MockServer) -> usize {
        requests_to(
            &server.received_requests().await.unwrap(),
            "/mail/v4/messages/delete",
        )
        .len()
    }

    async fn mount_created_draft(server: &MockServer) {
        Mock::given(method("POST"))
            .and(path("/mail/v4/messages"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"Code": 1000, "Message": {"ID": "draft-1"}})),
            )
            .expect(1)
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn a_saved_draft_carries_its_attachments() {
        let server = MockServer::start().await;
        mount_created_draft(&server).await;
        Mock::given(method("POST"))
            .and(path("/mail/v4/attachments"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"Code": 1000, "Attachment": {"ID": "attachment-1"}}),
            ))
            .expect(1)
            .mount(&server)
            .await;
        let file = attachment("saved");
        let opts = SendOptions {
            to: vec!["friend@proton.me".into()],
            body: "The minutes are attached.".into(),
            attachments: vec![file.clone()],
            ..SendOptions::default()
        };

        let client = client(&server, vec![address("me@proton.me", "address-1")]);
        assert_eq!(client.save_draft(&opts).await.unwrap(), "draft-1");

        let requests = server.received_requests().await.unwrap();
        let uploads = requests_to(&requests, "/mail/v4/attachments");
        let form = String::from_utf8_lossy(&uploads[0].body);
        assert!(form.contains("name=\"MessageID\"\r\n\r\ndraft-1\r\n"));
        let name = file.file_name().unwrap().to_str().unwrap();
        assert!(form.contains(&format!("name=\"Filename\"\r\n\r\n{name}\r\n")));
        server.verify().await;
        std::fs::remove_file(file).unwrap();
    }

    #[tokio::test]
    async fn a_draft_whose_attachment_is_refused_is_deleted_again() {
        let server = MockServer::start().await;
        mount_created_draft(&server).await;
        Mock::given(method("POST"))
            .and(path("/mail/v4/attachments"))
            .respond_with(
                ResponseTemplate::new(422).set_body_json(
                    serde_json::json!({"Code": 2011, "Error": "Attachment refused"}),
                ),
            )
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/mail/v4/messages/delete"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"Code": 1000})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let file = attachment("refused");
        let opts = SendOptions {
            to: vec!["friend@proton.me".into()],
            attachments: vec![file.clone()],
            ..SendOptions::default()
        };

        let client = client(&server, vec![address("me@proton.me", "address-1")]);
        assert!(matches!(
            client.save_draft(&opts).await,
            Err(Error::Api(refused)) if refused.code == 2011
        ));

        let requests = server.received_requests().await.unwrap();
        let deleted = requests_to(&requests, "/mail/v4/messages/delete");
        let body: serde_json::Value = serde_json::from_slice(&deleted[0].body).unwrap();
        assert_eq!(body["IDs"], serde_json::json!(["draft-1"]));
        server.verify().await;
        std::fs::remove_file(file).unwrap();
    }

    #[tokio::test]
    async fn a_draft_refuses_a_delivery_time_or_an_expiration() {
        let server = MockServer::start().await;
        let client = client(&server, vec![address("me@proton.me", "address-1")]);
        let scheduled = SendOptions {
            send_at: Some(1_900_000_000),
            ..SendOptions::default()
        };
        let expiring = SendOptions {
            expires_in: Some(3_600),
            ..SendOptions::default()
        };

        for opts in [scheduled, expiring] {
            assert!(client.save_draft(&opts).await.is_err());
            assert!(client.update_draft("draft-1", &opts).await.is_err());
        }
        // Refused before anything reached Proton.
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn editing_a_draft_keeps_the_address_it_was_written_from() {
        let server = MockServer::start().await;
        mount_message(
            &server,
            MessageMetadata {
                address_id: "address-2".into(),
                num_attachments: 1,
                ..metadata("draft-1", DRAFT_LABELS)
            },
        )
        .await;
        mount_message(&server, metadata("received", &[label_ids::INBOX])).await;
        Mock::given(method("PUT"))
            .and(path("/mail/v4/messages/draft-1"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"Code": 1000, "Message": {"ID": "draft-1"}})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let second = address("second@proton.me", "address-2");
        let client = client(
            &server,
            vec![address("primary@proton.me", "address-1"), second.clone()],
        );
        let opts = SendOptions {
            to: vec!["friend@proton.me".into()],
            body: "Revised.".into(),
            ..SendOptions::default()
        };

        client.update_draft("draft-1", &opts).await.unwrap();

        let requests = server.received_requests().await.unwrap();
        let updates = requests_to(&requests, "/mail/v4/messages/draft-1");
        let update = updates
            .iter()
            .find(|request| request.method.as_str() == "PUT")
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&update.body).unwrap();
        assert_eq!(body["Message"]["Sender"]["Address"], "second@proton.me");
        let armored = body["Message"]["Body"].as_str().unwrap();
        let (text, _) = crypto::decrypt_body(&crypto::provider(), &second, &[], armored).unwrap();
        assert_eq!(text, "Revised.");

        // Its attachment is keyed to that address, so it cannot move to another.
        let moved = SendOptions {
            from: Some("primary@proton.me".into()),
            ..opts.clone()
        };
        assert!(client.update_draft("draft-1", &moved).await.is_err());
        // Nor can an edit reach mail that is not a draft.
        assert!(client.update_draft("received", &opts).await.is_err());
        server.verify().await;
    }

    #[tokio::test]
    async fn deleting_drafts_refuses_any_other_message() {
        let server = MockServer::start().await;
        mount_message(&server, metadata("draft", DRAFT_LABELS)).await;
        mount_message(
            &server,
            metadata("received", &[label_ids::INBOX, label_ids::ALL_MAIL]),
        )
        .await;
        Mock::given(method("PUT"))
            .and(path("/mail/v4/messages/delete"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"Code": 1000})),
            )
            .mount(&server)
            .await;
        let client = client(&server, Vec::new());

        // One received message among the IDs stops the whole delete.
        let refused = client
            .delete_draft(&["draft".into(), "received".into()])
            .await
            .unwrap_err();
        assert!(refused.to_string().contains("received is not a draft"));
        assert_eq!(deletions(&server).await, 0);

        client.delete_draft(&["draft".into()]).await.unwrap();
        assert_eq!(deletions(&server).await, 1);
    }
}
