//! Export reconstructed MIME messages to `.eml` files.

use super::Client;
use super::read::FullMessage;
use crate::api;
use crate::api::messages::ListQuery;
use crate::error::{Error, Result};
use crate::mail::attachments::safe_attachment_name;
use crate::model::enums::resolve_folder;
use crate::model::message::{Attachment, Recipient};
use base64::{Engine, engine::general_purpose::STANDARD};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;

const MIME_BOUNDARY: &str = "ruston-mail-export";

impl Client {
    /// Export up to `max` messages from a folder as reconstructed MIME `.eml`
    /// files, including decrypted attachments. Existing files are never replaced.
    /// Returns the number written.
    pub async fn export_folder(&self, folder: &str, out_dir: &Path, max: u32) -> Result<usize> {
        std::fs::create_dir_all(out_dir)?;
        let label = resolve_folder(folder);
        let page_size = 50u32;
        let mut written = 0u32;
        let mut page = 0u32;
        while written < max {
            let q = ListQuery {
                label_id: Some(label.clone()),
                page: Some(page),
                page_size: Some(page_size),
                ..Default::default()
            };
            let (_total, msgs) = api::messages::list_messages(self.http(), &q).await?;
            if msgs.is_empty() {
                break;
            }
            for meta in &msgs {
                if written >= max {
                    break;
                }
                self.export_message(&meta.id, out_dir).await?;
                written += 1;
            }
            if (msgs.len() as u32) < page_size {
                break;
            }
            page += 1;
        }
        tracing::info!(target: "ruston_core::mail", folder, exported = written, "export_folder");
        Ok(written as usize)
    }

    async fn export_message(&self, message_id: &str, out_dir: &Path) -> Result<()> {
        let message = api::messages::get_message(self.http(), message_id).await?;
        let full = self.decrypt_message(&message).await?;
        let path = out_dir.join(export_filename(message_id));
        let mut file = BufWriter::new(create_export_file(&path)?);

        let result = async {
            let mut eml = EmlWriter::new(&mut file, &full)?;
            for attachment in &message.attachments {
                let (_, bytes) = self
                    .download_attachment_from_message(&message, attachment)
                    .await?;
                eml.attachment(attachment, &bytes)?;
            }
            eml.finish()?;
            file.flush()?;
            Ok(())
        }
        .await;

        drop(file);
        if result.is_err() {
            let _ = std::fs::remove_file(path);
        }
        result
    }
}

/// Hex preserves every ID byte and stays distinct on case-insensitive filesystems.
fn export_filename(id: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut name = String::with_capacity(12 + id.len() * 2);
    name.push_str("message-");
    for byte in id.bytes() {
        name.push(HEX[(byte >> 4) as usize] as char);
        name.push(HEX[(byte & 0x0f) as usize] as char);
    }
    name.push_str(".eml");
    name
}

fn create_export_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}

struct EmlWriter<W: Write> {
    out: W,
    multipart: bool,
}

impl<W: Write> EmlWriter<W> {
    fn new(mut out: W, message: &FullMessage) -> Result<Self> {
        write_address_header(&mut out, "From", std::slice::from_ref(&message.meta.sender))?;
        write_address_header(&mut out, "To", &message.meta.to_list)?;
        write_address_header(&mut out, "Cc", &message.meta.cc_list)?;
        write_address_header(&mut out, "Bcc", &message.meta.bcc_list)?;
        write_encoded_header(&mut out, "Subject", &message.meta.subject)?;
        let date = chrono::DateTime::from_timestamp(message.meta.time, 0)
            .unwrap_or(chrono::DateTime::UNIX_EPOCH);
        write!(out, "Date: {}\r\n", date.to_rfc2822())?;
        out.write_all(b"MIME-Version: 1.0\r\n")?;

        let multipart = !message.attachments.is_empty();
        if multipart {
            write!(
                out,
                "Content-Type: multipart/mixed; boundary=\"{MIME_BOUNDARY}\"\r\n\r\n--{MIME_BOUNDARY}\r\n"
            )?;
        }
        let body_type = if crate::html::is_html_mime(&message.mime_type) {
            "text/html"
        } else {
            "text/plain"
        };
        write!(
            out,
            "Content-Type: {body_type}; charset=UTF-8\r\nContent-Transfer-Encoding: base64\r\n\r\n"
        )?;
        write_base64_text(&mut out, &message.body)?;
        Ok(Self { out, multipart })
    }

    fn attachment(&mut self, attachment: &Attachment, bytes: &[u8]) -> Result<()> {
        if !self.multipart {
            return Err(Error::Other("attachment in non-multipart export".into()));
        }
        write!(
            self.out,
            "--{MIME_BOUNDARY}\r\nContent-Type: {}\r\nContent-Transfer-Encoding: base64\r\nContent-Disposition: {}",
            attachment_content_type(attachment),
            if attachment.is_inline() {
                "inline"
            } else {
                "attachment"
            }
        )?;
        write_filename_parameter(&mut self.out, &safe_attachment_name(&attachment.name))?;
        self.out.write_all(b"\r\n\r\n")?;
        write_base64_bytes(&mut self.out, bytes)?;
        Ok(())
    }

    fn finish(mut self) -> Result<W> {
        if self.multipart {
            write!(self.out, "--{MIME_BOUNDARY}--\r\n")?;
        }
        Ok(self.out)
    }
}

fn checked_address(address: &str) -> Result<&str> {
    if address.is_empty()
        || address.len() > 512
        || address
            .chars()
            .any(|c| c.is_control() || c == '<' || c == '>')
    {
        return Err(Error::Other(
            "message has an invalid address for EML export".into(),
        ));
    }
    Ok(address)
}

fn write_address_header<W: Write>(out: &mut W, field: &str, addresses: &[Recipient]) -> Result<()> {
    if addresses.is_empty() {
        return Ok(());
    }
    write!(out, "{field}:")?;
    for (index, recipient) in addresses.iter().enumerate() {
        let address = checked_address(&recipient.address)?;
        if index > 0 {
            out.write_all(b",\r\n")?;
        }
        if !recipient.name.is_empty() {
            write_encoded_words(out, &recipient.name)?;
            write!(out, "\r\n <{address}>")?;
        } else {
            write!(out, " <{address}>")?;
        }
    }
    out.write_all(b"\r\n")?;
    Ok(())
}

fn write_encoded_header<W: Write>(out: &mut W, field: &str, value: &str) -> Result<()> {
    write!(out, "{field}:")?;
    write_encoded_words(out, value)?;
    out.write_all(b"\r\n")?;
    Ok(())
}

/// RFC 2047 encoded words, folded below the 76-character line limit.
fn write_encoded_words<W: Write>(out: &mut W, value: &str) -> Result<()> {
    let mut chunk = String::new();
    let mut first = true;
    for character in value.chars() {
        let character = if character.is_control() {
            ' '
        } else {
            character
        };
        if chunk.len() + character.len_utf8() > 36 {
            write_encoded_word(out, &chunk, first)?;
            chunk.clear();
            first = false;
        }
        chunk.push(character);
    }
    if !chunk.is_empty() {
        write_encoded_word(out, &chunk, first)?;
    }
    Ok(())
}

fn write_encoded_word<W: Write>(out: &mut W, chunk: &str, first: bool) -> Result<()> {
    if first {
        out.write_all(b" ")?;
    } else {
        out.write_all(b"\r\n ")?;
    }
    write!(out, "=?UTF-8?B?{}?=", STANDARD.encode(chunk.as_bytes()))?;
    Ok(())
}

fn attachment_content_type(attachment: &Attachment) -> &str {
    attachment
        .mime_type
        .as_deref()
        .filter(|value| {
            value.len() <= 255
                && value.split_once('/').is_some_and(|(kind, subtype)| {
                    !subtype.contains('/') && is_mime_token(kind) && is_mime_token(subtype)
                })
        })
        .unwrap_or("application/octet-stream")
}

fn is_mime_token(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"!#$&^_.+-".contains(&byte))
}

/// RFC 2231 extended filename parameter, continued on short folded lines.
fn write_filename_parameter<W: Write>(out: &mut W, filename: &str) -> Result<()> {
    let mut chunks = vec![String::new()];
    for byte in filename.bytes() {
        let piece = if byte.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&byte) {
            (byte as char).to_string()
        } else {
            format!("%{byte:02X}")
        };
        if chunks
            .last()
            .is_some_and(|chunk| chunk.len() + piece.len() > 40)
        {
            chunks.push(String::new());
        }
        chunks.last_mut().unwrap().push_str(&piece);
    }
    if chunks.len() == 1 {
        write!(out, ";\r\n filename*=UTF-8''{}", chunks[0])?;
    } else {
        for (index, chunk) in chunks.iter().enumerate() {
            let charset = if index == 0 { "UTF-8''" } else { "" };
            write!(out, ";\r\n filename*{index}*={charset}{chunk}")?;
        }
    }
    Ok(())
}

struct Base64Lines<'a, W: Write> {
    out: &'a mut W,
    line: [u8; 57],
    len: usize,
}

impl<'a, W: Write> Base64Lines<'a, W> {
    fn new(out: &'a mut W) -> Self {
        Self {
            out,
            line: [0; 57],
            len: 0,
        }
    }

    fn write_bytes(&mut self, mut bytes: &[u8]) -> Result<()> {
        while !bytes.is_empty() {
            let count = (57 - self.len).min(bytes.len());
            self.line[self.len..self.len + count].copy_from_slice(&bytes[..count]);
            self.len += count;
            bytes = &bytes[count..];
            if self.len == 57 {
                self.flush_line()?;
            }
        }
        Ok(())
    }

    fn flush_line(&mut self) -> Result<()> {
        write!(self.out, "{}\r\n", STANDARD.encode(&self.line[..self.len]))?;
        self.len = 0;
        Ok(())
    }

    fn finish(mut self) -> Result<()> {
        if self.len > 0 {
            self.flush_line()?;
        }
        Ok(())
    }
}

fn write_base64_bytes<W: Write>(out: &mut W, bytes: &[u8]) -> Result<()> {
    let mut encoder = Base64Lines::new(out);
    encoder.write_bytes(bytes)?;
    encoder.finish()
}

fn write_base64_text<W: Write>(out: &mut W, text: &str) -> Result<()> {
    let mut encoder = Base64Lines::new(out);
    let bytes = text.as_bytes();
    let mut start = 0;
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\r' || bytes[index] == b'\n' {
            encoder.write_bytes(&bytes[start..index])?;
            if bytes[index] == b'\r' && bytes.get(index + 1) == Some(&b'\n') {
                index += 1;
            }
            encoder.write_bytes(b"\r\n")?;
            start = index + 1;
        }
        index += 1;
    }
    encoder.write_bytes(&bytes[start..])?;
    encoder.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::message::{MessageMetadata, Recipient};
    use mail_parser::{MessageParser, MimeHeaders};

    fn message() -> FullMessage {
        FullMessage {
            meta: MessageMetadata {
                subject: "Café résumé".into(),
                sender: Recipient {
                    name: "Alícia".into(),
                    address: "alice@example.test".into(),
                    ..Default::default()
                },
                to_list: vec![Recipient::new("bob@example.test")],
                time: 1_700_000_000,
                ..Default::default()
            },
            body: "first\nsecond".into(),
            mime_type: "text/plain".into(),
            verdict: crate::crypto::Verdict::Verified,
            attachments: vec![],
        }
    }

    #[test]
    fn distinct_message_ids_have_distinct_filenames() {
        assert_ne!(export_filename("a+b"), export_filename("a/b"));
        assert!(!export_filename("a/b").contains('/'));
    }

    #[test]
    fn subject_cannot_inject_a_second_header() {
        let mut full = message();
        full.meta.subject = "Hello\r\nBcc: attacker@example.test".into();
        full.meta.sender.name = "Alice\r\nX-Injected: yes".into();
        let eml = EmlWriter::new(Vec::new(), &full).unwrap().finish().unwrap();
        assert!(!eml.windows(6).any(|window| window == b"\r\nBcc:"));
        assert!(!eml.windows(13).any(|window| window == b"\r\nX-Injected:"));
        let parsed = MessageParser::default().parse(&eml).unwrap();
        assert_eq!(parsed.subject(), Some("Hello  Bcc: attacker@example.test"));
        assert_eq!(
            parsed.from().unwrap().first().unwrap().name.as_deref(),
            Some("Alice  X-Injected: yes")
        );
    }

    #[test]
    fn eml_round_trips_unicode_headers_body_and_attachment() {
        let mut full = message();
        let attachment = Attachment {
            name: "résumé très long 2026.pdf".into(),
            mime_type: Some("application/pdf".into()),
            ..Default::default()
        };
        full.attachments.push(attachment.clone());
        let bytes: Vec<u8> = (0..200).map(|value| value as u8).collect();
        let mut writer = EmlWriter::new(Vec::new(), &full).unwrap();
        writer.attachment(&attachment, &bytes).unwrap();
        let eml = writer.finish().unwrap();

        let parsed = MessageParser::default().parse(&eml).unwrap();
        assert_eq!(parsed.subject(), Some("Café résumé"));
        assert_eq!(
            parsed.from().unwrap().first().unwrap().name.as_deref(),
            Some("Alícia")
        );
        assert_eq!(parsed.body_text(0).as_deref(), Some("first\r\nsecond"));
        assert_eq!(parsed.attachment_count(), 1);
        let attached = parsed.attachment(0).unwrap();
        assert_eq!(
            attached.attachment_name(),
            Some("résumé très long 2026.pdf")
        );
        assert_eq!(attached.contents(), bytes);
    }

    #[test]
    fn eml_without_attachments_has_a_single_text_part() {
        let eml = EmlWriter::new(Vec::new(), &message())
            .unwrap()
            .finish()
            .unwrap();
        let parsed = MessageParser::default().parse(&eml).unwrap();
        assert_eq!(parsed.body_text(0).as_deref(), Some("first\r\nsecond"));
        assert_eq!(parsed.attachment_count(), 0);
    }

    #[test]
    fn existing_export_is_not_overwritten() {
        let dir = std::env::temp_dir().join(format!(
            "ruston-export-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join(export_filename("a+b"));
        std::fs::write(&path, b"existing").unwrap();
        assert!(matches!(
            create_export_file(&path),
            Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::AlreadyExists
        ));
        assert_eq!(std::fs::read(&path).unwrap(), b"existing");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn invalid_address_and_mime_type_cannot_inject_headers() {
        let mut full = message();
        full.meta.sender.address = "alice@example.test\r\nBcc: attacker@example.test".into();
        assert!(EmlWriter::new(Vec::new(), &full).is_err());

        let mut full = message();
        let attachment = Attachment {
            name: "report.pdf".into(),
            mime_type: Some("application/pdf\r\nX-Injected: yes".into()),
            ..Default::default()
        };
        full.attachments.push(attachment.clone());
        let mut writer = EmlWriter::new(Vec::new(), &full).unwrap();
        writer.attachment(&attachment, b"file").unwrap();
        let eml = writer.finish().unwrap();
        assert!(!eml.windows(13).any(|window| window == b"\r\nX-Injected:"));
        assert!(
            eml.windows(b"Content-Type: application/octet-stream".len())
                .any(|window| window == b"Content-Type: application/octet-stream")
        );
    }
}
