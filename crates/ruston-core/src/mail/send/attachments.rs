//! A message's own attachments: checked against the limits, read,
//! encrypted and uploaded to its draft, a few at a time.

use super::packages::AttachmentKey;
use crate::crypto::SessionKeyMaterial;
use crate::crypto::keys::AddressKeys;
use crate::error::{Error, Result};
use crate::transport::{Doer, Request};
use crate::{api, crypto};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use tokio::sync::{Semaphore, SemaphorePermit};
use zeroize::Zeroizing;

/// Local maximum plaintext size of each newly attached file (32 MiB).
pub const MAX_OUTGOING_ATTACHMENT_BYTES: usize = 32 * 1024 * 1024;

/// Local maximum combined plaintext size of newly attached files (128 MiB).
pub const MAX_OUTGOING_ATTACHMENT_TOTAL_BYTES: usize = 128 * 1024 * 1024;

// Bound memory across concurrent clients and sends, including blocking work
// that continues after its caller is cancelled. Sequential files share this cap.
pub(super) static ATTACHMENT_SLOTS: Semaphore = Semaphore::const_new(2);

pub(super) async fn attachment_work<T: Send + 'static>(
    slots: &'static Semaphore,
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<(T, SemaphorePermit<'static>)> {
    let permit = slots
        .acquire()
        .await
        .map_err(|_| Error::Other("attachment worker unavailable".into()))?;
    tokio::task::spawn_blocking(move || work().map(|value| (value, permit)))
        .await
        .map_err(|error| Error::Other(format!("attachment worker failed: {error}")))?
}

fn open_attachment(path: &Path) -> Result<(File, u64)> {
    let unavailable = |source| Error::AttachmentUnavailable {
        path: path.to_owned(),
        source,
    };
    // Reject directories and special files before opening, then check the
    // opened handle again so a replacement is not trusted by its old metadata.
    let metadata = std::fs::metadata(path).map_err(unavailable)?;
    if !metadata.is_file() {
        return Err(unavailable(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "attachment must be a regular file",
        )));
    }
    let file = File::open(path).map_err(unavailable)?;
    let metadata = file.metadata().map_err(unavailable)?;
    if !metadata.is_file() {
        return Err(unavailable(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "attachment must be a regular file",
        )));
    }
    Ok((file, metadata.len()))
}

fn check_attachment_size(size: u64, remaining: usize) -> Result<()> {
    if size > MAX_OUTGOING_ATTACHMENT_BYTES as u64 {
        return Err(Error::AttachmentTooLarge {
            limit: MAX_OUTGOING_ATTACHMENT_BYTES,
        });
    }
    if size > remaining as u64 {
        return Err(Error::AttachmentBatchTooLarge {
            limit: MAX_OUTGOING_ATTACHMENT_TOTAL_BYTES,
        });
    }
    Ok(())
}

/// Checks regular readable files and local size limits before creating a draft.
/// Sending rechecks opened files and counts the bytes actually read, since a
/// path or file length can change after this preflight. Inherited forwarded
/// attachments are already on Proton and are not part of this local budget.
pub async fn validate_attachments(paths: &[PathBuf]) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    let paths = paths.to_vec();
    let check = move || {
        let mut remaining = MAX_OUTGOING_ATTACHMENT_TOTAL_BYTES;
        for path in paths {
            let (_, size) = open_attachment(&path)?;
            check_attachment_size(size, remaining)?;
            remaining -= size as usize;
        }
        Ok(())
    };
    if tokio::runtime::Handle::try_current().is_ok() {
        let (_, _permit) = attachment_work(&ATTACHMENT_SLOTS, check).await?;
    } else {
        // Keep the desktop's preflight usable by synchronous executors too.
        check()?;
    }
    Ok(())
}

pub(super) fn read_attachment_bytes(
    reader: impl Read,
    size: u64,
    remaining: usize,
) -> Result<Zeroizing<Vec<u8>>> {
    check_attachment_size(size, remaining)?;
    let limit = MAX_OUTGOING_ATTACHMENT_BYTES.min(remaining);
    let mut bytes = Zeroizing::new(Vec::with_capacity(size as usize));
    // Metadata is only a preflight. Read one extra byte to detect a growing
    // file without ever loading its unbounded remainder.
    reader.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    check_attachment_size(bytes.len() as u64, remaining)?;
    Ok(bytes)
}

pub(super) struct PreparedAttachment {
    pub(super) request: Request,
    pub(super) material: SessionKeyMaterial,
    pub(super) size: usize,
    // Keep the slot until the multipart buffer is uploaded or dropped.
    _permit: SemaphorePermit<'static>,
}

pub(super) async fn prepare_attachment(
    path: PathBuf,
    addr: AddressKeys,
    message_id: String,
    remaining: usize,
) -> Result<PreparedAttachment> {
    let ((request, material, size), permit) = attachment_work(&ATTACHMENT_SLOTS, move || {
        let (file, metadata_len) = open_attachment(&path)?;
        let bytes = read_attachment_bytes(file, metadata_len, remaining)?;
        let read_len = bytes.len();
        // Instantiate the provider on this worker; its opaque types need not
        // be moved between Tokio and the blocking pool.
        let encrypted = crypto::encrypt_attachment(&crypto::provider(), &addr, &bytes)?;
        drop(bytes);
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("attachment");
        let request = api::attachments::upload_request(&api::attachments::AttachmentUpload {
            filename: name,
            message_id: &message_id,
            // Only pictures shown within a body have one.
            content_id: "",
            mime_type: &guess_mime(&path),
            key_packets: &encrypted.key_packet,
            data_packet: &encrypted.data_packet,
            signature: &encrypted.signature,
        })?;
        Ok((request, encrypted.material, read_len))
    })
    .await?;
    Ok(PreparedAttachment {
        request,
        material,
        size,
        _permit: permit,
    })
}

/// Uploads files to a draft, each encrypted to the sender's key and together
/// within the combined limit, and returns what each one is keyed with.
pub(in crate::mail) async fn upload_attachments<D: Doer>(
    http: &D,
    addr: &AddressKeys,
    message_id: &str,
    paths: &[PathBuf],
) -> Result<Vec<AttachmentKey>> {
    let mut uploaded = Vec::with_capacity(paths.len());
    let mut remaining = MAX_OUTGOING_ATTACHMENT_TOTAL_BYTES;
    for path in paths {
        let prepared =
            prepare_attachment(path.clone(), addr.clone(), message_id.to_owned(), remaining)
                .await?;
        remaining -= prepared.size;
        let id = api::attachments::upload_prepared(http, prepared.request).await?;
        uploaded.push(AttachmentKey {
            id,
            session_key: prepared.material,
            signed: true,
        });
    }
    Ok(uploaded)
}

/// The MIME type an attached file is uploaded with, by its extension. A file
/// with any other extension, or none, goes as plain bytes.
const ATTACHMENT_TYPES: [(&str, &str); 9] = [
    ("pdf", "application/pdf"),
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("txt", "text/plain"),
    ("html", "text/html"),
    ("zip", "application/zip"),
    ("json", "application/json"),
];

/// The MIME type of the file at `path`, by its extension in any case.
pub(super) fn guess_mime(path: &Path) -> String {
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    ATTACHMENT_TYPES
        .iter()
        .find(|(known, _)| extension.eq_ignore_ascii_case(known))
        .map_or("application/octet-stream", |(_, mime)| *mime)
        .to_owned()
}
