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

impl AuthState {
    pub fn is_authenticated(&self) -> bool {
        matches!(self, Self::Authenticated { .. })
    }

    /// Whether a sign-in is under way: running, or waiting for the person.
    /// Only then can it be cancelled, or its outcome be applied.
    pub(super) fn sign_in_running(&self) -> bool {
        match self {
            Self::SigningIn(_)
            | Self::NeedsTotp
            | Self::NeedsMailboxPassword
            | Self::NeedsHumanVerification { .. } => true,
            Self::CheckingSession
            | Self::SignedOut
            | Self::Authenticated { .. }
            | Self::SigningOut => false,
        }
    }
}

pub(super) struct LoginForm {
    pub(super) username: String,
    pub(super) password: Zeroizing<String>,
    pub(super) totp: Zeroizing<String>,
    pub(super) mailbox_password: Zeroizing<String>,
}

/// Room for a secret typed a key at a time. A string that outgrows its buffer
/// moves to a bigger one and leaves what was typed so far behind, unerased.
const SECRET_CAPACITY: usize = 256;

impl Default for LoginForm {
    fn default() -> Self {
        let secret = || Zeroizing::new(String::with_capacity(SECRET_CAPACITY));
        Self {
            username: String::new(),
            password: secret(),
            totp: secret(),
            mailbox_password: secret(),
        }
    }
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
        // Typing cannot make a demo process able to sign in, so its notice stays.
        if !self.demo_process {
            self.auth_error = None;
        }
    }

    pub fn error_message(&self) -> Option<String> {
        self.auth_error.map(|error| error.to_string())
    }

    pub(super) fn sign_in(&mut self) -> Effects {
        if self.demo_process {
            // The window still says "demo"; it must not hold a real account.
            self.auth_error = Some(AuthError::DemoSignInUnavailable);
            return Effects::none();
        }
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
        // Signing in replaces the saved session; there is nothing to retry.
        self.resume_may_retry = false;
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
            SignInPrompt::MailboxPassword(_) => {
                self.login_form.mailbox_password.zeroize();
                self.auth_state = AuthState::NeedsMailboxPassword;
                Effects::none()
            }
            SignInPrompt::HumanVerification { url, .. } => {
                self.auth_state = AuthState::NeedsHumanVerification { url: url.clone() };
                // A browser that fails to start goes unreported: the prompt
                // also offers "Copy link".
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
            SignInPrompt::MailboxPassword(reply) => {
                if self.login_form.mailbox_password.is_empty() {
                    self.auth_error = Some(AuthError::MailboxPasswordRequired);
                    self.pending_prompt = Some(SignInPrompt::MailboxPassword(reply));
                    return Effects::none();
                }
                // Taken exactly as typed, and kept no longer than the answer.
                let password = SecretString::from(self.login_form.mailbox_password.as_str());
                self.login_form.mailbox_password.zeroize();
                reply.send(password);
                self.auth_state = AuthState::SigningIn(SignInStep::MailboxPassword);
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
        // Only the check the startup screen waits for decides anything.
        if self.auth_state != AuthState::CheckingSession {
            return Effects::none();
        }
        match outcome {
            ResumeOutcome::Authenticated(service) => return self.set_authenticated(service),
            ResumeOutcome::SignedOut => self.auth_state = AuthState::SignedOut,
            ResumeOutcome::Failed(error) => {
                self.resume_may_retry = error.may_pass();
                self.auth_error = Some(error);
                self.auth_state = AuthState::SignedOut;
            }
        }

        Effects::none()
    }

    /// Whether the sign-in screen offers to try the saved session again.
    pub fn can_retry_saved_session(&self) -> bool {
        self.resume_may_retry && self.auth_state == AuthState::SignedOut
    }

    /// Goes back to the startup check with the session still saved, as when
    /// Ruston starts.
    pub(super) fn retry_saved_session(&mut self) -> Effects {
        if !self.can_retry_saved_session() {
            return Effects::none();
        }
        self.resume_may_retry = false;
        self.auth_error = None;
        self.auth_state = AuthState::CheckingSession;
        Effects::perform(ProtonMailService::resume(), Message::SessionChecked)
    }

    pub(super) fn finish_sign_in(
        &mut self,
        attempt: super::AuthAttempt,
        outcome: SignInOutcome,
    ) -> Effects {
        if attempt != self.auth_attempt || !self.auth_state.sign_in_running() {
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
        if !self.auth_state.sign_in_running() {
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
        let account = Some(service.user_id().to_owned());
        self.open_mailbox(MailBackend::Proton(service), email, account)
    }

    /// Opens a session's mailbox. `account` is the Proton account signed in,
    /// which a draft its last session ended under comes back to.
    pub(super) fn open_mailbox(
        &mut self,
        backend: MailBackend,
        email: Option<String>,
        account: Option<String>,
    ) -> Effects {
        self.advance_session_epoch();
        self.new_mail = super::notifications::NewMail::default();
        self.login_form.clear_all();
        self.auth_error = None;
        let page_size = backend.page_size();
        self.backend = Some(backend);
        self.account = account;
        self.auth_state = AuthState::Authenticated { email };
        let draft = self.restore_parked_draft();

        let page_request = self.next_request();
        let counts_request = self.next_request();
        // Demo mail has no account folders, so always start it in Inbox.
        let start = self.settings.start_folder();
        let folder = if self.is_demo() && start.system().is_none() {
            Folder::INBOX
        } else {
            start
        };
        let (mailbox, page) = Mailbox::open(folder, page_size, page_request, counts_request);
        self.mailbox = Some(mailbox);

        Effects::batch([
            self.fetch_page(Some(page)),
            self.fetch_counts(Some(counts_request)),
            self.fetch_folders(),
            if self.is_demo() {
                Effects::none()
            } else {
                self.fetch_new_mail_notification()
            },
            draft,
        ])
    }

    pub(super) fn logout(&mut self) -> Effects {
        if !self.auth_state.is_authenticated() {
            return Effects::none();
        }
        // Signing out never takes a message with it: the outcome of a send
        // would never arrive, and a written message is discarded only once
        // the user has chosen to.
        if self
            .compose
            .as_ref()
            .is_some_and(|draft| draft.in_flight() || !draft.is_untouched())
        {
            return Effects::none();
        }
        self.inspections.cancel();
        let service = match &self.backend {
            Some(MailBackend::Proton(service)) => service.clone(),
            // Leaving demo mode has no Proton session to revoke. The login
            // screen says at once that this process cannot sign in.
            Some(MailBackend::Demo(_)) => {
                self.close_mailbox(Some(AuthError::DemoSignInUnavailable));
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

    /// Completes the sign-out process.
    ///
    /// Remote session revocation is best-effort. Regardless of whether the
    /// Proton server acknowledges revocation or fails (e.g. offline, timeout,
    /// network error), in-memory mailbox state and drafts are discarded.
    /// Core completes local credential cleanup before revocation; a storage
    /// failure is shown on the sign-in screen rather than silently ignored.
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

        self.close_mailbox(result.err());
    }

    /// Returns to the login screen, dropping the session's state. Only a draft
    /// whose session Proton ended stays, in memory, for its own account.
    pub(super) fn close_mailbox(&mut self, error: Option<AuthError>) {
        // A folder the account made belongs to it: the next account must not
        // see its name or have its ID asked for. Proton's own folders stay
        // remembered, and the demo never changes what is remembered.
        if self.settings.folder.custom_id().is_some() {
            self.remember_folder(&Folder::INBOX);
        }
        self.advance_session_epoch();
        self.backend = None;
        self.mailbox = None;
        // A draft belongs to the account that created it and must never cross
        // into another account's session. One Proton ended the session under
        // waits in memory for its own account to sign in again.
        let account = self.account.take();
        self.park_draft(account.filter(|_| error == Some(AuthError::SessionExpired)));
        self.pending_link = None;
        // Pending email requests accepted in a session must not cross accounts.
        self.pending_mailto.clear();
        self.mailto_error = None;
        self.showing_settings = false;
        // Never show one account's folders in the next session.
        self.folders.clear();
        self.place_looks.clear();
        // A file one session saved is not news for the next one.
        self.failed_save = None;
        self.notice = None;
        self.saving_attachment = None;
        self.new_mail = super::notifications::NewMail::default();
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
