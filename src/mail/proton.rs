use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use futures::SinkExt;
use futures::channel::{mpsc, oneshot};
use proton_core::model::enums::label_ids;
use proton_core::{
    Client, Conversation, Error, HvChallenge, HvResolver, LabelCount, LoginOptions,
    MessageMetadata, Recipient, TotpPrompt,
};

use super::{
    AuthError, ConversationDetail, ConversationPage, ConversationSummary, LoginRequest,
    MailAddress, MailFolder, MailMessage, MailboxCounts, MailboxError, MessageBody, html,
};

/// Conversations requested per page from Proton.
pub const PAGE_SIZE: u32 = 50;

const PROFILE: &str = "ruston";
const SIGN_IN_CANCELLED: &str = "sign-in cancelled";
/// Proton's standalone human-verification page.
const VERIFY_ORIGIN: &str = "https://verify.proton.me";
const MAILBOX_PASSWORD_REQUIRED: &str = "this account uses a separate mailbox password";
const SECURITY_KEY_REQUIRED: &str = "FIDO2/WebAuthn";
const USER_KEY_UNLOCK_FAILED: &str = "no user key could be unlocked";
const MAX_LISTED_CORRESPONDENTS: usize = 3;

#[derive(Clone)]
pub enum ResumeOutcome {
    Authenticated(Arc<ProtonMailService>),
    SignedOut,
    Failed(AuthError),
}

#[derive(Clone)]
pub enum SignInOutcome {
    Authenticated(Arc<ProtonMailService>),
    NeedsMailboxPassword,
    /// The user abandoned a prompt.
    Cancelled,
    Failed(AuthError),
}

/// Something the user must provide while a sign-in is still running.
#[derive(Clone)]
pub enum SignInPrompt {
    /// Enter the current TOTP code.
    Totp(Reply<String>),
    /// Complete Proton's check at `url` in a browser, then confirm.
    HumanVerification { url: String, done: Reply<()> },
}

impl SignInPrompt {
    /// Abandons the prompt; the waiting sign-in ends as cancelled.
    pub fn cancel(self) {
        match self {
            Self::Totp(reply) => reply.cancel(),
            Self::HumanVerification { done, .. } => done.cancel(),
        }
    }
}

#[derive(Clone)]
pub enum SignInEvent {
    Prompt(SignInPrompt),
    Finished(SignInOutcome),
}

/// A one-time answer to a sign-in prompt. Clones share one answer slot, so it
/// can travel inside UI messages; only the first answer is delivered.
#[derive(Clone)]
pub struct Reply<T>(Arc<Mutex<Option<oneshot::Sender<T>>>>);

impl<T> Reply<T> {
    pub(crate) fn channel() -> (Self, oneshot::Receiver<T>) {
        let (sender, receiver) = oneshot::channel();
        (Self(Arc::new(Mutex::new(Some(sender)))), receiver)
    }

    pub fn send(&self, value: T) {
        if let Some(sender) = self.take() {
            let _ = sender.send(value);
        }
    }

    pub fn cancel(&self) {
        drop(self.take());
    }

    fn take(&self) -> Option<oneshot::Sender<T>> {
        self.0.lock().ok()?.take()
    }
}

pub struct ProtonMailService {
    client: Client,
    email: Option<String>,
}

impl ProtonMailService {
    pub async fn resume() -> ResumeOutcome {
        match Client::resume(PROFILE).await {
            Ok(client) => ResumeOutcome::Authenticated(Arc::new(Self::from_client(client))),
            Err(Error::Unauthorized | Error::Crypto(_) | Error::Srp(_)) => ResumeOutcome::SignedOut,
            Err(Error::Api(error)) if matches!(error.http_status, 401 | 403) => {
                ResumeOutcome::SignedOut
            }
            Err(error) => ResumeOutcome::Failed(map_error(error)),
        }
    }

    /// Signs in with a single login attempt. When Proton asks for a TOTP code
    /// or human verification, a prompt goes out through `events` and the login
    /// waits for its answer. Always ends by sending `SignInEvent::Finished`.
    pub async fn sign_in(request: LoginRequest, mut events: mpsc::Sender<SignInEvent>) {
        let has_totp = request.totp.is_some();
        let has_mailbox_password = request.mailbox_password.is_some();
        let prompted_totp = Arc::new(AtomicBool::new(false));
        let options = LoginOptions {
            username: request.username,
            password: request.password,
            totp: request.totp,
            mailbox_password: request.mailbox_password,
            profile: PROFILE.to_owned(),
            base_url: None,
            app_version: None,
            user_agent: Some(user_agent()),
            hv: Some(human_verification(events.clone())),
        };
        let totp_prompt = totp_prompt(events.clone(), prompted_totp.clone());

        let outcome = match Client::login_with_totp_prompt(options, totp_prompt).await {
            Ok(client) => SignInOutcome::Authenticated(Arc::new(Self::from_client(client))),
            Err(error) => {
                let has_totp = has_totp || prompted_totp.load(Ordering::Relaxed);
                map_sign_in_error(error, has_totp, has_mailbox_password)
            }
        };
        let _ = events.send(SignInEvent::Finished(outcome)).await;
        // The client keeps the verification resolver; closing the channel ends
        // the sign-in stream and makes any later prompt fail as cancelled.
        events.close_channel();
    }

    pub async fn logout(&self) -> Result<(), AuthError> {
        self.client.logout().await.map_err(map_error)
    }

    pub fn email(&self) -> Option<&str> {
        self.email.as_deref()
    }

    /// Lists one zero-based page of a folder's conversations, newest first.
    pub async fn list_conversations(
        &self,
        folder: MailFolder,
        page: u32,
        page_size: u32,
    ) -> Result<ConversationPage, MailboxError> {
        let (total, conversations) = self
            .client
            .list_conversations(label_id(folder), page, page_size, false)
            .await
            .map_err(map_mailbox_error)?;

        Ok(ConversationPage {
            conversations: conversations
                .into_iter()
                .map(|conversation| summarize(conversation, folder))
                .collect(),
            total,
        })
    }

    pub async fn conversation_counts(&self) -> Result<MailboxCounts, MailboxError> {
        let counts = self
            .client
            .conversation_counts()
            .await
            .map_err(map_mailbox_error)?;

        Ok(map_counts(&counts))
    }

    /// Fetches and decrypts a conversation for the reader, oldest message
    /// first. proton-core already sanitizes HTML bodies; they are shown as
    /// plain text until Ruston renders HTML.
    pub async fn conversation_detail(&self, id: &str) -> Result<ConversationDetail, MailboxError> {
        let (conversation, messages) = self
            .client
            .read_conversation(id)
            .await
            .map_err(map_mailbox_error)?;
        let subject = conversation.subject.trim();

        Ok(ConversationDetail {
            id: id.to_owned(),
            subject: (!subject.is_empty()).then(|| subject.to_owned()),
            messages: messages
                .into_iter()
                .map(|message| mail_message(message.meta, message.body, &message.mime_type))
                .collect(),
        })
    }

    fn from_client(client: Client) -> Self {
        let email = client.primary_email().map(str::to_owned);

        Self { client, email }
    }
}

/// An honest client identity, like protonmail-cli's.
fn user_agent() -> String {
    format!(
        "ruston/{} ({})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS
    )
}

/// Asks for the TOTP code only when Proton requires it, after any human
/// verification, so the code is still fresh.
fn totp_prompt(events: mpsc::Sender<SignInEvent>, prompted: Arc<AtomicBool>) -> TotpPrompt {
    Arc::new(move || {
        let events = events.clone();
        let prompted = prompted.clone();
        Box::pin(async move {
            let code = ask(events, SignInPrompt::Totp).await?;
            prompted.store(true, Ordering::Relaxed);
            Ok(code)
        })
    })
}

/// Proton Bridge's external-browser flow: the verification page attaches the
/// solved captcha to the challenge, so the retry echoes the challenge token.
fn human_verification(events: mpsc::Sender<SignInEvent>) -> HvResolver {
    Arc::new(move |challenge: HvChallenge| {
        let events = events.clone();
        Box::pin(async move {
            let captcha = challenge.methods.is_empty()
                || challenge.methods.iter().any(|method| method == "captcha");
            if !captcha {
                return Err(Error::HumanVerification(challenge));
            }
            let url = verification_url(&challenge.web_url, &challenge.token);
            ask(events, |done| SignInPrompt::HumanVerification { url, done }).await?;
            Ok((challenge.token, "captcha".to_owned()))
        })
    })
}

/// Sends a prompt to the UI and waits for the answer.
async fn ask<T: Send + 'static>(
    mut events: mpsc::Sender<SignInEvent>,
    prompt: impl FnOnce(Reply<T>) -> SignInPrompt,
) -> proton_core::Result<T> {
    let (reply, answer) = Reply::channel();
    events
        .send(SignInEvent::Prompt(prompt(reply)))
        .await
        .map_err(|_| cancelled())?;
    answer.await.map_err(|_| cancelled())
}

fn cancelled() -> Error {
    Error::Other(SIGN_IN_CANCELLED.to_owned())
}

/// The verification page for a challenge. The server's `WebUrl` host is kept
/// only when it is an https `proton.me` host; anything else uses the default.
fn verification_url(web_url: &str, token: &str) -> String {
    let origin = web_url
        .strip_prefix("https://")
        .and_then(|rest| rest.split(['/', '?', '#']).next())
        .filter(|host| {
            host.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
                && (*host == "proton.me" || host.ends_with(".proton.me"))
        })
        .map_or_else(
            || VERIFY_ORIGIN.to_owned(),
            |host| format!("https://{host}"),
        );

    format!("{origin}/?methods=captcha&token={}", percent_encode(token))
}

fn percent_encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

fn label_id(folder: MailFolder) -> &'static str {
    match folder {
        MailFolder::Inbox => label_ids::INBOX,
        MailFolder::Drafts => label_ids::DRAFTS,
        MailFolder::Sent => label_ids::SENT,
        MailFolder::Starred => label_ids::STARRED,
        MailFolder::Archive => label_ids::ARCHIVE,
        MailFolder::Spam => label_ids::SPAM,
        MailFolder::Trash => label_ids::TRASH,
    }
}

fn summarize(conversation: Conversation, folder: MailFolder) -> ConversationSummary {
    // Per-label context describes the conversation as seen from this folder.
    let context = conversation
        .labels
        .iter()
        .find(|label| label.id == label_id(folder));
    let unread = context
        .and_then(|label| label.context_num_unread)
        .unwrap_or(conversation.num_unread);
    let time = context
        .and_then(|label| label.context_time)
        .unwrap_or(conversation.time);
    let starred = conversation
        .labels
        .iter()
        .any(|label| label.id == label_ids::STARRED);
    let correspondents = match folder {
        MailFolder::Sent | MailFolder::Drafts => &conversation.recipients,
        _ => &conversation.senders,
    };
    let participants = conversation
        .senders
        .iter()
        .chain(&conversation.recipients)
        .map(mail_address)
        .collect();
    let subject = conversation.subject.trim();

    ConversationSummary {
        subject: (!subject.is_empty()).then(|| subject.to_owned()),
        correspondents: display_names(correspondents),
        participants,
        preview: None,
        time: (time > 0).then_some(time),
        unread: unread > 0,
        starred,
        message_count: u32::try_from(conversation.num_messages).unwrap_or(0),
        id: conversation.id,
    }
}

fn mail_address(person: &Recipient) -> MailAddress {
    MailAddress {
        name: (!person.name.trim().is_empty()).then(|| person.name.trim().to_owned()),
        address: person.address.trim().to_owned(),
    }
}

fn mail_message(meta: MessageMetadata, body: String, mime_type: &str) -> MailMessage {
    let body = if mime_type.to_ascii_lowercase().starts_with("text/html") {
        html::to_plain_text(&body)
    } else {
        body
    };

    MailMessage {
        sender: mail_address(&meta.sender),
        recipients: meta
            .to_list
            .iter()
            .chain(&meta.cc_list)
            .chain(&meta.bcc_list)
            .map(mail_address)
            .collect(),
        time: (meta.time > 0).then_some(meta.time),
        body: MessageBody::PlainText(body),
        id: meta.id,
    }
}

fn display_names(people: &[Recipient]) -> Option<String> {
    let mut names = people.iter().filter_map(|person| {
        [person.name.trim(), person.address.trim()]
            .into_iter()
            .find(|value| !value.is_empty())
    });
    let listed: Vec<&str> = names.by_ref().take(MAX_LISTED_CORRESPONDENTS).collect();
    if listed.is_empty() {
        return None;
    }

    let mut display = listed.join(", ");
    if names.next().is_some() {
        display.push_str(", …");
    }
    Some(display)
}

fn map_counts(counts: &[LabelCount]) -> MailboxCounts {
    MailFolder::ALL
        .into_iter()
        .filter_map(|folder| {
            let count = counts
                .iter()
                .find(|count| count.label_id == label_id(folder))?;
            Some((folder, u32::try_from(count.unread).unwrap_or(0)))
        })
        .collect()
}

fn map_mailbox_error(error: Error) -> MailboxError {
    match error {
        Error::Unauthorized => MailboxError::SessionExpired,
        Error::Api(error) if error.http_status == 401 => MailboxError::SessionExpired,
        Error::Http(_) => MailboxError::Connection,
        Error::Api(_) | Error::HumanVerification(_) => MailboxError::Service,
        _ => MailboxError::Unavailable,
    }
}

fn map_sign_in_error(error: Error, has_totp: bool, has_mailbox_password: bool) -> SignInOutcome {
    match error {
        Error::Other(message) if message == SIGN_IN_CANCELLED => SignInOutcome::Cancelled,
        Error::Other(message) if message.contains(MAILBOX_PASSWORD_REQUIRED) => {
            SignInOutcome::NeedsMailboxPassword
        }
        Error::Other(message) if message.contains(SECURITY_KEY_REQUIRED) => {
            SignInOutcome::Failed(AuthError::SecurityKeyUnsupported)
        }
        Error::HumanVerification(_) => SignInOutcome::Failed(AuthError::HumanVerificationRequired),
        Error::Unauthorized if has_totp => SignInOutcome::Failed(AuthError::InvalidTotp),
        Error::Unauthorized => SignInOutcome::Failed(AuthError::InvalidCredentials),
        Error::Api(error) if has_totp && mentions_totp(&error.message) => {
            SignInOutcome::Failed(AuthError::InvalidTotp)
        }
        Error::Api(error)
            if matches!(error.http_status, 401 | 403) || mentions_credentials(&error.message) =>
        {
            SignInOutcome::Failed(AuthError::InvalidCredentials)
        }
        Error::Crypto(message)
            if has_mailbox_password && message.contains(USER_KEY_UNLOCK_FAILED) =>
        {
            SignInOutcome::Failed(AuthError::InvalidMailboxPassword)
        }
        error => SignInOutcome::Failed(map_error(error)),
    }
}

fn map_error(error: Error) -> AuthError {
    match error {
        Error::HumanVerification(_) => AuthError::HumanVerificationRequired,
        Error::Http(_) => AuthError::Connection,
        Error::Session(_) | Error::Io(_) => AuthError::SessionUnavailable,
        Error::Api(error) => AuthError::Service {
            http_status: error.http_status,
            code: error.code,
        },
        Error::Other(message) if message.contains(SECURITY_KEY_REQUIRED) => {
            AuthError::SecurityKeyUnsupported
        }
        _ => AuthError::AuthenticationUnavailable,
    }
}

fn mentions_totp(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("totp") || message.contains("two-factor") || message.contains("2fa")
}

fn mentions_credentials(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("credential") || message.contains("username") || message.contains("password")
}

#[cfg(test)]
mod tests {
    use proton_core::ApiError;
    use proton_core::model::ConversationLabel;

    use super::*;

    fn person(name: &str, address: &str) -> Recipient {
        Recipient {
            name: name.into(),
            address: address.into(),
            contact_id: None,
            is_proton: None,
        }
    }

    fn label(id: &str) -> ConversationLabel {
        ConversationLabel {
            id: id.into(),
            ..Default::default()
        }
    }

    fn api_error(http_status: u16) -> Error {
        Error::Api(ApiError {
            http_status,
            code: 0,
            message: String::new(),
            raw_body: String::new(),
        })
    }

    #[test]
    fn decrypted_messages_map_to_reader_models() {
        let meta = MessageMetadata {
            id: "message".into(),
            sender: person("", "alex@example.com"),
            to_list: vec![person("Demo", "demo@example.com")],
            cc_list: vec![person("", "team@example.org")],
            bcc_list: vec![person("", "archive@example.net")],
            time: 1_700_000_000,
            ..Default::default()
        };

        let message = mail_message(meta, "<p>Hello &amp; welcome</p>".into(), "text/html");

        assert_eq!(message.id, "message");
        assert_eq!(message.sender.name, None);
        assert_eq!(message.sender.address, "alex@example.com");
        assert_eq!(message.recipients.len(), 3);
        assert_eq!(message.time, Some(1_700_000_000));
        assert_eq!(
            message.body,
            MessageBody::PlainText("Hello & welcome".into())
        );
    }

    #[test]
    fn plain_text_bodies_are_kept_verbatim() {
        let body = "Line <one>\n\n  indented & raw";

        let message = mail_message(MessageMetadata::default(), body.into(), "text/plain");

        assert_eq!(message.body, MessageBody::PlainText(body.into()));
        assert_eq!(message.time, None);
    }

    #[test]
    fn abandoned_prompt_is_a_cancelled_outcome() {
        let outcome = map_sign_in_error(cancelled(), false, false);

        assert!(matches!(outcome, SignInOutcome::Cancelled));
    }

    #[test]
    fn verification_page_stays_on_proton() {
        assert_eq!(
            verification_url("", "a b/c"),
            "https://verify.proton.me/?methods=captcha&token=a%20b%2Fc"
        );
        assert_eq!(
            verification_url("https://verify-api.proton.me/x?y=1", "t"),
            "https://verify-api.proton.me/?methods=captcha&token=t"
        );
        for untrusted in [
            "http://verify.proton.me/",
            "https://proton.me.example.com/",
            "https://evil@verify.proton.me/",
            "https://example.com/",
        ] {
            assert!(
                verification_url(untrusted, "t").starts_with("https://verify.proton.me/?"),
                "{untrusted}"
            );
        }
    }

    #[test]
    fn replies_are_delivered_once_or_cancelled() {
        let (reply, mut answer) = Reply::channel();
        reply.clone().send(1);
        reply.send(2);
        assert_eq!(answer.try_recv(), Ok(Some(1)));

        let (reply, mut answer) = Reply::<u8>::channel();
        reply.cancel();
        assert!(answer.try_recv().is_err());
    }

    #[test]
    fn missing_mailbox_password_has_a_structured_outcome() {
        let outcome =
            map_sign_in_error(Error::Other(MAILBOX_PASSWORD_REQUIRED.into()), false, false);

        assert!(matches!(outcome, SignInOutcome::NeedsMailboxPassword));
    }

    #[test]
    fn security_key_requirement_is_supported_failure() {
        let outcome = map_sign_in_error(Error::Other(SECURITY_KEY_REQUIRED.into()), false, false);

        assert!(matches!(
            outcome,
            SignInOutcome::Failed(AuthError::SecurityKeyUnsupported)
        ));
    }

    #[test]
    fn unauthorized_totp_retry_is_an_invalid_code() {
        let outcome = map_sign_in_error(Error::Unauthorized, true, false);

        assert!(matches!(
            outcome,
            SignInOutcome::Failed(AuthError::InvalidTotp)
        ));
    }

    #[test]
    fn failed_mailbox_unlock_is_an_invalid_mailbox_password() {
        let outcome = map_sign_in_error(
            Error::Crypto(format!("{USER_KEY_UNLOCK_FAILED} (wrong password?)")),
            false,
            true,
        );

        assert!(matches!(
            outcome,
            SignInOutcome::Failed(AuthError::InvalidMailboxPassword)
        ));
    }

    #[test]
    fn conversation_maps_to_summary() {
        let conversation = Conversation {
            id: "conversation".into(),
            subject: "  Quarterly report ".into(),
            time: 1_700_000_000,
            num_messages: 3,
            num_unread: 1,
            senders: vec![
                person("Ada", "ada@example.com"),
                person("", "bob@example.com"),
            ],
            labels: vec![label(label_ids::INBOX), label(label_ids::STARRED)],
            ..Default::default()
        };

        let summary = summarize(conversation, MailFolder::Inbox);

        assert_eq!(
            summary,
            ConversationSummary {
                id: "conversation".into(),
                subject: Some("Quarterly report".into()),
                correspondents: Some("Ada, bob@example.com".into()),
                participants: vec![
                    MailAddress {
                        name: Some("Ada".into()),
                        address: "ada@example.com".into(),
                    },
                    MailAddress {
                        name: None,
                        address: "bob@example.com".into(),
                    },
                ],
                preview: None,
                time: Some(1_700_000_000),
                unread: true,
                starred: true,
                message_count: 3,
            }
        );
    }

    #[test]
    fn missing_subject_sender_and_time_are_absent() {
        let conversation = Conversation {
            id: "conversation".into(),
            subject: " ".into(),
            num_messages: -1,
            senders: vec![person(" ", "")],
            ..Default::default()
        };

        let summary = summarize(conversation, MailFolder::Inbox);

        assert_eq!(summary.subject, None);
        assert_eq!(summary.correspondents, None);
        assert_eq!(summary.time, None);
        assert_eq!(summary.message_count, 0);
        assert!(!summary.unread);
        assert!(!summary.starred);
    }

    #[test]
    fn folder_context_overrides_conversation_totals() {
        let mut sent = label(label_ids::SENT);
        sent.context_num_unread = Some(0);
        sent.context_time = Some(42);
        let conversation = Conversation {
            id: "conversation".into(),
            time: 99,
            num_unread: 2,
            senders: vec![person("Sender", "")],
            recipients: vec![person("Recipient", "")],
            labels: vec![sent],
            ..Default::default()
        };

        let summary = summarize(conversation, MailFolder::Sent);

        assert!(!summary.unread);
        assert_eq!(summary.time, Some(42));
        assert_eq!(summary.correspondents.as_deref(), Some("Recipient"));
    }

    #[test]
    fn long_correspondent_lists_are_shortened() {
        let people: Vec<_> = ["A", "B", "C", "D"]
            .into_iter()
            .map(|name| person(name, ""))
            .collect();

        assert_eq!(display_names(&people).as_deref(), Some("A, B, C, …"));
    }

    #[test]
    fn counts_map_only_known_folders() {
        let counts = [
            LabelCount {
                label_id: label_ids::INBOX.into(),
                total: 10,
                unread: 4,
            },
            LabelCount {
                label_id: "custom".into(),
                total: 1,
                unread: 1,
            },
        ];

        let counts = map_counts(&counts);

        assert_eq!(counts.unread(MailFolder::Inbox), Some(4));
        assert_eq!(counts.unread(MailFolder::Trash), None);
    }

    #[test]
    fn expired_session_errors_are_distinguished() {
        assert_eq!(
            map_mailbox_error(Error::Unauthorized),
            MailboxError::SessionExpired
        );
        assert_eq!(
            map_mailbox_error(api_error(401)),
            MailboxError::SessionExpired
        );
        assert_eq!(map_mailbox_error(api_error(500)), MailboxError::Service);
        assert_eq!(
            map_mailbox_error(Error::Other("unexpected".into())),
            MailboxError::Unavailable
        );
    }
}
