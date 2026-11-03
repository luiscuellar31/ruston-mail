use std::borrow::Cow;
use std::path::PathBuf;

use ruston_core::Quote;
use ruston_core::html::escape_text;

use super::MailboxError;

/// How the body is sent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BodyFormat {
    /// Sent as it was typed.
    #[default]
    PlainText,
    /// Sent as HTML built from what was typed. What you type is still text:
    /// a tag you write is shown as the characters you wrote, not obeyed.
    Html,
}

/// Whether mail is new or answers an existing message. Core supplies the
/// subjects and forwarded attachments; the recipients are the ones typed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum OutgoingKind {
    #[default]
    New,
    Reply {
        message_id: String,
        /// Everyone the message reached, rather than only whoever sent it.
        everyone: bool,
        /// Whether the original goes under the text.
        quote: Quote,
    },
    Forward {
        message_id: String,
        /// Whether the original's text goes under the text; its files go
        /// either way.
        quote: Quote,
        /// The original's attachments, by ID, that do not go with it.
        leave_out: Vec<String>,
    },
}

impl OutgoingKind {
    /// Whether the original goes under the text; a new message answers
    /// nothing, so it says nothing either way.
    pub fn quote(&self) -> Quote {
        match self {
            Self::New => Quote::default(),
            Self::Reply { quote, .. } | Self::Forward { quote, .. } => *quote,
        }
    }

    /// The original's attachments, by ID, that a forward leaves out.
    pub fn leave_out(&self) -> &[String] {
        match self {
            Self::Forward { leave_out, .. } => leave_out,
            Self::New | Self::Reply { .. } => &[],
        }
    }
}

/// How Proton will protect a new message for one recipient.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protection {
    /// A Proton address: end-to-end encrypted.
    EndToEnd,
    /// Another provider's address with a published PGP key.
    Pgp,
    /// No key to encrypt to: delivered without end-to-end encryption.
    Unencrypted,
}

/// One recipient's protection, or why it could not be looked up.
pub type ProtectionCheck = (String, Result<Protection, MailboxError>);

/// A message ready to leave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outgoing {
    pub kind: OutgoingKind,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub subject: String,
    pub body: String,
    pub format: BodyFormat,
    pub attachments: Vec<PathBuf>,
}

impl Outgoing {
    /// The body as it goes on the wire.
    pub fn wire_body(&self) -> String {
        match self.format {
            BodyFormat::PlainText => self.body.clone(),
            BodyFormat::Html => as_html(&self.body),
        }
    }

    pub fn is_html(&self) -> bool {
        self.format == BodyFormat::Html
    }
}

/// Escapes typed text, then preserves paragraphs and line breaks as HTML.
fn as_html(body: &str) -> String {
    let paragraphs: Vec<String> = body
        .replace("\r\n", "\n")
        .split("\n\n")
        .map(|paragraph| paragraph.trim_matches('\n'))
        .filter(|paragraph| !paragraph.trim().is_empty())
        .map(|paragraph| {
            let lines: Vec<String> = paragraph.split('\n').map(escape_text).collect();
            format!("<p>{}</p>", lines.join("<br>"))
        })
        .collect();

    paragraphs.join("\n")
}

/// What can stop a message from leaving.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendError {
    /// The fictional mailbox takes only a few messages per run.
    DemoLimitReached,
    Mailbox(MailboxError),
    /// Proton may have accepted the send, but the client could not confirm it.
    Unconfirmed,
    /// An attached file could not be found or read on disk.
    MissingAttachment(String),
    /// A file exceeds core's local outgoing attachment size limit.
    AttachmentTooLarge {
        limit: usize,
    },
    /// The newly attached files exceed core's combined plaintext limit.
    AttachmentTotalTooLarge {
        limit: usize,
    },
}

impl SendError {
    pub(super) fn from_attachment(error: &ruston_core::Error) -> Option<Self> {
        match error {
            ruston_core::Error::AttachmentUnavailable { path, .. } => {
                Some(Self::MissingAttachment(
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("Attachment")
                        .to_owned(),
                ))
            }
            ruston_core::Error::AttachmentTooLarge { limit } => {
                Some(Self::AttachmentTooLarge { limit: *limit })
            }
            ruston_core::Error::AttachmentBatchTooLarge { limit } => {
                Some(Self::AttachmentTotalTooLarge { limit: *limit })
            }
            _ => None,
        }
    }

    pub fn message(&self) -> Cow<'static, str> {
        match self {
            Self::DemoLimitReached => Cow::Borrowed(
                "The demo mailbox takes only a few messages per run. Restart it to send more.",
            ),
            Self::Mailbox(MailboxError::Connection) => {
                Cow::Borrowed("Ruston Mail could not reach Proton. The message was not sent.")
            }
            Self::Mailbox(MailboxError::SessionExpired) => Cow::Borrowed(
                "Your Proton session has expired. Sign in again; the message was not sent.",
            ),
            Self::Mailbox(MailboxError::SessionUnavailable) => Cow::Borrowed(
                "Ruston Mail could not access the saved Proton session. Unlock your password \
                 storage and try again; the message was not sent.",
            ),
            Self::Mailbox(MailboxError::Service | MailboxError::Unavailable) => {
                Cow::Borrowed("Proton would not accept the message. It was not sent.")
            }
            Self::Mailbox(MailboxError::AttachmentVerificationFailed) => Cow::Borrowed(
                "An attachment signature could not be verified. The message was not sent.",
            ),
            Self::Unconfirmed => Cow::Borrowed(
                "Could not confirm whether Proton sent this message. Check Sent before trying again.",
            ),
            Self::MissingAttachment(name) => {
                Cow::Owned(format!("Attachment cannot be found: {name}"))
            }
            Self::AttachmentTooLarge { limit } => Cow::Owned(format!(
                "Each attachment must be at most {} MiB. The message was not sent.",
                limit / (1024 * 1024)
            )),
            Self::AttachmentTotalTooLarge { limit } => Cow::Owned(format!(
                "Attachments must total at most {} MiB. The message was not sent.",
                limit / (1024 * 1024)
            )),
        }
    }
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message())
    }
}

impl std::error::Error for SendError {}

/// Uses core's shared readability and size checks for outgoing attachments.
///
/// Running this off the UI thread and off Tokio asynchronous worker threads
/// prevents slow or hanging filesystems (e.g. NFS/SMB mounts) from stalling
/// the application.
pub async fn validate_attachments(attachments: &[PathBuf]) -> Result<(), SendError> {
    ruston_core::mail::send::validate_attachments(attachments)
        .await
        .map_err(|error| {
            SendError::from_attachment(&error).unwrap_or(SendError::Mailbox(MailboxError::Service))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn html(body: &str) -> String {
        Outgoing {
            kind: OutgoingKind::New,
            to: Vec::new(),
            cc: Vec::new(),
            bcc: Vec::new(),
            subject: String::new(),
            body: body.to_owned(),
            format: BodyFormat::Html,
            attachments: Vec::new(),
        }
        .wire_body()
    }

    #[test]
    fn plain_text_leaves_exactly_as_it_was_typed() {
        let typed = "Hi Alex,\n\nThe < sign & the rest.\n";
        let message = Outgoing {
            kind: OutgoingKind::New,
            to: Vec::new(),
            cc: Vec::new(),
            bcc: Vec::new(),
            subject: String::new(),
            body: typed.to_owned(),
            format: BodyFormat::PlainText,
            attachments: Vec::new(),
        };

        assert_eq!(message.wire_body(), typed);
        assert!(!message.is_html());
    }

    #[test]
    fn html_keeps_the_shape_of_what_was_typed() {
        assert_eq!(
            html("Hi Alex,\nHow are you?\n\nSee you Thursday."),
            "<p>Hi Alex,<br>How are you?</p>\n<p>See you Thursday.</p>"
        );
        // Blank lines around and between do not become empty paragraphs.
        assert_eq!(html("\n\nOnly this.\n\n\n"), "<p>Only this.</p>");
        assert_eq!(html("   "), "");
    }

    #[test]
    fn a_tag_that_was_typed_is_shown_not_obeyed() {
        // Nothing anyone types may become markup in the recipient's client.
        assert_eq!(
            html("2 < 3 & \"quoted\" <b>not bold</b>"),
            "<p>2 &lt; 3 &amp; &quot;quoted&quot; &lt;b&gt;not bold&lt;/b&gt;</p>"
        );
        assert!(!html("<script>alert(1)</script>").contains("<script"));
    }

    #[tokio::test]
    async fn validate_attachments_accepts_existing_file() {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        assert!(validate_attachments(&[manifest]).await.is_ok());
    }

    #[test]
    fn attachment_preflight_keeps_synchronous_executor_support() {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        assert!(futures::executor::block_on(validate_attachments(&[manifest])).is_ok());
    }

    #[tokio::test]
    async fn outgoing_size_errors_show_the_local_limits() {
        use ruston_core::mail::send::{
            MAX_OUTGOING_ATTACHMENT_BYTES, MAX_OUTGOING_ATTACHMENT_TOTAL_BYTES,
        };
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "ruston-outgoing-limit-{}-{unique}",
            std::process::id()
        ));
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(MAX_OUTGOING_ATTACHMENT_BYTES as u64 + 1)
            .unwrap();
        let error = validate_attachments(std::slice::from_ref(&path))
            .await
            .unwrap_err();
        assert_eq!(
            error,
            SendError::AttachmentTooLarge {
                limit: MAX_OUTGOING_ATTACHMENT_BYTES
            }
        );
        assert!(error.message().contains("32 MiB"));
        file.set_len(MAX_OUTGOING_ATTACHMENT_BYTES as u64).unwrap();
        let error = validate_attachments(&vec![path.clone(); 5])
            .await
            .unwrap_err();
        assert_eq!(
            error,
            SendError::AttachmentTotalTooLarge {
                limit: MAX_OUTGOING_ATTACHMENT_TOTAL_BYTES
            }
        );
        assert!(error.message().contains("128 MiB"));
        drop(file);
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn validate_attachments_rejects_missing_file() {
        let missing = PathBuf::from("/tmp/this_file_does_not_exist_ruston_test.xyz");
        let result = validate_attachments(&[missing]).await;
        assert_eq!(
            result,
            Err(SendError::MissingAttachment(
                "this_file_does_not_exist_ruston_test.xyz".to_owned()
            ))
        );
        assert!(
            result
                .unwrap_err()
                .message()
                .contains("this_file_does_not_exist_ruston_test.xyz")
        );
    }

    #[test]
    fn send_error_messages_match_expectation() {
        assert_eq!(
            SendError::MissingAttachment("photo.jpg".into()).message(),
            "Attachment cannot be found: photo.jpg"
        );
        assert!(SendError::DemoLimitReached.message().contains("Restart"));
        assert!(SendError::Unconfirmed.message().contains("Check Sent"));
        assert!(!SendError::Unconfirmed.message().contains("was not sent"));
    }
}
