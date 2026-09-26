//! Attachment listing + download (decrypt).

use super::Client;
use crate::api;
use crate::crypto;
use crate::error::{Error, Result};
use crate::model::message::Attachment;
use std::path::{Component, Path};

/// Return a portable plain filename for an untrusted attachment name.
/// Path components are removed; invalid characters and empty names become
/// `attachment`, and Windows device names are prefixed with an underscore.
pub fn safe_attachment_name(name: &str) -> String {
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
        let key_packets = att
            .key_packets
            .as_deref()
            .ok_or_else(|| Error::Crypto("attachment has no key packets".into()))?;
        let addr = self
            .keys()
            .address(&msg.meta.address_id)
            .or_else(|| self.keys().primary_address())
            .ok_or_else(|| Error::Crypto("no address key for attachment".into()))?;

        let data_packet = api::attachments::get_attachment(self.http(), attachment_id).await?;
        let provider = crypto::provider();
        let plain = crypto::decrypt_attachment(&provider, addr, key_packets, &data_packet)?;
        Ok((att.name.clone(), plain))
    }

    /// Download all (non-inline) attachments of a message.
    pub async fn download_all_attachments(
        &self,
        message_id: &str,
        include_inline: bool,
    ) -> Result<Vec<(String, Vec<u8>)>> {
        let atts = self.list_attachments(message_id, include_inline).await?;
        let mut out = Vec::new();
        for a in atts {
            out.push(self.download_attachment(message_id, &a.id).await?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::safe_attachment_name;

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
