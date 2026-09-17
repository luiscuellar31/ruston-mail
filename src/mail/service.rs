use std::fmt;

use secrecy::SecretString;

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

impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::UsernameRequired => "Enter your Proton username or email.",
            Self::PasswordRequired => "Enter your Proton password.",
            Self::TotpRequired => "Enter your authentication code.",
            Self::MailboxPasswordRequired => "Enter your mailbox password.",
            Self::InvalidCredentials => "The username or password is incorrect.",
            Self::InvalidTotp => "The authentication code is invalid.",
            Self::InvalidMailboxPassword => "The mailbox password is incorrect.",
            Self::Connection => "Ruston Mail could not connect to Proton.",
            Self::HumanVerificationRequired => {
                "Proton requires human verification. Ruston Mail does not support this flow yet."
            }
            Self::SecurityKeyUnsupported => {
                "This account requires FIDO2/WebAuthn, which Ruston Mail does not support yet."
            }
            Self::SessionUnavailable => "Ruston Mail could not access the saved Proton session.",
            Self::SessionExpired => "Your Proton session has expired. Sign in again.",
            // Only the status and code are shown; Proton's message and raw body
            // are never surfaced.
            Self::Service { http_status, code } => {
                return write!(
                    f,
                    "Proton Mail rejected the request. (HTTP {http_status}, code {code})"
                );
            }
            Self::AuthenticationUnavailable => {
                "Ruston Mail could not complete authentication. Try again."
            }
        };

        f.write_str(message)
    }
}

pub struct LoginRequest {
    pub(crate) username: String,
    pub(crate) password: SecretString,
    pub(crate) totp: Option<SecretString>,
    pub(crate) mailbox_password: Option<SecretString>,
}

impl LoginRequest {
    pub(crate) fn new(
        username: String,
        password: SecretString,
        totp: Option<SecretString>,
        mailbox_password: Option<SecretString>,
    ) -> Self {
        Self {
            username,
            password,
            totp,
            mailbox_password,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_error_shows_status_and_code() {
        let error = AuthError::Service {
            http_status: 422,
            code: 5003,
        };

        assert_eq!(
            error.to_string(),
            "Proton Mail rejected the request. (HTTP 422, code 5003)"
        );
    }

    #[test]
    fn other_errors_keep_their_messages() {
        assert_eq!(
            AuthError::InvalidCredentials.to_string(),
            "The username or password is incorrect."
        );
        assert_eq!(
            AuthError::HumanVerificationRequired.to_string(),
            "Proton requires human verification. Ruston Mail does not support this flow yet."
        );
    }
}
