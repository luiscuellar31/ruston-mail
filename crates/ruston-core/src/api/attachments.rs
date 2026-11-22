//! Attachments: downloading their data, still encrypted, and uploading new
//! ones as multipart forms.

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

/// What ends each part of a multipart body, counted where its size is and
/// written after it.
const PART_END: &[u8] = b"\r\n";

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
            .and_then(|n| n.checked_add(PART_END.len()))
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
        out.extend_from_slice(PART_END);
    }
    out.extend_from_slice(end.as_bytes());
    Ok(Multipart {
        body: out,
        boundary,
    })
}

/// The data packet of the attachment `id`, as encrypted as Proton keeps it.
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

/// One attachment as it is uploaded: what Proton is told of it and its
/// three encrypted parts, each by name, so that two of the same type cannot
/// trade places unseen.
pub(crate) struct AttachmentUpload<'a> {
    pub(crate) filename: &'a str,
    pub(crate) message_id: &'a str,
    pub(crate) content_id: &'a str,
    pub(crate) mime_type: &'a str,
    pub(crate) key_packets: &'a [u8],
    pub(crate) data_packet: &'a [u8],
    pub(crate) signature: &'a [u8],
}

/// Builds the replayable upload on the send pipeline's blocking worker.
pub(crate) fn upload_request(upload: &AttachmentUpload) -> Result<Request> {
    let multipart = build_multipart(&[
        Part::Text("Filename", upload.filename),
        Part::Text("MessageID", upload.message_id),
        Part::Text("ContentID", upload.content_id),
        Part::Text("MIMEType", upload.mime_type),
        Part::File("KeyPackets", upload.key_packets),
        Part::File("DataPacket", upload.data_packet),
        Part::File("Signature", upload.signature),
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

    /// An upload of a file named `filename`, its other parts made up.
    fn upload_named(filename: &str) -> Result<Request> {
        upload_request(&AttachmentUpload {
            filename,
            message_id: "m",
            content_id: "",
            mime_type: "text/plain",
            key_packets: b"k",
            data_packet: b"d",
            signature: b"s",
        })
    }

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
        let request = upload_request(&AttachmentUpload {
            filename: "file.txt",
            message_id: "message",
            content_id: "content",
            mime_type: "text/plain",
            key_packets: b"keys",
            data_packet: b"data",
            signature: b"signature",
        })
        .unwrap();
        let id = upload_prepared(&CheckAttachmentLimit, request)
            .await
            .unwrap();
        assert_eq!(id, "uploaded");
    }

    #[test]
    fn a_multipart_body_holds_each_part_under_its_name_then_its_end() {
        let Multipart { body, boundary } = build_multipart(&[
            Part::Text("Filename", "f.txt"),
            Part::File("DataPacket", b"\x00\x01"),
        ])
        .unwrap();
        let mut expected = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"Filename\"\r\n\r\nf.txt\r\n\
             --{boundary}\r\nContent-Disposition: form-data; name=\"DataPacket\"; \
             filename=\"blob\"\r\nContent-Type: application/octet-stream\r\n\r\n"
        )
        .into_bytes();
        expected.extend_from_slice(b"\x00\x01\r\n");
        expected.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        assert_eq!(body, expected);
        // The body was reserved at its final size, so it never grew.
        assert_eq!(body.capacity(), body.len());
    }

    #[test]
    fn every_upload_has_a_boundary_of_its_own() {
        let upload = || upload_named("f.txt").unwrap();
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
            let refused = upload_named(name);
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
