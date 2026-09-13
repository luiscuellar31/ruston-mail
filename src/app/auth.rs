//! Signing in, and everything that follows from being signed in or out.
//!
//! The state a sign-in moves through, the form it is driven from, and the
//! steps between a saved session and an open mailbox. `ui::login` draws what
//! this decides.

use std::sync::Arc;

use crate::mail::{
    AuthError, Folder, LoginRequest, MailBackend, ProtonMailService, ResumeOutcome, SignInOutcome,
    SignInPrompt,
};

use super::{App, Effects, Mailbox, Message, non_empty, open_in_browser, optional_trimmed};

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
    pub(super) password: String,
    pub(super) totp: String,
    pub(super) mailbox_password: String,
}

impl LoginForm {
    pub(super) fn clear_sensitive(&mut self) {
        self.password.clear();
        self.totp.clear();
        self.mailbox_password.clear();
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
            self.login_form.password.clone(),
            optional_trimmed(&self.login_form.totp),
            non_empty(&self.login_form.mailbox_password),
        );
        self.auth_error = None;
        self.auth_state = AuthState::SigningIn(step);

        sign_in_task(request)
    }

    /// Shows a question from the running sign-in. Prompts that arrive when no
    /// sign-in is running are cancelled.
    pub(super) fn show_prompt(&mut self, prompt: SignInPrompt) -> Effects {
        if !matches!(self.auth_state, AuthState::SigningIn(_)) {
            prompt.cancel();
            return Effects::none();
        }

        self.auth_error = None;
        let task = match &prompt {
            SignInPrompt::Totp(_) => {
                self.login_form.totp.clear();
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
                let code = self.login_form.totp.trim().to_owned();
                if code.is_empty() {
                    self.auth_error = Some(AuthError::TotpRequired);
                    self.pending_prompt = Some(SignInPrompt::Totp(reply));
                    return Effects::none();
                }
                // Codes expire; never keep one for a later attempt.
                self.login_form.totp.clear();
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

    pub(super) fn finish_sign_in(&mut self, outcome: SignInOutcome) -> Effects {
        match outcome {
            SignInOutcome::Authenticated(service) => return self.set_authenticated(service),
            // The user already left with Back.
            SignInOutcome::Cancelled => {}
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

        Effects::none()
    }

    pub(super) fn set_authenticated(&mut self, service: Arc<ProtonMailService>) -> Effects {
        let email = service.email().map(str::to_owned);
        self.open_mailbox(MailBackend::Proton(service), email)
    }

    pub(super) fn open_mailbox(&mut self, backend: MailBackend, email: Option<String>) -> Effects {
        self.login_form.clear_all();
        self.auth_error = None;
        let page_size = backend.page_size();
        self.backend = Some(backend);
        self.auth_state = AuthState::Authenticated { email };

        let page_request = self.next_request();
        let counts_request = self.next_request();
        // Someone comes back to the folder they pinned, or the one they were
        // last reading. The demo account made no folders of its own, so a
        // remembered one would open a place with none of its fictional mail
        // in it.
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
        Effects::perform(
            async move { service.logout().await },
            Message::LogoutFinished,
        )
    }

    pub(super) fn finish_logout(&mut self, result: Result<(), AuthError>) {
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

    /// Drops the backend and all mailbox data and returns to the login screen.
    /// Everything the last session put on screen goes with it: what comes
    /// back is a sign-in, and after it a mailbox, not whatever was open when
    /// the session ended.
    pub(super) fn close_mailbox(&mut self, error: Option<AuthError>) {
        self.backend = None;
        self.mailbox = None;
        self.pending_link = None;
        self.showing_settings = false;
        // These are one account's own names. The next sign-in may be someone
        // else, and until their folders arrive the sidebar would be showing
        // the previous account's.
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
fn sign_in_task(request: LoginRequest) -> Effects {
    Effects::sign_in(request)
}

// ponytail: launch failures are ignored; the page offers "Copy link" instead.
