use std::sync::Arc;

use iced::Task;

use crate::mail::{AuthError, LoginRequest, ProtonMailService, ResumeOutcome, SignInOutcome};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignInStep {
    Credentials,
    Totp,
    MailboxPassword,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthState {
    CheckingSession,
    SignedOut,
    SigningIn(SignInStep),
    NeedsTotp,
    NeedsMailboxPassword,
    Authenticated { email: Option<String> },
    SigningOut,
}

#[derive(Clone)]
pub enum Message {
    UsernameChanged(String),
    PasswordChanged(String),
    TotpChanged(String),
    MailboxPasswordChanged(String),
    Submit,
    CancelChallenge,
    SessionChecked(ResumeOutcome),
    SignInFinished(SignInOutcome),
    Logout,
    LogoutFinished(Result<(), AuthError>),
}

#[derive(Default)]
struct LoginForm {
    username: String,
    password: String,
    totp: String,
    mailbox_password: String,
}

impl LoginForm {
    fn clear_sensitive(&mut self) {
        self.password.clear();
        self.totp.clear();
        self.mailbox_password.clear();
    }

    fn clear_all(&mut self) {
        self.username.clear();
        self.clear_sensitive();
    }
}

pub struct App {
    auth_state: AuthState,
    login_form: LoginForm,
    auth_error: Option<AuthError>,
    mail_service: Option<Arc<ProtonMailService>>,
}

impl App {
    pub fn boot() -> (Self, Task<Message>) {
        (
            Self::new(),
            Task::perform(ProtonMailService::resume(), Message::SessionChecked),
        )
    }

    fn new() -> Self {
        Self {
            auth_state: AuthState::CheckingSession,
            login_form: LoginForm::default(),
            auth_error: None,
            mail_service: None,
        }
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::UsernameChanged(username) => {
                self.login_form.username = username;
                self.auth_error = None;
            }
            Message::PasswordChanged(password) => {
                self.login_form.password = password;
                self.auth_error = None;
            }
            Message::TotpChanged(totp) => {
                self.login_form.totp = totp;
                self.auth_error = None;
            }
            Message::MailboxPasswordChanged(mailbox_password) => {
                self.login_form.mailbox_password = mailbox_password;
                self.auth_error = None;
            }
            Message::Submit => return self.sign_in(),
            Message::CancelChallenge => {
                if !matches!(self.auth_state, AuthState::SigningIn(_)) {
                    self.login_form.clear_sensitive();
                    self.auth_error = None;
                    self.auth_state = AuthState::SignedOut;
                }
            }
            Message::SessionChecked(outcome) => self.finish_session_check(outcome),
            Message::SignInFinished(outcome) => self.finish_sign_in(outcome),
            Message::Logout => return self.logout(),
            Message::LogoutFinished(result) => self.finish_logout(result),
        }

        Task::none()
    }

    pub fn auth_state(&self) -> &AuthState {
        &self.auth_state
    }

    pub fn username(&self) -> &str {
        &self.login_form.username
    }

    pub fn password(&self) -> &str {
        &self.login_form.password
    }

    pub fn totp(&self) -> &str {
        &self.login_form.totp
    }

    pub fn mailbox_password(&self) -> &str {
        &self.login_form.mailbox_password
    }

    pub fn error_message(&self) -> Option<&'static str> {
        self.auth_error.map(AuthError::message)
    }

    fn sign_in(&mut self) -> Task<Message> {
        let step = match self.auth_state {
            AuthState::SignedOut => SignInStep::Credentials,
            AuthState::NeedsTotp => SignInStep::Totp,
            AuthState::NeedsMailboxPassword => SignInStep::MailboxPassword,
            _ => return Task::none(),
        };

        if self.login_form.username.trim().is_empty() {
            self.auth_error = Some(AuthError::UsernameRequired);
            return Task::none();
        }
        if self.login_form.password.is_empty() {
            self.auth_error = Some(AuthError::PasswordRequired);
            return Task::none();
        }
        if step == SignInStep::Totp && self.login_form.totp.trim().is_empty() {
            self.auth_error = Some(AuthError::TotpRequired);
            return Task::none();
        }
        if step == SignInStep::MailboxPassword && self.login_form.mailbox_password.is_empty() {
            self.auth_error = Some(AuthError::MailboxPasswordRequired);
            return Task::none();
        }

        let request = LoginRequest::new(
            self.login_form.username.trim().to_owned(),
            self.login_form.password.clone(),
            optional_trimmed(&self.login_form.totp),
            optional_unmodified(&self.login_form.mailbox_password),
        );
        self.auth_error = None;
        self.auth_state = AuthState::SigningIn(step);

        Task::perform(ProtonMailService::sign_in(request), Message::SignInFinished)
    }

    fn finish_session_check(&mut self, outcome: ResumeOutcome) {
        match outcome {
            ResumeOutcome::Authenticated(service) => self.set_authenticated(service),
            ResumeOutcome::SignedOut => self.auth_state = AuthState::SignedOut,
            ResumeOutcome::Failed(error) => {
                self.auth_error = Some(error);
                self.auth_state = AuthState::SignedOut;
            }
        }
    }

    fn finish_sign_in(&mut self, outcome: SignInOutcome) {
        match outcome {
            SignInOutcome::Authenticated(service) => self.set_authenticated(service),
            SignInOutcome::NeedsTotp => {
                self.login_form.totp.clear();
                self.auth_error = None;
                self.auth_state = AuthState::NeedsTotp;
            }
            SignInOutcome::NeedsMailboxPassword => {
                self.login_form.mailbox_password.clear();
                self.auth_error = None;
                self.auth_state = AuthState::NeedsMailboxPassword;
            }
            SignInOutcome::Failed(error @ AuthError::InvalidTotp) => {
                self.login_form.totp.clear();
                self.auth_error = Some(error);
                self.auth_state = AuthState::NeedsTotp;
            }
            SignInOutcome::Failed(error @ AuthError::InvalidMailboxPassword) => {
                self.login_form.mailbox_password.clear();
                self.auth_error = Some(error);
                self.auth_state = AuthState::NeedsMailboxPassword;
            }
            SignInOutcome::Failed(error) => {
                self.login_form.clear_sensitive();
                self.auth_error = Some(error);
                self.auth_state = AuthState::SignedOut;
            }
        }
    }

    fn set_authenticated(&mut self, service: Arc<ProtonMailService>) {
        let email = service.email().map(str::to_owned);
        self.login_form.clear_all();
        self.auth_error = None;
        self.mail_service = Some(service);
        self.auth_state = AuthState::Authenticated { email };
    }

    fn logout(&mut self) -> Task<Message> {
        if !matches!(self.auth_state, AuthState::Authenticated { .. }) {
            return Task::none();
        }
        let Some(service) = self.mail_service.clone() else {
            self.auth_state = AuthState::SignedOut;
            return Task::none();
        };

        self.auth_error = None;
        self.auth_state = AuthState::SigningOut;
        Task::perform(
            async move { service.logout().await },
            Message::LogoutFinished,
        )
    }

    fn finish_logout(&mut self, result: Result<(), AuthError>) {
        match result {
            Ok(()) => {
                self.mail_service = None;
                self.login_form.clear_all();
                self.auth_error = None;
                self.auth_state = AuthState::SignedOut;
            }
            Err(error) => {
                let email = self
                    .mail_service
                    .as_ref()
                    .and_then(|service| service.email())
                    .map(str::to_owned);
                self.auth_error = Some(error);
                self.auth_state = AuthState::Authenticated { email };
            }
        }
    }
}

fn optional_trimmed(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn optional_unmodified(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_state_checks_saved_session() {
        let app = App::new();

        assert_eq!(app.auth_state, AuthState::CheckingSession);
    }

    #[test]
    fn absent_saved_session_opens_login() {
        let mut app = App::new();

        let _ = app.update(Message::SessionChecked(ResumeOutcome::SignedOut));

        assert_eq!(app.auth_state, AuthState::SignedOut);
        assert_eq!(app.auth_error, None);
    }

    #[test]
    fn totp_requirement_opens_totp_step() {
        let mut app = App::new();
        app.login_form.username = "username sentinel".into();
        app.login_form.password = "password sentinel".into();
        app.login_form.totp = "totp sentinel".into();

        let _ = app.update(Message::SignInFinished(SignInOutcome::NeedsTotp));

        assert_eq!(app.auth_state, AuthState::NeedsTotp);
        assert_eq!(app.login_form.username, "username sentinel");
        assert_eq!(app.login_form.password, "password sentinel");
        assert!(app.login_form.totp.is_empty());
    }

    #[test]
    fn terminal_sign_in_failure_clears_secrets() {
        let mut app = App::new();
        app.login_form.password = "password sentinel".into();
        app.login_form.totp = "totp sentinel".into();
        app.login_form.mailbox_password = "mailbox password sentinel".into();

        let _ = app.update(Message::SignInFinished(SignInOutcome::Failed(
            AuthError::Connection,
        )));

        assert_eq!(app.auth_state, AuthState::SignedOut);
        assert!(app.login_form.password.is_empty());
        assert!(app.login_form.totp.is_empty());
        assert!(app.login_form.mailbox_password.is_empty());
    }

    #[test]
    fn failed_logout_restores_authenticated_shell() {
        let mut app = App::new();
        app.auth_state = AuthState::SigningOut;

        let _ = app.update(Message::LogoutFinished(Err(AuthError::SessionUnavailable)));

        assert_eq!(app.auth_state, AuthState::Authenticated { email: None });
        assert_eq!(app.auth_error, Some(AuthError::SessionUnavailable));
    }

    #[test]
    fn successful_logout_opens_login() {
        let mut app = App::new();
        app.auth_state = AuthState::SigningOut;

        let _ = app.update(Message::LogoutFinished(Ok(())));

        assert_eq!(app.auth_state, AuthState::SignedOut);
    }
}
