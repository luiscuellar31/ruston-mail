use std::borrow::Cow;
use std::path::PathBuf;

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

/// Whether mail is new or answers an existing message.
/// Proton supplies reply recipients, subjects and forwarded attachments.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    New,
    Reply {
        message_id: String,
        /// Everyone the message reached, rather than only whoever sent it.
        everyone: bool,
    },
    Forward {
        message_id: String,
    },
}

/// A message ready to leave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outgoing {
    pub kind: Kind,
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

    /// Whether the sender has to say where this goes. A reply already knows,
    /// from the message it answers.
    pub fn needs_recipients(kind: &Kind) -> bool {
        !matches!(kind, Kind::Reply { .. })
    }
}

/// What a recipient field came to: the addresses to send to, and the pieces
/// that do not look like an address, kept so the field can point at them.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Recipients {
    pub accepted: Vec<String>,
    pub rejected: Vec<String>,
}

/// Reads comma- or semicolon-separated recipients and removes duplicates.
pub fn recipients(field: &str) -> Recipients {
    let mut result = Recipients::default();
    for piece in field
        .split([',', ';'])
        .map(str::trim)
        .filter(|piece| !piece.is_empty())
    {
        if !is_address(piece) {
            if !result.rejected.iter().any(|other| other == piece) {
                result.rejected.push(piece.to_owned());
            }
            continue;
        }
        let already = result
            .accepted
            .iter()
            .any(|other| other.eq_ignore_ascii_case(piece));
        if !already {
            result.accepted.push(piece.to_owned());
        }
    }

    result
}

/// Rejects obvious mistakes without reimplementing the full address grammar.
fn is_address(piece: &str) -> bool {
    if piece.chars().any(char::is_whitespace) {
        return false;
    }
    let Some((local, domain)) = piece.split_once('@') else {
        return false;
    };

    !local.is_empty()
        && !domain.contains('@')
        && domain.starts_with(|c: char| c.is_ascii_alphanumeric())
        && domain.ends_with(|c: char| c.is_ascii_alphanumeric())
        && domain.contains('.')
        && !domain.contains("..")
}

/// Escapes typed text, then preserves paragraphs and line breaks as HTML.
fn as_html(body: &str) -> String {
    let paragraphs: Vec<String> = body
        .replace("\r\n", "\n")
        .split("\n\n")
        .map(|paragraph| paragraph.trim_matches('\n'))
        .filter(|paragraph| !paragraph.trim().is_empty())
        .map(|paragraph| {
            let lines: Vec<String> = paragraph.split('\n').map(escape).collect();
            format!("<p>{}</p>", lines.join("<br>"))
        })
        .collect();

    paragraphs.join("\n")
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(character),
        }
    }

    out
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
}

impl SendError {
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
            Self::Mailbox(MailboxError::Service | MailboxError::Unavailable) => {
                Cow::Borrowed("Proton would not accept the message. It was not sent.")
            }
            Self::Unconfirmed => Cow::Borrowed(
                "Could not confirm whether Proton sent this message. Check Sent before trying again.",
            ),
            Self::MissingAttachment(name) => {
                Cow::Owned(format!("Attachment cannot be found: {name}"))
            }
        }
    }
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message())
    }
}

impl std::error::Error for SendError {}

/// Verifies that all attachments exist and can be read.
///
/// Running this off the UI thread and off Tokio asynchronous worker threads
/// prevents slow or hanging filesystems (e.g. NFS/SMB mounts) from stalling
/// the application.
pub async fn validate_attachments(attachments: &[PathBuf]) -> Result<(), SendError> {
    if attachments.is_empty() {
        return Ok(());
    }

    let paths = attachments.to_vec();
    let check = move || {
        for path in &paths {
            if !path.is_file() || std::fs::File::open(path).is_err() {
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("Attachment");
                return Err(SendError::MissingAttachment(name.to_owned()));
            }
        }
        Ok(())
    };

    if tokio::runtime::Handle::try_current().is_ok() {
        tokio::task::spawn_blocking(check)
            .await
            .unwrap_or(Err(SendError::Mailbox(MailboxError::Service)))
    } else {
        check()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_field_can_name_several_people() {
        let read = recipients("alex@example.com, sam@example.org; jo@example.net");

        assert_eq!(
            read.accepted,
            ["alex@example.com", "sam@example.org", "jo@example.net"]
        );
        assert!(read.rejected.is_empty());
    }

    #[test]
    fn the_same_person_is_only_written_to_once() {
        // However it was typed: a duplicate is a slip, not an instruction.
        let read = recipients("alex@example.com, Alex@Example.com , alex@example.com");

        assert_eq!(read.accepted, ["alex@example.com"]);
    }

    #[test]
    fn what_is_not_an_address_is_kept_to_be_pointed_at() {
        // Dropping it silently would send the mail to fewer people than the
        // sender believes, which is worse than refusing.
        let read = recipients("alex@example.com, not-an-address, @nowhere, a@b");

        assert_eq!(read.accepted, ["alex@example.com"]);
        assert_eq!(read.rejected, ["not-an-address", "@nowhere", "a@b"]);
    }

    #[test]
    fn an_empty_field_names_nobody() {
        assert_eq!(recipients(""), Recipients::default());
        assert_eq!(recipients("  ,  ; "), Recipients::default());
    }

    #[test]
    fn ordinary_addresses_are_not_turned_away() {
        for address in [
            "a@b.co",
            "first.last@example.co.uk",
            "user+tag@example.com",
            "user_name@sub.example.org",
            "1@2.com",
        ] {
            assert_eq!(
                recipients(address).accepted,
                [address],
                "{address} was refused"
            );
        }
    }

    fn html(body: &str) -> String {
        Outgoing {
            kind: Kind::New,
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
            kind: Kind::New,
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
