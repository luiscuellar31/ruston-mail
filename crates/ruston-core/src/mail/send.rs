//! Sending mail. A new message, a reply or a forward has its recipients'
//! keys looked up, is saved as a draft with its files, and then goes out in
//! packages, one for each way its copies are protected.

mod attachments;
mod packages;
mod recipients;
mod reply;

pub(super) use attachments::upload_attachments;
pub use attachments::{
    MAX_OUTGOING_ATTACHMENT_BYTES, MAX_OUTGOING_ATTACHMENT_TOTAL_BYTES, validate_attachments,
};
pub(crate) use recipients::recipient_list;
pub use recipients::{PROTECTION_LOOKUP_LIMIT, RecipientProtection};
pub use reply::{
    ReplyRecipients, forward_subject, reply_recipients, reply_redirected, reply_subject,
};

use super::Client;
use super::drafts::draft_message_json;
use crate::api;
use crate::crypto::keys::AddressKeys;
use crate::crypto::{self, SessionKeyMaterial};
use crate::error::{Error, Result};
use crate::model::MimeType;
use crate::model::enums::message_action;
use crate::model::message::Message;
use packages::{
    AttachmentKey, Delivery, EncryptedBody, EncryptedOutside, SAVE_RECIPIENTS_AS_CONTACTS,
    build_packages, cleanup_failed_draft, password_entry, submit_send,
};
use recipients::{RecipientKey, dedupe, look_up_recipients};
use reply::{Answering, Carried, quote_block};
use serde_json::{Value, json};
use std::path::PathBuf;
use zeroize::Zeroizing;

/// Whether an answer carries the message it answers under its own text.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Quote {
    /// The original goes under the text, when it can be opened.
    #[default]
    Original,
    /// The answer goes without the original.
    Omitted,
}

/// A message as it is written: to send, or to keep as a draft.
#[derive(Debug, Default, Clone)]
pub struct SendOptions {
    /// The To addresses.
    pub to: Vec<String>,
    /// The Cc addresses.
    pub cc: Vec<String>,
    /// The Bcc addresses.
    pub bcc: Vec<String>,
    /// Sender address to send from. Defaults to the primary address, or for a
    /// reply or forward to the address the original message reached.
    pub from: Option<String>,
    /// The subject.
    pub subject: String,
    /// The text, as plain text or as HTML, which `html` tells apart.
    pub body: String,
    /// Whether `body` is HTML rather than plain text. PGP/Inline
    /// recipients, who can only receive text, get a plain-text version.
    pub html: bool,
    /// Regular readable file paths to attach, limited to 32 MiB each and
    /// 128 MiB combined. Files are checked again when read.
    pub attachments: Vec<PathBuf>,
    /// When to send it, in Unix seconds, for a message scheduled for later.
    pub send_at: Option<i64>,
    /// How many seconds it lasts once sent, before it deletes itself.
    pub expires_in: Option<i64>,
    /// For a reply or a forward, whether the original goes under the text; a
    /// new message answers nothing.
    pub quote: Quote,
    /// For a forward, the original's attachments, by ID, that do not go with
    /// it; it carries all the others.
    pub leave_out: Vec<String>,
}

impl SendOptions {
    /// The MIME type of the body as written.
    pub(super) fn body_mime_type(&self) -> &'static str {
        if self.html {
            MimeType::Html.as_str()
        } else {
            MimeType::PlainText.as_str()
        }
    }
}

impl Client {
    /// Look up how a message to `email` would be protected. This is the key
    /// request sending makes for each recipient; it creates no draft.
    pub async fn recipient_protection(&self, email: &str) -> Result<RecipientProtection> {
        let keys = api::keys::get_all_public_keys(self.http(), email).await?;
        Ok(RecipientProtection::of(&keys))
    }

    /// Sends a new message, which answers none, and returns its ID.
    pub async fn send(&self, opts: &SendOptions) -> Result<String> {
        self.send_with_parent(opts, None, None).await
    }

    /// Sends a new message as [`Client::send`] does, except that the copies
    /// for recipients without a key open with `password`, and `hint` helps
    /// them recall it. Recipients with a key still get copies encrypted to
    /// it.
    pub async fn send_eo(
        &self,
        opts: &SendOptions,
        password: &str,
        hint: Option<&str>,
    ) -> Result<String> {
        let protection = EncryptedOutside {
            password,
            hint: hint.unwrap_or_default(),
        };
        self.send_with_parent(opts, None, Some(protection)).await
    }

    /// Reply to a message. It goes to the recipients `opts` names, when it
    /// names any, as its writer chose them; otherwise to whoever
    /// [`reply_recipients`] picks, with its Cc too when `all`.
    pub async fn reply(&self, reference: &str, all: bool, opts: &SendOptions) -> Result<String> {
        let (parent_id, parent) = self.fetch_parent(reference).await?;
        let mut answer = self.answer_options(&parent, opts).await;
        if opts.to.is_empty() && opts.cc.is_empty() && opts.bcc.is_empty() {
            let own: Vec<String> = self
                .keys()
                .addresses
                .iter()
                .map(|a| a.email.clone())
                .collect();
            let recipients = reply_recipients(&parent.meta, &parent.reply_tos, &own);
            answer.to = recipients.to;
            if all {
                answer.cc = recipients.cc;
            }
        }
        answer.subject = reply_subject(&parent.meta.subject);
        let answering = Answering {
            parent_id: &parent_id,
            action: if all {
                message_action::REPLY_ALL
            } else {
                message_action::REPLY
            },
            carried: None,
        };
        self.send_with_parent(&answer, Some(answering), None).await
    }

    /// Forward a message, with the original's attachments except those
    /// `opts` leaves out.
    pub async fn forward(&self, reference: &str, opts: &SendOptions) -> Result<String> {
        let (parent_id, parent) = self.fetch_parent(reference).await?;
        let mut answer = self.answer_options(&parent, opts).await;
        answer.subject = forward_subject(&parent.meta.subject);
        let carried = Carried {
            address_id: parent.meta.address_id.clone(),
            key_packets: parent
                .attachments
                .iter()
                .filter(|a| !opts.leave_out.contains(&a.id))
                .filter_map(|a| Some((a.id.clone(), a.key_packets.clone()?)))
                .collect(),
        };
        let answering = Answering {
            parent_id: &parent_id,
            action: message_action::FORWARD,
            carried: Some(carried),
        };
        self.send_with_parent(&answer, Some(answering), None).await
    }

    /// The message `reference` names, and its ID, to answer it.
    async fn fetch_parent(&self, reference: &str) -> Result<(String, Message)> {
        let parent_id = self.resolve_ref(reference).await?;
        let parent = api::messages::get_message(self.http(), &parent_id).await?;
        Ok((parent_id, parent))
    }

    /// `opts` as an answer to `parent`: sent from the address the original
    /// reached unless another is chosen, with the original quoted under the
    /// text unless `opts` leaves it out. Only the text is quoted, so the
    /// original is opened as it was fetched, without asking for its sender's
    /// keys, and one that does not open is answered without a quote rather
    /// than not at all.
    async fn answer_options(&self, parent: &Message, opts: &SendOptions) -> SendOptions {
        let mut answer = opts.clone();
        answer.from = answer.from.or_else(|| self.receiving_address(parent));
        if opts.quote == Quote::Omitted {
            return answer;
        }
        let quoted = self
            .open_message_with(parent.clone(), Vec::new())
            .await
            .ok();
        if let Some(original) = quoted {
            answer.body = format!(
                "{}\n\n{}",
                answer.body,
                quote_block(parent, &original.body, &original.mime_type, answer.html)
            );
        }
        answer
    }

    /// The address `parent` reached, so an answer without a chosen sender does
    /// not reveal another of the account's addresses. `None`, also when mail
    /// can no longer go out from it, falls back to the primary address, as
    /// for a new message.
    fn receiving_address(&self, parent: &Message) -> Option<String> {
        self.keys()
            .address(&parent.meta.address_id)
            .filter(|address| address.can_send)
            .map(|address| address.email.clone())
    }

    /// Stops the message `reference` names from going at the time it was
    /// scheduled for.
    pub async fn cancel_send(&self, reference: &str) -> Result<()> {
        let id = self.resolve_ref(reference).await?;
        api::messages::cancel_send(self.http(), &id).await
    }

    /// Sends `opts` as an answer to what `answering` names, or as a new
    /// message without it; `eo` is the message password and its hint, for
    /// recipients without a key.
    async fn send_with_parent(
        &self,
        opts: &SendOptions,
        answering: Option<Answering<'_>>,
        eo: Option<EncryptedOutside<'_>>,
    ) -> Result<String> {
        validate_attachments(&opts.attachments).await?;
        let provider = crypto::provider();
        let addr = self.sender_keys(opts.from.as_deref())?;
        tracing::info!(target: "ruston_core::send", from = %addr.email, to = opts.to.len(), cc = opts.cc.len(), bcc = opts.bcc.len(), attachments = opts.attachments.len(), mime = opts.body_mime_type(), "send: starting pipeline");

        // 1. every recipient's keys before anything is created or uploaded,
        // so a recipient Proton refuses leaves no draft to clean up.
        let recipients = dedupe(&[&opts.to, &opts.cc, &opts.bcc]);
        tracing::debug!(target: "ruston_core::send", recipients = recipients.len(), "send step 1: looking up recipient keys");
        let recipients = look_up_recipients(self.http(), recipients).await?;

        // 2. create draft (self-encrypted body).
        tracing::debug!(target: "ruston_core::send", "send step 2: creating encrypted draft (POST /mail/v4/messages)");
        let mut draft = draft_message_json(addr, opts)?;
        if let Some(answering) = &answering {
            draft["ParentID"] = json!(answering.parent_id);
            draft["Action"] = json!(answering.action);
            if let Some(carried) = &answering.carried
                && !carried.key_packets.is_empty()
            {
                draft["AttachmentKeyPackets"] = carried.rekeyed(&provider, self.keys(), addr)?;
            }
        }

        let created = api::messages::create_draft(self.http(), draft).await?;
        let message_id = created.meta.id.clone();
        tracing::debug!(target: "ruston_core::send", message_id = %message_id, "send: draft created");

        // Clean up only when the failure proves the message was not sent.
        match self
            .finish_send(&provider, addr, &created, &recipients, opts, eo)
            .await
        {
            Ok(()) => {
                tracing::info!(target: "ruston_core::send", message_id = %message_id, "send: complete");
                Ok(message_id)
            }
            Err(e) => {
                cleanup_failed_draft(self.http(), &message_id, &e).await;
                Err(e)
            }
        }
    }

    async fn finish_send<P: proton_crypto::crypto::PGPProviderSync>(
        &self,
        provider: &P,
        addr: &AddressKeys,
        draft: &Message,
        recipients: &[(String, api::keys::RecipientKeys)],
        opts: &SendOptions,
        eo: Option<EncryptedOutside<'_>>,
    ) -> Result<()> {
        let message_id = draft.meta.id.as_str();
        // 3. attachments: those the draft already carries (a forward's copies
        // of the original's) are keyed for recipients like new uploads.
        let mut attachment_keys: Vec<AttachmentKey> = Vec::new();
        for attachment in &draft.attachments {
            let key_packets = attachment.key_packets.as_deref().ok_or_else(|| {
                Error::Crypto(format!(
                    "draft attachment {} has no key packet",
                    attachment.id
                ))
            })?;
            attachment_keys.push(AttachmentKey {
                id: attachment.id.clone(),
                session_key: crypto::decrypt_session_key(provider, addr, key_packets)?,
                signed: attachment
                    .signature
                    .as_deref()
                    .is_some_and(|s| !s.is_empty()),
            });
        }
        let uploaded = upload_attachments(self.http(), addr, message_id, &opts.attachments).await?;
        attachment_keys.extend(uploaded);

        if !attachment_keys.is_empty() {
            tracing::debug!(target: "ruston_core::send", count = attachment_keys.len(), "send step 3: attachments uploaded");
        }

        // 4. encrypt the transport body, then decide how each recipient's
        // copy goes out and build the packages they go in.
        let transport_body = EncryptedBody::encrypt(provider, addr, &opts.body)?;
        tracing::debug!(target: "ruston_core::send", recipients = recipients.len(), "send step 4: building packages");
        let deliveries = self
            .deliveries(
                recipients,
                &transport_body.session_key,
                &attachment_keys,
                eo,
            )
            .await?;
        let packages = build_packages(
            provider,
            addr,
            opts,
            &transport_body,
            &attachment_keys,
            deliveries,
        )?;

        // 5. send.
        let mut send_request = json!({
            "ExpirationTime": Value::Null,
            "AutoSaveContacts": SAVE_RECIPIENTS_AS_CONTACTS,
            "Packages": packages,
        });
        if let Some(t) = opts.send_at {
            send_request["DeliveryTime"] = json!(t);
        }
        if let Some(s) = opts.expires_in {
            send_request["ExpiresIn"] = json!(s);
        }
        tracing::debug!(target: "ruston_core::send", packages = packages.len(), "send step 5: submitting (POST /mail/v4/messages/{{id}})");
        submit_send(self.http(), message_id, send_request).await
    }

    /// How each recipient's copy goes out, by the key [`RecipientKey::pick`]
    /// finds. Without a key, a copy goes behind the message password when
    /// there is one, else unencrypted.
    async fn deliveries<'a>(
        &self,
        recipients: &'a [(String, api::keys::RecipientKeys)],
        body_key: &SessionKeyMaterial,
        attachment_keys: &[AttachmentKey],
        eo: Option<EncryptedOutside<'_>>,
    ) -> Result<Vec<(&'a str, Delivery<'a>)>> {
        let mut modulus_cache: Option<(String, String)> = None;
        let mut deliveries = Vec::with_capacity(recipients.len());
        for (email, keys) in recipients {
            let delivery = match (RecipientKey::pick(keys), eo) {
                (RecipientKey::Verified(key), _) => {
                    tracing::debug!(target: "ruston_core::send", recipient = %email, "internal recipient — E2E package (Type 1)");
                    Delivery::EndToEnd(key)
                }
                (RecipientKey::Published(key), _) => {
                    tracing::debug!(target: "ruston_core::send", recipient = %email, "external recipient with PGP key — PGP-inline (Type 8)");
                    Delivery::PgpInline(key)
                }
                (RecipientKey::Missing, Some(eo)) => {
                    let modulus = match &modulus_cache {
                        Some(m) => m.clone(),
                        None => {
                            let m = api::keys::get_modulus(self.http()).await?;
                            modulus_cache = Some(m.clone());
                            m
                        }
                    };
                    // Up to seconds of key derivation per recipient: keep it
                    // off the async workers.
                    let password = Zeroizing::new(eo.password.to_owned());
                    let hint = eo.hint.to_owned();
                    let body_key = body_key.clone();
                    let attachments = attachment_keys.to_vec();
                    let entry = tokio::task::spawn_blocking(move || {
                        password_entry(&password, &hint, &modulus, &body_key, &attachments)
                    })
                    .await
                    .map_err(|error| {
                        Error::Other(format!("password protection failed: {error}"))
                    })??;
                    tracing::debug!(target: "ruston_core::send", recipient = %email, "external recipient — encrypted-outside (Type 2)");
                    Delivery::Password(entry)
                }
                (RecipientKey::Missing, None) => {
                    tracing::warn!(target: "ruston_core::send", recipient = %email, "external recipient with no key — sending CLEARTEXT (Type 4)");
                    Delivery::Clear
                }
            };
            deliveries.push((email.as_str(), delivery));
        }
        Ok(deliveries)
    }
}

#[cfg(test)]
mod tests;
