//! Export reconstructed MIME messages to `.eml` files.

use super::Client;
use super::read::{FullMessage, UnreadableMessage};
use crate::api;
use crate::api::messages::ListQuery;
use crate::error::{Error, Result};
use crate::mail::attachments::safe_attachment_name;
use crate::model::enums::resolve_folder;
use crate::model::message::{Attachment, Message, Recipient};
use base64::{Engine, engine::general_purpose::STANDARD};
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;

const MIME_BOUNDARY: &str = "ruston-mail-export";

/// What an export did with the messages it went through.
#[derive(Debug, Default)]
pub struct ExportReport {
    /// Messages written to a new file.
    pub written: usize,
    /// Messages whose file an earlier export already wrote.
    pub already_exported: usize,
    /// Messages with no file, because they or their attachments could not be
    /// opened.
    pub skipped: Vec<UnreadableMessage>,
}

/// What exporting one message did.
enum Exported {
    Written,
    AlreadyThere,
    Skipped(Box<UnreadableMessage>),
}

impl Client {
    /// Export up to `max` messages from a folder as reconstructed MIME `.eml`
    /// files, including decrypted attachments, as
    /// [`export_folder_report`](Self::export_folder_report) does. Returns the
    /// number written, or the error of the first message that could not be
    /// exported, once the others are written.
    pub async fn export_folder(&self, folder: &str, out_dir: &Path, max: u32) -> Result<usize> {
        let report = self.export_folder_report(folder, out_dir, max).await?;
        match report.skipped.into_iter().next() {
            Some(skipped) => Err(skipped.error),
            None => Ok(report.written),
        }
    }

    /// Export up to `max` messages from a folder as reconstructed MIME `.eml`
    /// files, including decrypted attachments, and report what happened to
    /// each. Existing files are never replaced: a message whose file is
    /// already there counts as exported, so running again into the same
    /// folder resumes. A message that cannot be opened, or whose attachments
    /// cannot be, is skipped and the others are still exported. A failed
    /// request stops the export.
    pub async fn export_folder_report(
        &self,
        folder: &str,
        out_dir: &Path,
        max: u32,
    ) -> Result<ExportReport> {
        std::fs::create_dir_all(out_dir)?;
        let label = resolve_folder(folder);
        let page_size = 50u32;
        let mut report = ExportReport::default();
        // Every message gone through counts, so a rerun covers the same ones.
        let mut considered = 0u32;
        let mut page = 0u32;
        while considered < max {
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
                if considered >= max {
                    break;
                }
                match self.export_message(&meta.id, out_dir).await? {
                    Exported::Written => report.written += 1,
                    Exported::AlreadyThere => report.already_exported += 1,
                    Exported::Skipped(skipped) => report.skipped.push(*skipped),
                }
                considered += 1;
            }
            if (msgs.len() as u32) < page_size {
                break;
            }
            page += 1;
        }
        tracing::info!(
            target: "ruston_core::mail",
            folder,
            exported = report.written,
            already_exported = report.already_exported,
            skipped = report.skipped.len(),
            "export_folder"
        );
        Ok(report)
    }

    async fn export_message(&self, message_id: &str, out_dir: &Path) -> Result<Exported> {
        let path = out_dir.join(export_filename(message_id));
        // Only finished files ever get this name.
        if std::fs::symlink_metadata(&path).is_ok() {
            return Ok(Exported::AlreadyThere);
        }
        let message = api::messages::get_message(self.http(), message_id).await?;
        let full = match self.open_message(message.clone()).await {
            Ok(full) => full,
            Err(error) => return Ok(skipped(message, error)),
        };
        // The file is written under a hidden name and moved into place once
        // complete, so an interrupted export never leaves a partial file
        // that a rerun would take as exported.
        let temp = out_dir.join(format!(".{}.tmp", export_filename(message_id)));
        // One left by an interrupted export is incomplete.
        let _ = std::fs::remove_file(&temp);
        let mut file = BufWriter::new(create_export_file(&temp)?);

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
            file.get_ref().sync_all()?;
            Ok(())
        }
        .await;

        drop(file);
        let placed = result.and_then(|()| place_export(&temp, &path));
        // After a hard link the file has both names; after anything else
        // the hidden one is gone or unfinished.
        let _ = std::fs::remove_file(&temp);
        match placed {
            Ok(true) => Ok(Exported::Written),
            Ok(false) => Ok(Exported::AlreadyThere),
            Err(error) if is_about_the_message(&error) => Ok(skipped(message, error)),
            Err(error) => Err(error),
        }
    }
}

fn skipped(message: Message, error: Error) -> Exported {
    tracing::debug!(target: "ruston_core::mail", message_id = %message.meta.id, error = %error, "message skipped by export");
    Exported::Skipped(Box::new(UnreadableMessage {
        meta: message.meta,
        reply_tos: message.reply_tos,
        error,
    }))
}

/// Whether writing a message's file failed because of that message alone,
/// so the export skips it and goes on. Anything else, such as a failed
/// request or a full disk, would fail the next message too and stops the
/// export.
fn is_about_the_message(error: &Error) -> bool {
    match error {
        Error::Crypto(_)
        | Error::AttachmentVerificationFailed
        | Error::AttachmentTooLarge { .. }
        | Error::ResponseTooLarge { .. }
        | Error::Other(_) => true,
        Error::Api(_)
        | Error::Unauthorized
        | Error::HumanVerification(_)
        | Error::Http(_)
        | Error::AttachmentBatchTooLarge { .. }
        | Error::AttachmentUnavailable { .. }
        | Error::SendUnconfirmed { .. }
        | Error::Json(_)
        | Error::Srp(_)
        | Error::Session(_)
        | Error::Cache(_)
        | Error::Io(_)
        | Error::NotFound { .. }
        | Error::Ambiguous(_) => false,
    }
}

/// Moves a finished export from `temp` to `path` unless a file is already
/// there, and says whether it moved. A hard link fails when the name is
/// taken, so nothing is ever replaced; on a file system without hard links,
/// a rename follows a check that the name is free.
fn place_export(temp: &Path, path: &Path) -> Result<bool> {
    match std::fs::hard_link(temp, path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(_) if std::fs::symlink_metadata(path).is_ok() => Ok(false),
        Err(_) => {
            std::fs::rename(temp, path)?;
            Ok(true)
        }
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
    Ok(create_private(path)?)
}

/// Creates a new file at `path` for writing that only its owner can open,
/// for decrypted mail: mode `0600` on Unix, and on Windows an access list
/// for the owner, the system and administrators, as a user's profile
/// folders have, that takes nothing from the folder it is in. Otherwise a
/// file in a shared folder would let every user who can read there read it
/// too. Fails if `path` exists.
pub fn create_private(path: &Path) -> io::Result<File> {
    #[cfg(windows)]
    {
        create_private_on_windows(path)
    }
    #[cfg(not(windows))]
    {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options.open(path)
    }
}

/// Full access for the owner, the system and administrators, in a protected
/// list that inherits nothing.
#[cfg(windows)]
const PRIVATE_FILE_ACCESS: &str = "D:P(A;;FA;;;OW)(A;;FA;;;SY)(A;;FA;;;BA)";

/// Creates the file with its access list already in place, so no other user
/// can open it, and keep it open, before it is restricted.
#[cfg(windows)]
fn create_private_on_windows(path: &Path) -> io::Result<File> {
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::Foundation::{GENERIC_WRITE, INVALID_HANDLE_VALUE, LocalFree};
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::Storage::FileSystem::{
        CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE,
    };

    let name = verbatim(path)?;
    let access: Vec<u16> = PRIVATE_FILE_ACCESS.encode_utf16().chain([0]).collect();
    let mut descriptor = std::ptr::null_mut();
    // SAFETY: `access` is NUL-terminated, and on success the descriptor is
    // this function's to free with LocalFree.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            access.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    // SAFETY: `name` is NUL-terminated and `attributes` points at the live
    // descriptor. Sharing is std's, so the file can be renamed while open.
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            &attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        )
    };
    let created = if handle == INVALID_HANDLE_VALUE {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: the handle was just opened, and nothing else owns it.
        Ok(unsafe { File::from_raw_handle(handle) })
    };
    // SAFETY: the descriptor came from the conversion above, and the file
    // holds its own copy.
    unsafe { LocalFree(descriptor) };
    created
}

/// `path` as CreateFileW takes it at any length: absolute and verbatim, the
/// form std's own file functions use for long names.
#[cfg(windows)]
fn verbatim(path: &Path) -> io::Result<Vec<u16>> {
    use std::os::windows::ffi::OsStrExt;
    let wide = |text: &str| text.encode_utf16().collect::<Vec<u16>>();
    let absolute: Vec<u16> = std::path::absolute(path)?
        .as_os_str()
        .encode_wide()
        .collect();
    let mut name = if absolute.starts_with(&wide(r"\\?\")) || absolute.starts_with(&wide(r"\\.\")) {
        absolute
    } else if let Some(share) = absolute.strip_prefix(wide(r"\\").as_slice()) {
        [wide(r"\\?\UNC\").as_slice(), share].concat()
    } else {
        [wide(r"\\?\"), absolute].concat()
    };
    name.push(0);
    Ok(name)
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
            reply_tos: vec![],
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

    /// The access list of `path` in SDDL, as icacls would read it.
    #[cfg(windows)]
    fn access_list(path: &Path) -> String {
        use windows_sys::Win32::Foundation::LocalFree;
        use windows_sys::Win32::Security::Authorization::{
            ConvertSecurityDescriptorToStringSecurityDescriptorW, GetNamedSecurityInfoW,
            SDDL_REVISION_1, SE_FILE_OBJECT,
        };
        use windows_sys::Win32::Security::DACL_SECURITY_INFORMATION;

        let name = verbatim(path).unwrap();
        let mut descriptor = std::ptr::null_mut();
        let mut text = std::ptr::null_mut();
        let mut length = 0;
        // SAFETY: `name` is NUL-terminated; both outputs are freed below.
        unsafe {
            assert_eq!(
                GetNamedSecurityInfoW(
                    name.as_ptr(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    &mut descriptor,
                ),
                0
            );
            assert_ne!(
                ConvertSecurityDescriptorToStringSecurityDescriptorW(
                    descriptor,
                    SDDL_REVISION_1,
                    DACL_SECURITY_INFORMATION,
                    &mut text,
                    &mut length,
                ),
                0
            );
            let sddl = String::from_utf16_lossy(std::slice::from_raw_parts(text, length as usize));
            LocalFree(text.cast());
            LocalFree(descriptor);
            sddl.trim_end_matches('\0').to_owned()
        }
    }

    #[cfg(windows)]
    #[test]
    fn a_private_file_on_windows_takes_nothing_from_its_folder() {
        use windows_sys::Win32::Security::Authorization::{
            ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
        };
        use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
        use windows_sys::Win32::Storage::FileSystem::CreateDirectoryW;

        // A folder that lets everyone read what it holds, as a shared one can.
        let dir = std::env::temp_dir().join(format!(
            "ruston-private-file-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let shared: Vec<u16> = "D:(A;OICI;FA;;;WD)".encode_utf16().chain([0]).collect();
        let mut descriptor = std::ptr::null_mut();
        // SAFETY: both strings are NUL-terminated; the descriptor lives until
        // the folder is created and is left to the test process to reclaim.
        unsafe {
            assert_ne!(
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    shared.as_ptr(),
                    SDDL_REVISION_1,
                    &mut descriptor,
                    std::ptr::null_mut(),
                ),
                0
            );
            let attributes = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor,
                bInheritHandle: 0,
            };
            assert_ne!(
                CreateDirectoryW(verbatim(&dir).unwrap().as_ptr(), &attributes),
                0
            );
        }

        let ordinary = dir.join("ordinary.eml");
        std::fs::write(&ordinary, b"").unwrap();
        assert!(access_list(&ordinary).contains(";WD)"));

        let private = dir.join("private.eml");
        drop(create_private(&private).unwrap());
        assert_eq!(access_list(&private), PRIVATE_FILE_ACCESS);
        assert_eq!(
            create_private(&private).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );

        // Past MAX_PATH, as std allows.
        let deep = dir.join("a".repeat(120)).join("b".repeat(120));
        std::fs::create_dir_all(&deep).unwrap();
        let long = deep.join("private.eml");
        assert!(long.as_os_str().len() > 260);
        drop(create_private(&long).unwrap());
        assert_eq!(access_list(&long), PRIVATE_FILE_ACCESS);

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn placing_an_export_never_replaces_what_is_there() {
        let dir = std::env::temp_dir().join(format!(
            "ruston-export-place-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&dir).unwrap();
        let temp = dir.join(".message.eml.tmp");
        let path = dir.join("message.eml");

        std::fs::write(&temp, b"first").unwrap();
        let first = place_export(&temp, &path).unwrap();
        std::fs::remove_file(&temp).unwrap();
        std::fs::write(&temp, b"second").unwrap();
        let second = place_export(&temp, &path).unwrap();
        // A dangling symbolic link is a name already taken, not a target.
        #[cfg(unix)]
        let through_link = {
            let link = dir.join("link.eml");
            std::os::unix::fs::symlink(dir.join("missing.eml"), &link).unwrap();
            place_export(&temp, &link).unwrap()
        };
        let kept = std::fs::read(&path).unwrap();
        let missing_created = dir.join("missing.eml").exists();
        std::fs::remove_dir_all(dir).unwrap();

        assert!(first);
        assert!(!second);
        assert_eq!(kept, b"first");
        #[cfg(unix)]
        assert!(!through_link);
        assert!(!missing_created);
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
