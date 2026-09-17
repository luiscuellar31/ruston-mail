//! Authentication state and session lifecycle. `ui::login` renders it.

use std::sync::Arc;

use secrecy::SecretString;
use zeroize::{Zeroize, Zeroizing};

use crate::mail::{
    AuthError, Folder, LoginRequest, MailBackend, ProtonMailService, ResumeOutcome, SignInOutcome,
    SignInPrompt,
};

use super::{App, Effects, Mailbox, Message, open_in_browser};

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
    /// Waiting for the user to finish Proton's check in the browser.
    NeedsHumanVerification {
        url: String,
    },
    Authenticated {
        email: Option<String>,
    },
    SigningOut,
}

#[derive(Default)]
pub(super) struct LoginForm {
    pub(super) username: String,
    pub(super) password: Zeroizing<String>,
    pub(super) totp: Zeroizing<String>,
    pub(super) mailbox_password: Zeroizing<String>,
}

impl LoginForm {
    pub(super) fn clear_sensitive(&mut self) {
        self.password.zeroize();
        self.totp.zeroize();
        self.mailbox_password.zeroize();
    }

    pub(super) fn clear_all(&mut self) {
        self.username.clear();
        self.clear_sensitive();
    }
}

impl App {
    pub fn auth_state(&self) -> &AuthState {
        &self.auth_state
    }

    pub(crate) fn username_mut(&mut self) -> &mut String {
        &mut self.login_form.username
    }

    pub(crate) fn password_mut(&mut self) -> &mut String {
        &mut self.login_form.password
    }

    pub(crate) fn totp_mut(&mut self) -> &mut String {
        &mut self.login_form.totp
    }

    pub(crate) fn mailbox_password_mut(&mut self) -> &mut String {
        &mut self.login_form.mailbox_password
    }

    pub(crate) fn login_edited(&mut self) {
        self.auth_error = None;
    }

    pub fn error_message(&self) -> Option<String> {
        self.auth_error.map(|error| error.to_string())
    }

    pub(super) fn sign_in(&mut self) -> Effects {
        if let Some(prompt) = self.pending_prompt.take() {
            return self.answer_prompt(prompt);
        }

        let step = match self.auth_state {
            AuthState::SignedOut => SignInStep::Credentials,
            AuthState::NeedsTotp => SignInStep::Totp,
            AuthState::NeedsMailboxPassword => SignInStep::MailboxPassword,
            _ => return Effects::none(),
        };

        if self.login_form.username.trim().is_empty() {
            self.auth_error = Some(AuthError::UsernameRequired);
            return Effects::none();
        }
        if self.login_form.password.is_empty() {
            self.auth_error = Some(AuthError::PasswordRequired);
            return Effects::none();
        }
        if step == SignInStep::Totp && self.login_form.totp.trim().is_empty() {
            self.auth_error = Some(AuthError::TotpRequired);
            return Effects::none();
        }
        if step == SignInStep::MailboxPassword && self.login_form.mailbox_password.is_empty() {
            self.auth_error = Some(AuthError::MailboxPasswordRequired);
            return Effects::none();
        }

        let request = LoginRequest::new(
            self.login_form.username.trim().to_owned(),
            SecretString::from(self.login_form.password.as_str()),
            optional_trimmed_secret(&self.login_form.totp),
            non_empty_secret(&self.login_form.mailbox_password),
        );
        self.auth_error = None;
        self.auth_state = AuthState::SigningIn(step);
        let attempt = self.advance_auth_attempt();

        sign_in_task(attempt, request)
    }

    /// Shows a question from the running sign-in. Prompts that arrive when no
    /// sign-in is running are cancelled.
    pub(super) fn show_prompt(
        &mut self,
        attempt: super::AuthAttempt,
        prompt: SignInPrompt,
    ) -> Effects {
        if attempt != self.auth_attempt || !matches!(self.auth_state, AuthState::SigningIn(_)) {
            prompt.cancel();
            return Effects::none();
        }

        self.auth_error = None;
        let task = match &prompt {
            SignInPrompt::Totp(_) => {
                self.login_form.totp.zeroize();
                self.auth_state = AuthState::NeedsTotp;
                Effects::none()
            }
            SignInPrompt::HumanVerification { url, .. } => {
                self.auth_state = AuthState::NeedsHumanVerification { url: url.clone() };
                open_in_browser(url.clone())
            }
        };
        self.pending_prompt = Some(prompt);
        task
    }

    pub(super) fn answer_prompt(&mut self, prompt: SignInPrompt) -> Effects {
        match prompt {
            SignInPrompt::Totp(reply) => {
                let code = self.login_form.totp.trim();
                if code.is_empty() {
                    self.auth_error = Some(AuthError::TotpRequired);
                    self.pending_prompt = Some(SignInPrompt::Totp(reply));
                    return Effects::none();
                }
                let code = SecretString::from(code);
                // Codes expire; never keep one for a later attempt.
                self.login_form.totp.zeroize();
                reply.send(code);
                self.auth_state = AuthState::SigningIn(SignInStep::Totp);
            }
            SignInPrompt::HumanVerification { done, .. } => {
                done.send(());
                self.auth_state = AuthState::SigningIn(SignInStep::Credentials);
            }
        }

        self.auth_error = None;
        Effects::none()
    }

    pub(super) fn finish_session_check(&mut self, outcome: ResumeOutcome) -> Effects {
        match outcome {
            ResumeOutcome::Authenticated(service) => return self.set_authenticated(service),
            ResumeOutcome::SignedOut => self.auth_state = AuthState::SignedOut,
            ResumeOutcome::Failed(error) => {
                self.auth_error = Some(error);
                self.auth_state = AuthState::SignedOut;
            }
        }

        Effects::none()
    }

    pub(super) fn finish_sign_in(
        &mut self,
        attempt: super::AuthAttempt,
        outcome: SignInOutcome,
    ) -> Effects {
        if attempt != self.auth_attempt
            || matches!(
                self.auth_state,
                AuthState::CheckingSession
                    | AuthState::SignedOut
                    | AuthState::Authenticated { .. }
                    | AuthState::SigningOut
            )
        {
            return Effects::none();
        }
        if let Some(prompt) = self.pending_prompt.take() {
            prompt.cancel();
        }
        match outcome {
            SignInOutcome::Authenticated(service) => return self.set_authenticated(service),
            SignInOutcome::Cancelled => {
                self.login_form.clear_sensitive();
                self.auth_error = None;
                self.auth_state = AuthState::SignedOut;
            }
            SignInOutcome::NeedsMailboxPassword => {
                self.login_form.mailbox_password.zeroize();
                self.auth_error = None;
                self.auth_state = AuthState::NeedsMailboxPassword;
            }
            SignInOutcome::Failed(error @ AuthError::InvalidTotp) => {
                self.login_form.totp.zeroize();
                self.auth_error = Some(error);
                self.auth_state = AuthState::NeedsTotp;
            }
            SignInOutcome::Failed(error @ AuthError::InvalidMailboxPassword) => {
                self.login_form.mailbox_password.zeroize();
                self.auth_error = Some(error);
                self.auth_state = AuthState::NeedsMailboxPassword;
            }
            SignInOutcome::Failed(error) => {
                self.login_form.clear_sensitive();
                self.auth_error = Some(error);
                self.auth_state = AuthState::SignedOut;
            }
        }

        Effects::none()
    }

    pub(super) fn cancel_sign_in(&mut self) -> Effects {
        if !matches!(
            self.auth_state,
            AuthState::SigningIn(_)
                | AuthState::NeedsTotp
                | AuthState::NeedsMailboxPassword
                | AuthState::NeedsHumanVerification { .. }
        ) {
            return Effects::none();
        }
        if let Some(prompt) = self.pending_prompt.take() {
            prompt.cancel();
        }
        self.login_form.clear_sensitive();
        self.auth_error = None;
        self.auth_state = AuthState::SignedOut;
        self.advance_auth_attempt();
        Effects::cancel_sign_in()
    }

    pub(super) fn set_authenticated(&mut self, service: Arc<ProtonMailService>) -> Effects {
        let email = service.email().map(str::to_owned);
        self.open_mailbox(MailBackend::Proton(service), email)
    }

    pub(super) fn open_mailbox(&mut self, backend: MailBackend, email: Option<String>) -> Effects {
        self.advance_session_epoch();
        self.login_form.clear_all();
        self.auth_error = None;
        let page_size = backend.page_size();
        self.backend = Some(backend);
        self.auth_state = AuthState::Authenticated { email };

        let page_request = self.next_request();
        let counts_request = self.next_request();
        // Demo mail has no account folders, so always start it in Inbox.
        let remembered = self.settings.start_folder();
        let folder = if self.is_demo() && remembered.system().is_none() {
            Folder::INBOX
        } else {
            remembered
        };
        let (mailbox, page) = Mailbox::open(folder, page_size, page_request, counts_request);
        self.mailbox = Some(mailbox);

        Effects::batch([
            self.fetch_page(Some(page)),
            self.fetch_counts(Some(counts_request)),
            self.fetch_folders(),
        ])
    }

    pub(super) fn logout(&mut self) -> Effects {
        if !matches!(self.auth_state, AuthState::Authenticated { .. }) {
            return Effects::none();
        }
        let service = match &self.backend {
            Some(MailBackend::Proton(service)) => service.clone(),
            // Leaving demo mode has no Proton session to revoke.
            Some(MailBackend::Demo(_)) => {
                self.close_mailbox(None);
                return Effects::none();
            }
            None => {
                self.auth_state = AuthState::SignedOut;
                return Effects::none();
            }
        };

        self.auth_error = None;
        self.auth_state = AuthState::SigningOut;
        let epoch = self.session_epoch;
        Effects::perform(async move { service.logout().await }, move |result| {
            Message::LogoutFinished(epoch, result)
        })
    }

    pub(super) fn finish_logout(
        &mut self,
        epoch: super::SessionEpoch,
        result: Result<(), AuthError>,
    ) {
        if !self.is_current_session(epoch) {
            return;
        }
        // The session may have expired while signing out.
        if self.auth_state != AuthState::SigningOut {
            return;
        }

        match result {
            Ok(()) => self.close_mailbox(None),
            Err(error) => {
                let email = match &self.backend {
                    Some(MailBackend::Proton(service)) => service.email().map(str::to_owned),
                    _ => None,
                };
                self.auth_error = Some(error);
                self.auth_state = AuthState::Authenticated { email };
            }
        }
    }

    /// Drops all session state and returns to the login screen.
    pub(super) fn close_mailbox(&mut self, error: Option<AuthError>) {
        self.advance_session_epoch();
        self.backend = None;
        self.mailbox = None;
        // A draft belongs to the account that created it and must never cross
        // into a later session.
        self.compose = None;
        self.pending_link = None;
        self.showing_settings = false;
        // Never show one account's folders in the next session.
        self.folders.clear();
        // A file one session saved is not news for the next one.
        self.saved_attachment = None;
        self.saving_attachment = None;
        self.login_form.clear_all();
        self.auth_error = error;
        self.auth_state = AuthState::SignedOut;
    }
}

/// Runs one sign-in, forwarding its prompts and final outcome as messages.
fn sign_in_task(attempt: super::AuthAttempt, request: LoginRequest) -> Effects {
    Effects::sign_in(attempt, request)
}

fn optional_trimmed_secret(value: &str) -> Option<SecretString> {
    let value = value.trim();
    (!value.is_empty()).then(|| SecretString::from(value))
}

/// A mailbox password is taken exactly as typed, spaces included.
fn non_empty_secret(value: &str) -> Option<SecretString> {
    (!value.is_empty()).then(|| SecretString::from(value))
}

// ponytail: launch failures are ignored; the page offers "Copy link" instead.
