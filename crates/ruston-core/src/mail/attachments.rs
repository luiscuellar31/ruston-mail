//! Attachments: listing a message's, opening them once their signatures
//! check out, and saving them to files.

use super::Client;
use crate::api;
use crate::crypto;
use crate::error::{Error, Result};
use crate::model::message::{Attachment, Message, MessageMetadata};
use zeroize::Zeroizing;

mod files;
pub use files::{mark_downloaded, safe_attachment_name, save_under_free_name, write_new_file};

/// The largest attachment that can be opened (49 MiB). proton-crypto stops
/// reading a decrypted plaintext at proton-rpgp's default limit, 50 MiB, which
/// for the usual encryption also counts the packet around the file (its name,
/// chunk lengths and integrity check); the last 1 MiB is left for that.
pub const MAX_ATTACHMENT_PLAINTEXT_BYTES: usize = 49 * 1024 * 1024;

/// What the ID of an attachment inside a PGP/MIME body starts with; its
/// position in that body follows. Proton's own IDs never take this form.
const MIME_ATTACHMENT_PREFIX: &str = "mime-";

/// The ID of the attachment at `position`, from 1, inside a PGP/MIME body.
pub(super) fn mime_attachment_id(position: usize) -> String {
    format!("{MIME_ATTACHMENT_PREFIX}{position}")
}

/// The position, from 1, that an attachment ID names inside a PGP/MIME
/// body, if it names one.
fn mime_attachment_position(id: &str) -> Option<usize> {
    let digits = id.strip_prefix(MIME_ATTACHMENT_PREFIX)?;
    if digits.is_empty() || !digits.bytes().all(|digit| digit.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok().filter(|&position| position >= 1)
}

impl Client {
    /// A message's attachments, those inside a PGP/MIME body included. Inline
    /// ones, such as images within the body, come only with `include_inline`.
    pub async fn list_attachments(
        &self,
        message_id: &str,
        include_inline: bool,
    ) -> Result<Vec<Attachment>> {
        let msg = api::messages::get_message(self.http(), message_id).await?;
        let (_, attachments) = self.attachments_of(msg).await?;
        Ok(attachments
            .into_iter()
            .filter(|a| include_inline || !a.is_inline())
            .collect())
    }

    /// A message's metadata and attachments: Proton's, and those inside a
    /// PGP/MIME body, which only show once it is opened. Listing them
    /// refuses nothing, so it opens without checking the signature.
    async fn attachments_of(&self, msg: Message) -> Result<(MessageMetadata, Vec<Attachment>)> {
        if msg.mime_type.starts_with("multipart/") {
            let opened = self.open_message_with(msg, Vec::new()).await?;
            return Ok((opened.meta, opened.attachments));
        }
        Ok((msg.meta, msg.attachments))
    }

    /// Downloads one attachment of a message and opens it, giving its file
    /// name and bytes.
    /// A nonempty detached signature must verify before plaintext is returned;
    /// unavailable sender keys or an invalid signature fail the download. One
    /// inside a PGP/MIME body, named `mime-` and its position, follows its
    /// body's signature instead: only an invalid one refuses it.
    pub async fn download_attachment(
        &self,
        message_id: &str,
        attachment_id: &str,
    ) -> Result<(String, Vec<u8>)> {
        if let Some(position) = mime_attachment_position(attachment_id) {
            return self.download_mime_attachment(message_id, position).await;
        }
        let msg = api::messages::get_message(self.http(), message_id).await?;
        let Some(att) = msg.attachments.iter().find(|a| a.id == attachment_id) else {
            return Err(Error::NotFound {
                kind: "attachment".into(),
            });
        };
        self.download_listed_attachment(&msg.meta, att).await
    }

    /// Download + decrypt one attachment of a message already read, from the
    /// message's metadata and the attachment as that read listed it, with
    /// its key packets. Unlike [`Self::download_attachment`], it does not
    /// fetch the message again. The same signature rules apply.
    pub async fn download_listed_attachment(
        &self,
        meta: &MessageMetadata,
        att: &Attachment,
    ) -> Result<(String, Vec<u8>)> {
        if let Some(position) = mime_attachment_position(&att.id) {
            return self.download_mime_attachment(&meta.id, position).await;
        }
        let too_large = || Error::AttachmentTooLarge {
            limit: MAX_ATTACHMENT_PLAINTEXT_BYTES,
        };
        // Decryption would refuse it, so it is not worth downloading.
        if att.size > MAX_ATTACHMENT_PLAINTEXT_BYTES as u64 {
            return Err(too_large());
        }
        let key_packets = att
            .key_packets
            .as_deref()
            .ok_or_else(|| Error::Crypto("attachment has no key packets".into()))?;
        let addr = self
            .keys_for_message(&meta.address_id)
            .ok_or_else(|| Error::Crypto("no address key for attachment".into()))?;

        // Resolve verifier keys before allocating and decrypting attachment data.
        // An absent or empty Signature denotes an unsigned attachment in Proton.
        let signature = att.signature.as_deref().filter(|sig| !sig.is_empty());
        let sender_pubs = if signature.is_some() {
            // A failed lookup is reported as itself, so it can be retried.
            let keys = self.sender_pubkeys(&meta.sender.address).await?;
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
        // Decrypting and verifying up to 49 MiB is CPU work for the blocking pool.
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
    /// The callback completes before the next attachment is fetched. Those
    /// inside a PGP/MIME body all come from one download of it, under the
    /// signature rules of [`Self::download_attachment`].
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
        let (meta, attachments, mut inside, refused) = if msg.mime_type.starts_with("multipart/") {
            let sender_pubs = self.verifier_keys(&msg.meta.sender.address).await;
            let (opened, inside) = self.open_message_and_parts(msg, sender_pubs).await?;
            let refused = opened.verdict == crypto::Verdict::Invalid;
            (opened.meta, opened.attachments, inside, refused)
        } else {
            (msg.meta, msg.attachments, Vec::new(), false)
        };
        for att in attachments
            .iter()
            .filter(|att| include_inline || !att.is_inline())
        {
            let (name, bytes) = self
                .take_attachment(&meta, att, &mut inside, refused)
                .await?;
            receive(name, bytes)?;
        }
        Ok(())
    }

    /// The name and bytes of `att`, listed by a message opened with
    /// [`Self::open_message_and_parts`] into `inside`, the parts of its
    /// PGP/MIME body: one of those is taken from there, unless `refused`
    /// says the body's signature is invalid, and any other is downloaded.
    pub(super) async fn take_attachment(
        &self,
        meta: &MessageMetadata,
        att: &Attachment,
        inside: &mut [Vec<u8>],
        refused: bool,
    ) -> Result<(String, Vec<u8>)> {
        match mime_attachment_position(&att.id) {
            Some(_) if refused => Err(Error::AttachmentVerificationFailed),
            Some(position) => {
                let bytes = inside
                    .get_mut(position - 1)
                    .map(std::mem::take)
                    .ok_or_else(|| Error::NotFound {
                        kind: "attachment".into(),
                    })?;
                Ok((att.name.clone(), bytes))
            }
            None => self.download_listed_attachment(meta, att).await,
        }
    }

    /// Downloads the message again and returns the attachment at `position`,
    /// from 1, inside its PGP/MIME body. It has no signature of its own: the
    /// body's covers it, so an invalid one refuses it, while an unsigned body,
    /// or one whose sender's keys cannot be found, lets it through, as
    /// reading shows such a body.
    async fn download_mime_attachment(
        &self,
        message_id: &str,
        position: usize,
    ) -> Result<(String, Vec<u8>)> {
        let not_found = || Error::NotFound {
            kind: "attachment".into(),
        };
        let msg = api::messages::get_message(self.http(), message_id).await?;
        if !msg.mime_type.starts_with("multipart/") {
            return Err(not_found());
        }
        let addr = self
            .keys_for_message(&msg.meta.address_id)
            .ok_or_else(|| Error::Crypto("no address key for message".into()))?
            .clone();
        let sender_pubs = self.verifier_keys(&msg.meta.sender.address).await;
        super::off_thread(move || {
            let provider = crypto::provider();
            let (body, verdict) =
                crypto::message::decrypt_body_bytes(&provider, &addr, &sender_pubs, &msg.body)?;
            if verdict == crypto::Verdict::Invalid {
                return Err(Error::AttachmentVerificationFailed);
            }
            super::read::mime_attachment(&body, position).ok_or_else(not_found)
        })
        .await
    }
}

#[cfg(test)]
mod tests;
