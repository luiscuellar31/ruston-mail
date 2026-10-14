//! Draft lifecycle: save / update / list / delete (without sending).

use super::Client;
use super::send::{SendOptions, recipient_list};
use crate::api;
use crate::crypto;
use crate::error::{Error, Result};
use crate::model::enums::label_ids;
use crate::model::message::MessageMetadata;
use serde_json::json;

impl Client {
    fn draft_message_json(&self, opts: &SendOptions) -> Result<serde_json::Value> {
        let addr = match &opts.from {
            Some(e) => self.keys().address_for_email(e),
            None => self.keys().primary_address(),
        }
        .ok_or_else(|| Error::NotFound {
            kind: "sender address".into(),
        })?;
        let provider = crypto::provider();
        let mime_type = if opts.html { "text/html" } else { "text/plain" };
        let armored = crypto::encrypt_self_draft(&provider, addr, &opts.body)?;
        Ok(json!({
            "Message": {
                "ToList": recipient_list(&opts.to),
                "CCList": recipient_list(&opts.cc),
                "BCCList": recipient_list(&opts.bcc),
                "Subject": opts.subject,
                "Sender": { "Address": addr.email, "Name": "" },
                "Body": armored,
                "MIMEType": mime_type,
            }
        }))
    }

    /// Create a draft (encrypted to self, stored in Drafts) without sending it.
    pub async fn save_draft(&self, opts: &SendOptions) -> Result<String> {
        tracing::info!(target: "ruston_core::mail", to = opts.to.len(), "save_draft");
        let body = self.draft_message_json(opts)?;
        let created = api::messages::create_draft(self.http(), body).await?;
        Ok(created.meta.id)
    }

    /// Replace an existing draft's content.
    pub async fn update_draft(&self, id: &str, opts: &SendOptions) -> Result<()> {
        tracing::info!(target: "ruston_core::mail", draft_id = %id, "update_draft");
        let body = self.draft_message_json(opts)?;
        api::messages::update_draft(self.http(), id, body).await?;
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
                "{id} is not a draft; nothing was deleted"
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
    use crate::crypto::KeyStore;
    use crate::model::message::Message;
    use crate::session::{MemoryStore, Paths};
    use crate::transport::HttpClient;
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn mount_message(server: &MockServer, id: &str, labels: &[&str]) {
        let message = Message {
            meta: MessageMetadata {
                id: id.into(),
                label_ids: labels.iter().map(|label| label.to_string()).collect(),
                ..Default::default()
            },
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

    async fn deletions(server: &MockServer) -> usize {
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|request| request.url.path() == "/mail/v4/messages/delete")
            .count()
    }

    #[tokio::test]
    async fn deleting_drafts_refuses_any_other_message() {
        let server = MockServer::start().await;
        mount_message(
            &server,
            "draft",
            &[label_ids::ALL_DRAFTS, label_ids::DRAFTS],
        )
        .await;
        mount_message(
            &server,
            "received",
            &[label_ids::INBOX, label_ids::ALL_MAIL],
        )
        .await;
        Mock::given(method("PUT"))
            .and(path("/mail/v4/messages/delete"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"Code": 1000})),
            )
            .mount(&server)
            .await;
        let client = Client {
            http: HttpClient::new(server.uri(), "Other"),
            keys: KeyStore {
                user_keys: Vec::new(),
                addresses: Vec::new(),
            },
            paths: Paths::with_base(std::env::temp_dir()),
            profile: "test".into(),
            session_uid: "test".into(),
            cache_identity: CacheIdentity::new(&server.uri(), "test").unwrap(),
            store: Arc::new(MemoryStore::default()),
            sender_cache: Mutex::new(super::super::SenderKeyCache::default()),
        };

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
