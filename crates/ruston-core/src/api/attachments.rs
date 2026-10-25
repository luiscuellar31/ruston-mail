//! Attachments API: binary download + multipart upload.

use crate::error::{Error, Result};
use crate::transport::{Doer, Request};
use serde::Deserialize;
use std::time::Duration;

/// Maximum encrypted attachment response (64 MiB). Core opens at most
/// [`MAX_ATTACHMENT_PLAINTEXT_BYTES`]; the rest leaves room for OpenPGP's
/// overhead, under 1% with usual encodings and about a quarter with the
/// smallest AEAD chunks.
///
/// [`MAX_ATTACHMENT_PLAINTEXT_BYTES`]: crate::mail::attachments::MAX_ATTACHMENT_PLAINTEXT_BYTES
pub const MAX_ATTACHMENT_RESPONSE_BYTES: usize = 64 * 1024 * 1024;
/// Maximum complete multipart upload (128 MiB), a local transport safety limit.
pub const MAX_ATTACHMENT_UPLOAD_BYTES: usize = 128 * 1024 * 1024;
/// Larger transfers have a longer deadline than small JSON requests. Each
/// attachment upload or download gets this long.
pub const ATTACHMENT_REQUEST_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Deserialize)]
struct UploadResp {
    #[serde(rename = "Attachment")]
    attachment: AttachmentId,
}
#[derive(Deserialize)]
struct AttachmentId {
    #[serde(rename = "ID")]
    id: String,
}

enum Part<'a> {
    Text(&'a str, &'a str),
    File(&'a str, &'a [u8]),
}

/// An upload's body, and the boundary between its parts.
struct Multipart {
    body: Vec<u8>,
    boundary: String,
}

/// A boundary for one upload, with 128 bits no content can predict, so no
/// part can end early or start another of its own.
fn random_boundary() -> String {
    use std::hash::{BuildHasher, RandomState};
    let state = RandomState::new();
    format!(
        "ruston-{:016x}{:016x}",
        state.hash_one(0_u8),
        state.hash_one(1_u8)
    )
}

fn build_multipart(parts: &[Part]) -> Result<Multipart> {
    // A line break in a value, such as the name of a local file, would read
    // as the end of the part's own lines.
    for part in parts {
        if let Part::Text(name, value) = part
            && value.contains(['\r', '\n'])
        {
            return Err(Error::Other(format!(
                "an attachment's {} cannot contain a line break",
                name.to_ascii_lowercase()
            )));
        }
    }
    let boundary = random_boundary();
    let end = format!("--{boundary}--\r\n");
    let mut size = end.len();
    let mut headers = Vec::with_capacity(parts.len());
    for p in parts {
        let (header, len) = match p {
            Part::Text(name, value) => (
                format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n"),
                value.len(),
            ),
            Part::File(name, data) => (
                format!(
                    "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"blob\"\r\nContent-Type: application/octet-stream\r\n\r\n"
                ),
                data.len(),
            ),
        };
        size = size
            .checked_add(header.len())
            .and_then(|n| n.checked_add(len))
            .and_then(|n| n.checked_add(2))
            .filter(|n| *n <= MAX_ATTACHMENT_UPLOAD_BYTES)
            .ok_or_else(|| {
                Error::Other(format!(
                    "attachment upload exceeds the {MAX_ATTACHMENT_UPLOAD_BYTES} byte limit"
                ))
            })?;
        headers.push(header);
    }
    let mut out = Vec::with_capacity(size);
    for (p, header) in parts.iter().zip(headers) {
        out.extend_from_slice(header.as_bytes());
        match p {
            Part::Text(_, value) => out.extend_from_slice(value.as_bytes()),
            Part::File(_, data) => out.extend_from_slice(data),
        };
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(end.as_bytes());
    Ok(Multipart {
        body: out,
        boundary,
    })
}

/// Download the raw (encrypted) attachment data packet.
pub async fn get_attachment<D: Doer>(d: &D, id: &str) -> Result<Vec<u8>> {
    let resp = d
        .do_raw(
            Request::get(format!("/mail/v4/attachments/{id}"))
                .max_response_bytes(MAX_ATTACHMENT_RESPONSE_BYTES)
                .timeout(ATTACHMENT_REQUEST_TIMEOUT),
        )
        .await?;
    Ok(resp.body)
}

/// Upload an encrypted attachment to a draft. Returns the new attachment ID.
#[allow(clippy::too_many_arguments)]
pub async fn upload_attachment<D: Doer>(
    d: &D,
    filename: &str,
    message_id: &str,
    content_id: &str,
    mime_type: &str,
    key_packets: &[u8],
    data_packet: &[u8],
    signature: &[u8],
) -> Result<String> {
    let request = upload_request(
        filename,
        message_id,
        content_id,
        mime_type,
        key_packets,
        data_packet,
        signature,
    )?;
    upload_prepared(d, request).await
}

/// Builds the replayable upload on the send pipeline's blocking worker.
#[allow(clippy::too_many_arguments)]
pub(crate) fn upload_request(
    filename: &str,
    message_id: &str,
    content_id: &str,
    mime_type: &str,
    key_packets: &[u8],
    data_packet: &[u8],
    signature: &[u8],
) -> Result<Request> {
    let multipart = build_multipart(&[
        Part::Text("Filename", filename),
        Part::Text("MessageID", message_id),
        Part::Text("ContentID", content_id),
        Part::Text("MIMEType", mime_type),
        Part::File("KeyPackets", key_packets),
        Part::File("DataPacket", data_packet),
        Part::File("Signature", signature),
    ])?;
    let content_type = format!("multipart/form-data; boundary={}", multipart.boundary);
    Ok(Request::post("/mail/v4/attachments")
        .raw(multipart.body, content_type)
        .timeout(ATTACHMENT_REQUEST_TIMEOUT))
}

pub(crate) async fn upload_prepared<D: Doer>(d: &D, request: Request) -> Result<String> {
    let r: UploadResp = d.decode(request).await?;
    Ok(r.attachment.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::{Body, Response};
    use async_trait::async_trait;
    use serde::de::DeserializeOwned;

    struct CheckAttachmentLimit;

    #[async_trait]
    impl Doer for CheckAttachmentLimit {
        async fn do_raw(&self, req: Request) -> Result<Response> {
            assert_eq!(req.max_response_bytes, MAX_ATTACHMENT_RESPONSE_BYTES);
            assert_eq!(req.timeout, ATTACHMENT_REQUEST_TIMEOUT);
            Ok(Response {
                status: 200,
                body: b"encrypted".to_vec(),
                retry_after: None,
            })
        }

        async fn decode<T: DeserializeOwned>(&self, req: Request) -> Result<T> {
            assert_eq!(req.timeout, ATTACHMENT_REQUEST_TIMEOUT);
            Ok(serde_json::from_value(serde_json::json!({
                "Attachment": { "ID": "uploaded" }
            }))?)
        }
    }

    #[tokio::test]
    async fn attachment_download_uses_binary_response_limit() {
        assert_eq!(
            get_attachment(&CheckAttachmentLimit, "att-1")
                .await
                .unwrap(),
            b"encrypted"
        );
    }

    #[tokio::test]
    async fn attachment_upload_uses_transfer_deadline() {
        let id = upload_attachment(
            &CheckAttachmentLimit,
            "file.txt",
            "message",
            "content",
            "text/plain",
            b"keys",
            b"data",
            b"signature",
        )
        .await
        .unwrap();
        assert_eq!(id, "uploaded");
    }

    #[test]
    fn multipart_contains_all_parts() {
        let Multipart { body, boundary } = build_multipart(&[
            Part::Text("Filename", "f.txt"),
            Part::File("DataPacket", b"\x00\x01"),
        ])
        .unwrap();
        let s = String::from_utf8_lossy(&body);
        assert!(s.contains("name=\"Filename\""));
        assert!(s.contains("f.txt"));
        assert!(s.contains("name=\"DataPacket\""));
        assert!(s.contains("application/octet-stream"));
        assert!(s.trim_end().ends_with(&format!("--{boundary}--")));
        assert_eq!(body.capacity(), body.len());
    }

    #[test]
    fn every_upload_has_a_boundary_of_its_own() {
        let upload = || upload_request("f.txt", "m", "", "text/plain", b"k", b"d", b"s").unwrap();
        let boundary = |request: &Request| {
            let content_type = request.content_type.clone().unwrap();
            let boundary = content_type
                .strip_prefix("multipart/form-data; boundary=")
                .unwrap()
                .to_owned();
            let Body::Bytes(body) = &request.body else {
                panic!("an upload has a raw body");
            };
            // The header names the boundary the body uses.
            assert!(body.ends_with(format!("--{boundary}--\r\n").as_bytes()));
            boundary
        };
        assert_ne!(boundary(&upload()), boundary(&upload()));
    }

    #[test]
    fn a_line_break_cannot_reach_the_upload_form() {
        for name in ["f.txt\r\n--x", "f\nMessageID", "f\r"] {
            let refused = upload_request(name, "m", "", "text/plain", b"k", b"d", b"s");
            assert!(
                matches!(&refused, Err(Error::Other(message)) if message.contains("line break")),
                "{name:?}"
            );
        }
    }

    #[test]
    fn oversized_multipart_is_rejected_before_allocating_the_body() {
        let data = vec![0; MAX_ATTACHMENT_UPLOAD_BYTES / 4];
        let result = build_multipart(&[
            Part::File("a", &data),
            Part::File("b", &data),
            Part::File("c", &data),
            Part::File("d", &data),
        ]);
        assert!(
            matches!(result, Err(Error::Other(message)) if message.contains("attachment upload exceeds"))
        );
    }
}
