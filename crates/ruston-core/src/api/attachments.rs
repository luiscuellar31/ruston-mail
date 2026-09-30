//! Attachments API: binary download + multipart upload.

use crate::error::{Error, Result};
use crate::transport::{Doer, Request};
use serde::Deserialize;
use std::time::Duration;

const BOUNDARY: &str = "----protoncliBOUNDARYx7MA4YWxkTrZu0gW";
/// Maximum encrypted attachment response (128 MiB).
pub const MAX_ATTACHMENT_RESPONSE_BYTES: usize = 128 * 1024 * 1024;
/// Maximum complete multipart upload (128 MiB), a local transport safety limit.
pub const MAX_ATTACHMENT_UPLOAD_BYTES: usize = 128 * 1024 * 1024;
/// Larger transfers have a longer deadline than small JSON requests.
const ATTACHMENT_REQUEST_TIMEOUT: Duration = Duration::from_secs(180);

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

fn build_multipart(parts: &[Part]) -> Result<Vec<u8>> {
    let end = format!("--{BOUNDARY}--\r\n");
    let mut size = end.len();
    let mut headers = Vec::with_capacity(parts.len());
    for p in parts {
        let (header, len) = match p {
            Part::Text(name, value) => (
                format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n"),
                value.len(),
            ),
            Part::File(name, data) => (
                format!(
                    "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"blob\"\r\nContent-Type: application/octet-stream\r\n\r\n"
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
    Ok(out)
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
    let body = build_multipart(&[
        Part::Text("Filename", filename),
        Part::Text("MessageID", message_id),
        Part::Text("ContentID", content_id),
        Part::Text("MIMEType", mime_type),
        Part::File("KeyPackets", key_packets),
        Part::File("DataPacket", data_packet),
        Part::File("Signature", signature),
    ])?;
    let content_type = format!("multipart/form-data; boundary={BOUNDARY}");
    Ok(Request::post("/mail/v4/attachments")
        .raw(body, content_type)
        .timeout(ATTACHMENT_REQUEST_TIMEOUT))
}

pub(crate) async fn upload_prepared<D: Doer>(d: &D, request: Request) -> Result<String> {
    let r: UploadResp = d.decode(request).await?;
    Ok(r.attachment.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::Response;
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
        let body = build_multipart(&[
            Part::Text("Filename", "f.txt"),
            Part::File("DataPacket", b"\x00\x01"),
        ])
        .unwrap();
        let s = String::from_utf8_lossy(&body);
        assert!(s.contains("name=\"Filename\""));
        assert!(s.contains("f.txt"));
        assert!(s.contains("name=\"DataPacket\""));
        assert!(s.contains("application/octet-stream"));
        assert!(s.trim_end().ends_with(&format!("--{BOUNDARY}--")));
        assert_eq!(body.capacity(), body.len());
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
