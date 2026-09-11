use std::sync::Arc;

use proton_core::model::enums::label_ids;
use proton_core::{Client, Conversation, Error, LabelCount, LoginOptions, Recipient};

use super::{
    AuthError, ConversationPage, ConversationSummary, LoginRequest, MailFolder, MailboxCounts,
    MailboxError,
};

const PROFILE: &str = "ruston";
const TOTP_REQUIRED: &str = "account requires 2FA but no TOTP code was provided";
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
    NeedsTotp,
    NeedsMailboxPassword,
    Failed(AuthError),
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

    pub async fn sign_in(request: LoginRequest) -> SignInOutcome {
        let has_totp = request.totp.is_some();
        let has_mailbox_password = request.mailbox_password.is_some();
        let options = LoginOptions {
            username: request.username,
            password: request.password,
            totp: request.totp,
            mailbox_password: request.mailbox_password,
            profile: PROFILE.to_owned(),
            base_url: None,
            app_version: None,
            user_agent: None,
            hv: None,
        };

        match Client::login(options).await {
            Ok(client) => SignInOutcome::Authenticated(Arc::new(Self::from_client(client))),
            Err(error) => map_sign_in_error(error, has_totp, has_mailbox_password),
        }
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

    fn from_client(client: Client) -> Self {
        let email = client.primary_email().map(str::to_owned);

        Self { client, email }
    }
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
    let subject = conversation.subject.trim();

    ConversationSummary {
        subject: (!subject.is_empty()).then(|| subject.to_owned()),
        correspondents: display_names(correspondents),
        time: (time > 0).then_some(time),
        unread: unread > 0,
        starred,
        message_count: u32::try_from(conversation.num_messages).unwrap_or(0),
        id: conversation.id,
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
        Error::Other(message) if message.contains(TOTP_REQUIRED) => SignInOutcome::NeedsTotp,
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
    fn missing_totp_has_a_structured_outcome() {
        let outcome = map_sign_in_error(Error::Other(TOTP_REQUIRED.into()), false, false);

        assert!(matches!(outcome, SignInOutcome::NeedsTotp));
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
