use std::sync::Arc;

use proton_core::{Client, Error, LoginOptions};

use super::{AuthError, LoginRequest};

const PROFILE: &str = "ruston";
const TOTP_REQUIRED: &str = "account requires 2FA but no TOTP code was provided";
const MAILBOX_PASSWORD_REQUIRED: &str = "this account uses a separate mailbox password";
const SECURITY_KEY_REQUIRED: &str = "FIDO2/WebAuthn";
const USER_KEY_UNLOCK_FAILED: &str = "no user key could be unlocked";

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

    fn from_client(client: Client) -> Self {
        let email = client.primary_email().map(str::to_owned);

        Self { client, email }
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
    use super::*;

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
}
