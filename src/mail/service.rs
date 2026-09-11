#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthError {
    UsernameRequired,
    PasswordRequired,
    TotpRequired,
    MailboxPasswordRequired,
    InvalidCredentials,
    InvalidTotp,
    InvalidMailboxPassword,
    Connection,
    HumanVerificationRequired,
    SecurityKeyUnsupported,
    SessionUnavailable,
    SessionExpired,
    Service { http_status: u16, code: i64 },
    AuthenticationUnavailable,
}

impl AuthError {
    pub fn message(self) -> &'static str {
        match self {
            Self::UsernameRequired => "Enter your Proton username or email.",
            Self::PasswordRequired => "Enter your Proton password.",
            Self::TotpRequired => "Enter your authentication code.",
            Self::MailboxPasswordRequired => "Enter your mailbox password.",
            Self::InvalidCredentials => "The username or password is incorrect.",
            Self::InvalidTotp => "The authentication code is invalid.",
            Self::InvalidMailboxPassword => "The mailbox password is incorrect.",
            Self::Connection => "Ruston could not connect to Proton Mail.",
            Self::HumanVerificationRequired => {
                "Proton requires human verification. Ruston does not support this flow yet."
            }
            Self::SecurityKeyUnsupported => {
                "This account requires FIDO2/WebAuthn, which Ruston does not support yet."
            }
            Self::SessionUnavailable => "Ruston could not access the saved Proton session.",
            Self::SessionExpired => "Your Proton session has expired. Sign in again.",
            Self::Service { .. } => "Proton Mail rejected the request. Try again later.",
            Self::AuthenticationUnavailable => {
                "Ruston could not complete authentication. Try again."
            }
        }
    }
}

pub struct LoginRequest {
    pub(crate) username: String,
    pub(crate) password: String,
    pub(crate) totp: Option<String>,
    pub(crate) mailbox_password: Option<String>,
}

impl LoginRequest {
    pub(crate) fn new(
        username: String,
        password: String,
        totp: Option<String>,
        mailbox_password: Option<String>,
    ) -> Self {
        Self {
            username,
            password,
            totp,
            mailbox_password,
        }
    }
}
