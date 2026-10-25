//! Attachment listing + download (decrypt).

use super::Client;
use crate::api;
use crate::crypto;
use crate::error::{Error, Result};
use crate::model::message::{Attachment, Message};
use std::fs::OpenOptions;
use std::io::{ErrorKind, Write};
use std::path::{Component, Path, PathBuf};
use zeroize::Zeroizing;

/// Maximum combined plaintext returned by `download_all_attachments` (128 MiB).
pub const MAX_BULK_ATTACHMENT_BYTES: usize = 128 * 1024 * 1024;

/// The largest attachment that can be opened (50 MiB): proton-rpgp's default
/// reading limit, which proton-crypto applies to every decrypted plaintext.
pub const MAX_ATTACHMENT_PLAINTEXT_BYTES: usize = 50 * 1024 * 1024;

/// The longest name kept, in UTF-8 bytes. NTFS allows 255 UTF-16 units and
/// Linux and macOS file systems 255 bytes; this leaves room for the number
/// [`save_under_free_name`] adds to a copy, " (9999)" at most.
const MAX_NAME_BYTES: usize = 240;

/// How many names a saved attachment tries: its own and 9,999 numbered ones.
const FREE_NAME_ATTEMPTS: usize = 10_000;

/// Return a portable plain filename for an untrusted attachment name.
/// Invisible direction marks are dropped; path components are removed;
/// characters no file system accepts become `_`; a long name is cut, keeping
/// its extension; empty names become `attachment`, and Windows device names
/// are prefixed with an underscore.
pub fn safe_attachment_name(name: &str) -> String {
    let name: String = name.chars().filter(|&c| !is_invisible_mark(c)).collect();
    let name: String = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let name = shorten(trim_name(&name));
    let name = trim_name(&name);
    let mut components = Path::new(name).components();
    let plain_name =
        matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none();

    if name.is_empty() || !plain_name {
        return "attachment".to_owned();
    }

    if is_windows_reserved(name) {
        format!("_{name}")
    } else {
        name.to_owned()
    }
}

/// Leading dots would hide the file on Linux and macOS, and Windows drops
/// trailing dots and spaces, so a name ending in them would be saved under
/// another one than the frontend reports.
fn trim_name(name: &str) -> &str {
    name.trim_start_matches(|c: char| c == '.' || c.is_whitespace())
        .trim_end_matches(|c: char| c == '.' || c.is_whitespace())
}

/// Cuts a name longer than [`MAX_NAME_BYTES`] at a character boundary,
/// keeping a short extension.
fn shorten(name: &str) -> String {
    if name.len() <= MAX_NAME_BYTES {
        return name.to_owned();
    }
    let extension = match name.rfind('.') {
        Some(dot) if dot > 0 && name.len() - dot <= 16 => &name[dot..],
        _ => "",
    };
    let stem = &name[..name.len() - extension.len()];
    let mut end = MAX_NAME_BYTES - extension.len();
    while !stem.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{extension}", &stem[..end])
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

/// Marks a saved attachment as downloaded from the internet, as browsers mark
/// their downloads: on Windows, SmartScreen then checks the file before it
/// runs and Office opens it in Protected View. Elsewhere this does nothing.
/// The mark is an NTFS alternate data stream, so on a file system without
/// them, such as FAT32, the file stays unmarked and this returns the error.
pub fn mark_downloaded(path: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        let mut stream = path.as_os_str().to_owned();
        stream.push(":Zone.Identifier");
        // Zone 3 is the Internet zone.
        std::fs::write(stream, "[ZoneTransfer]\r\nZoneId=3\r\n")
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Ok(())
    }
}

/// Saves an attachment's `contents` in `folder` under its name made safe,
/// or as `name (1)`, `name (2)` and so on when that is taken, and returns
/// where it landed. Each name is claimed by creating the file exclusively,
/// so a file or symlink that appears meanwhile is never written through.
pub fn save_under_free_name(
    folder: &Path,
    name: &str,
    contents: &[u8],
) -> std::io::Result<PathBuf> {
    let name = safe_attachment_name(name);
    for candidate in free_names(&name).take(FREE_NAME_ATTEMPTS) {
        let path = folder.join(candidate);
        match write_new_file(&path, contents) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        ErrorKind::AlreadyExists,
        "no free name for the attachment",
    ))
}

/// Writes `contents` to a new file at `path` and marks it as downloaded.
/// Creating it exclusively fails on anything already there, a dangling
/// symlink included; a file that could not be written in full is removed.
pub fn write_new_file(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    write_new_file_with(path, |file| file.write_all(contents))
}

/// [`write_new_file`], with `write` filling the file; tests make it fail.
fn write_new_file_with(
    path: &Path,
    write: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    if let Err(error) = write(&mut file) {
        drop(file);
        let _ = std::fs::remove_file(path);
        return Err(error);
    }
    // Best effort: a file system without the mark still gets the file.
    let _ = mark_downloaded(path);
    Ok(())
}

/// `name`, then `name (1)`, `name (2)` and so on, numbered before the
/// extension.
fn free_names(name: &str) -> impl Iterator<Item = String> + '_ {
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, format!(".{extension}")),
        _ => (name, String::new()),
    };
    std::iter::once(name.to_owned())
        .chain((1..).map(move |number| format!("{stem} ({number}){extension}")))
}

fn is_windows_reserved(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).trim_end();
    if ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$", "CLOCK$"]
        .iter()
        .any(|reserved| stem.eq_ignore_ascii_case(reserved))
    {
        return true;
    }
    // COM and LPT with one digit, the superscripts ¹, ² and ³ included.
    let mut chars = stem.chars();
    let prefix: String = chars.by_ref().take(3).collect();
    let digit = chars.next();
    chars.next().is_none()
        && (prefix.eq_ignore_ascii_case("COM") || prefix.eq_ignore_ascii_case("LPT"))
        && digit.is_some_and(|c| c.is_ascii_digit() || matches!(c, '¹' | '²' | '³'))
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
            .keys_for_message(&msg.meta.address_id)
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
mod tests;
