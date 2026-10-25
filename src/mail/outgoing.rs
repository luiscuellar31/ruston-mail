use std::borrow::Cow;
use std::collections::HashSet;
use std::path::PathBuf;

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
    },
    Forward {
        message_id: String,
    },
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

/// Which domains a recipient field takes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Domains {
    /// Only domains written in ASCII. A domain spelled with letters from
    /// another script can pass for a familiar one, as `pаypal.com` with a
    /// Cyrillic "а" does; typed in its `xn--` form, the difference shows.
    #[default]
    Ascii,
    /// Domains in other scripts as well, each sent in its ASCII `xn--` form
    /// and listed beside it so a look-alike still shows.
    International,
}

/// A domain typed in another script, and the ASCII form it is sent in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InternationalDomain {
    pub typed: String,
    pub ascii: String,
}

/// What a recipient field came to: the addresses to send to, and the pieces
/// that do not look like an address, kept so the field can point at them.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Recipients {
    pub accepted: Vec<String>,
    pub rejected: Vec<String>,
    /// Domains typed in another script, once each. The addresses they are
    /// part of are accepted in ASCII form.
    pub international: Vec<InternationalDomain>,
}

/// Reads comma- or semicolon-separated recipients and removes duplicates.
pub fn recipients(field: &str, domains: Domains) -> Recipients {
    let mut result = Recipients::default();
    // What was already seen, so a long field is read in one pass.
    let mut refused = HashSet::new();
    let mut accepted = HashSet::new();
    for piece in field
        .split([',', ';'])
        .map(str::trim)
        .filter(|piece| !piece.is_empty())
    {
        let Some(address) = address(piece, domains) else {
            if refused.insert(piece) {
                result.rejected.push(piece.to_owned());
            }
            continue;
        };
        if let Some(domain) = address.international
            && !result.international.contains(&domain)
        {
            result.international.push(domain);
        }
        // The same address in another case is the same person.
        if accepted.insert(address.sent.to_ascii_lowercase()) {
            result.accepted.push(address.sent);
        }
    }

    result
}

/// An address as it leaves: as typed, except for a domain in another script.
struct Address {
    sent: String,
    international: Option<InternationalDomain>,
}

/// Rejects obvious mistakes without reimplementing the full address grammar.
fn address(piece: &str, domains: Domains) -> Option<Address> {
    if piece.chars().any(char::is_whitespace) {
        return None;
    }
    let (local, domain) = piece.split_once('@')?;
    if local.is_empty() || domain.contains('@') {
        return None;
    }
    if domain.is_ascii() {
        return is_domain(domain).then(|| Address {
            sent: piece.to_owned(),
            international: None,
        });
    }
    if domains == Domains::Ascii {
        return None;
    }
    let ascii = to_ascii(domain).filter(|ascii| is_domain(ascii))?;

    Some(Address {
        sent: format!("{local}@{ascii}"),
        international: Some(InternationalDomain {
            typed: domain.to_owned(),
            ascii,
        }),
    })
}

/// The ASCII form IDNA gives a domain in another script: `münchen.de` is sent
/// as `xn--mnchen-3ya.de`.
fn to_ascii(domain: &str) -> Option<String> {
    // The host parser decodes percent escapes before IDNA sees the domain,
    // which would send something other than what was typed.
    if domain.contains('%') {
        return None;
    }
    match url::Host::parse(domain).ok()? {
        url::Host::Domain(ascii) => Some(ascii),
        // Digits from another script can spell an IP address, not a domain.
        url::Host::Ipv4(_) | url::Host::Ipv6(_) => None,
    }
}

fn is_domain(domain: &str) -> bool {
    domain.starts_with(|c: char| c.is_ascii_alphanumeric())
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

    /// "pаypal.com" with a Cyrillic "а": it looks like PayPal's domain.
    const LOOK_ALIKE: &str = "p\u{430}ypal.com";
    /// "почта.рф", a real international domain.
    const CYRILLIC: &str = "\u{43f}\u{43e}\u{447}\u{442}\u{430}.\u{440}\u{444}";

    #[test]
    fn a_field_can_name_several_people() {
        let read = recipients(
            "alex@example.com, sam@example.org; jo@example.net",
            Domains::Ascii,
        );

        assert_eq!(
            read.accepted,
            ["alex@example.com", "sam@example.org", "jo@example.net"]
        );
        assert!(read.rejected.is_empty());
    }

    #[test]
    fn the_same_person_is_only_written_to_once() {
        // However it was typed: a duplicate is a slip, not an instruction.
        let read = recipients(
            "alex@example.com, Alex@Example.com , alex@example.com",
            Domains::Ascii,
        );

        assert_eq!(read.accepted, ["alex@example.com"]);
    }

    #[test]
    fn what_is_not_an_address_is_kept_to_be_pointed_at() {
        // Dropping it silently would send the mail to fewer people than the
        // sender believes, which is worse than refusing.
        let read = recipients(
            "alex@example.com, not-an-address, @nowhere, a@b",
            Domains::Ascii,
        );

        assert_eq!(read.accepted, ["alex@example.com"]);
        assert_eq!(read.rejected, ["not-an-address", "@nowhere", "a@b"]);
    }

    #[test]
    fn look_alike_domains_from_other_scripts_are_refused() {
        let read = recipients(
            &format!("ceo@{LOOK_ALIKE}, ivan@{CYRILLIC}"),
            Domains::Ascii,
        );
        assert!(read.accepted.is_empty());
        assert_eq!(read.rejected.len(), 2);

        // The same international domain typed in its xn-- form is accepted,
        // and so is a local part with accents.
        let read = recipients(
            "ivan@xn--80a1acny.xn--p1ai, jos\u{e9}@example.com",
            Domains::Ascii,
        );
        assert!(read.rejected.is_empty());
        assert_eq!(read.accepted.len(), 2);
        assert!(read.international.is_empty());
    }

    #[test]
    fn international_domains_leave_in_ascii_beside_what_was_typed() {
        let read = recipients(
            &format!("ana@M\u{fc}nchen.de, ceo@{LOOK_ALIKE}; ivan@{CYRILLIC}"),
            Domains::International,
        );

        assert!(read.rejected.is_empty());
        assert_eq!(
            read.accepted,
            [
                "ana@xn--mnchen-3ya.de",
                "ceo@xn--pypal-4ve.com",
                "ivan@xn--80a1acny.xn--p1ai",
            ]
        );
        // The look-alike is named next to a form that is plainly not paypal.com.
        let pairs: Vec<(&str, &str)> = read
            .international
            .iter()
            .map(|domain| (domain.typed.as_str(), domain.ascii.as_str()))
            .collect();
        assert_eq!(
            pairs,
            [
                ("M\u{fc}nchen.de", "xn--mnchen-3ya.de"),
                (LOOK_ALIKE, "xn--pypal-4ve.com"),
                (CYRILLIC, "xn--80a1acny.xn--p1ai"),
            ]
        );
    }

    #[test]
    fn an_international_domain_and_its_ascii_form_are_one_recipient() {
        let read = recipients(
            &format!("ivan@{CYRILLIC}, ivan@xn--80a1acny.xn--p1ai, ivan@{CYRILLIC}"),
            Domains::International,
        );

        assert_eq!(read.accepted, ["ivan@xn--80a1acny.xn--p1ai"]);
        assert_eq!(read.international.len(), 1);
        // ASCII domains are sent exactly as typed.
        let read = recipients("Alex@Example.com", Domains::International);
        assert_eq!(read.accepted, ["Alex@Example.com"]);
        assert!(read.international.is_empty());
    }

    #[test]
    fn a_domain_that_does_not_convert_cleanly_is_refused() {
        for piece in [
            // Percent escapes would be decoded into something not typed.
            "a@m\u{fc}nchen%2Ede",
            // Full-width digits spell an IP address, not a domain.
            "a@\u{ff11}\u{ff12}\u{ff17}.\u{ff10}.\u{ff10}.\u{ff11}",
            // A full-width "@" turns into a second one.
            "a@m\u{fc}nchen\u{ff20}example.de",
            "a@m\u{fc}nchen",
            "a@m\u{fc}nchen..de",
            "a@.m\u{fc}nchen.de",
            "a@m\u{fc}nchen.de.",
            "a@m\u{fc}n chen.de",
        ] {
            let read = recipients(piece, Domains::International);
            assert!(read.accepted.is_empty(), "{piece} was accepted");
            assert!(read.international.is_empty(), "{piece} was converted");
        }
    }

    #[test]
    fn an_empty_field_names_nobody() {
        for domains in [Domains::Ascii, Domains::International] {
            assert_eq!(recipients("", domains), Recipients::default());
            assert_eq!(recipients("  ,  ; ", domains), Recipients::default());
        }
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
            for domains in [Domains::Ascii, Domains::International] {
                assert_eq!(
                    recipients(address, domains).accepted,
                    [address],
                    "{address} was refused"
                );
            }
        }
    }

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
