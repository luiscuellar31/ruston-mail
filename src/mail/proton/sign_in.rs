//! Signing in to Proton, resuming a saved session and signing out, with the
//! questions a sign-in asks the person while it waits for their answers.

use super::ProtonMailService;
use super::map_auth_error;
use super::mentions_credentials;
use super::mentions_totp;
use super::resume_within;
use super::verification_error;
use crate::mail::AuthError;
use crate::mail::LoginRequest;
use futures::SinkExt;
use futures::channel::mpsc;
use futures::channel::oneshot;
use ruston_core::Client;
use ruston_core::Error;
use ruston_core::HvChallenge;
use ruston_core::HvResolver;
use ruston_core::LoginOptions;
use ruston_core::LoginPrompts;
use ruston_core::MailboxPasswordPrompt;
use ruston_core::TotpPrompt;
use ruston_core::model::enums::human_verification;
use secrecy::SecretString;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;
/// A complete authentication attempt must eventually release its credentials
/// even when the server stops responding. The time a person spends on a
/// prompt, solving a CAPTCHA or finding a code, does not count.
pub(super) const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(120);

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
    Totp(PromptAnswer<SecretString>),
    /// Enter the separate mailbox password of a two-password account.
    MailboxPassword(PromptAnswer<SecretString>),
    /// Complete Proton's check at `url` in a browser, then confirm.
    HumanVerification { url: String, done: PromptAnswer<()> },
}

impl SignInPrompt {
    /// Abandons the prompt; the waiting sign-in ends as cancelled.
    pub fn cancel(self) {
        match self {
            Self::Totp(reply) | Self::MailboxPassword(reply) => reply.cancel(),
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
pub struct PromptAnswer<T>(Arc<Mutex<Option<oneshot::Sender<T>>>>);

impl<T> PromptAnswer<T> {
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

/// An honest client identity, like ruston-cli's.
fn user_agent() -> String {
    format!(
        "ruston/{} ({})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS
    )
}

/// A prompt for a secret. Core gives the TOTP code's and the mailbox
/// password's prompts this same type, so one builder serves both.
type SecretPrompt = TotpPrompt;

/// What the person signing in gave, up front or when asked. A failure
/// means something else once a code or a mailbox password was in play.
#[derive(Clone, Copy, Default)]
pub(super) struct Supplied {
    pub(super) totp: bool,
    pub(super) mailbox_password: bool,
}

/// Asks the person signing in, through the sign-in's events, for what the
/// sign-in needs. Clones ask through the same events, cancel the same
/// sign-in and count their time on the same clock.
#[derive(Clone)]
pub(super) struct Prompter {
    pub(super) events: mpsc::Sender<SignInEvent>,
    /// Set when a prompt is abandoned, which ends the sign-in as cancelled.
    pub(super) cancelled: Arc<AtomicBool>,
    /// The time spent on prompts, which the sign-in deadline leaves out.
    pub(super) prompt_time: Arc<PromptTime>,
}

impl Prompter {
    pub(super) fn new(events: mpsc::Sender<SignInEvent>) -> Self {
        Self {
            events,
            cancelled: Arc::default(),
            prompt_time: Arc::default(),
        }
    }

    /// Asks for the TOTP code only when Proton requires it, after any human
    /// verification, so the code is still fresh. `answered` is set once the
    /// person gives one.
    pub(super) fn totp(&self, answered: Arc<AtomicBool>) -> TotpPrompt {
        self.secret(SignInPrompt::Totp, "no TOTP code was given", answered)
    }

    /// Asks for the mailbox password only when the account has a separate
    /// one, once its password and any code are accepted. `answered` is set
    /// once the person gives it.
    pub(super) fn mailbox_password(&self, answered: Arc<AtomicBool>) -> MailboxPasswordPrompt {
        self.secret(
            SignInPrompt::MailboxPassword,
            "no mailbox password was given",
            answered,
        )
    }

    /// A secret the sign-in asks the person for with `prompt`. Only a sign-in
    /// asks, so a secret nobody gives ends it, with `unanswered`.
    fn secret(
        &self,
        prompt: fn(PromptAnswer<SecretString>) -> SignInPrompt,
        unanswered: &'static str,
        answered: Arc<AtomicBool>,
    ) -> SecretPrompt {
        let prompter = self.clone();
        Arc::new(move || {
            let prompter = prompter.clone();
            let answered = answered.clone();
            Box::pin(async move {
                let secret = prompter.ask(prompt).await.map_err(|_| {
                    prompter.cancelled.store(true, Ordering::Relaxed);
                    Error::Other(unanswered.to_owned())
                })?;
                answered.store(true, Ordering::Relaxed);
                Ok(secret)
            })
        })
    }

    /// Proton Bridge's external-browser flow: the verification page attaches
    /// the solved captcha to the challenge, so the retry echoes the challenge
    /// token.
    pub(super) fn human_verification(&self) -> HvResolver {
        let prompter = self.clone();
        Arc::new(move |challenge: HvChallenge| {
            let prompter = prompter.clone();
            Box::pin(async move {
                if !challenge.offers_captcha() {
                    return Err(Error::HumanVerification(challenge));
                }
                let url = challenge.verification_url();
                let prompt = |done| SignInPrompt::HumanVerification { url, done };
                match prompter.ask(prompt).await {
                    Ok(()) => Ok((challenge.token, human_verification::CAPTCHA.to_owned())),
                    // The client keeps this resolver after its sign-in. A
                    // challenge then is Proton's requirement, as for a
                    // resumed session.
                    Err(Unanswered::Closed) => Err(Error::HumanVerification(challenge)),
                    Err(Unanswered::Dismissed) => {
                        prompter.cancelled.store(true, Ordering::Relaxed);
                        Err(Error::HumanVerification(challenge))
                    }
                }
            })
        })
    }

    /// Sends a prompt to the UI and waits for the answer, on the time
    /// `prompt_time` sets aside.
    async fn ask<T: Send + 'static>(
        &self,
        prompt: impl FnOnce(PromptAnswer<T>) -> SignInPrompt,
    ) -> Result<T, Unanswered> {
        let _answering = self.prompt_time.start();
        let (reply, answer) = PromptAnswer::channel();
        self.events
            .clone()
            .send(SignInEvent::Prompt(prompt(reply)))
            .await
            .map_err(|_| Unanswered::Closed)?;
        answer.await.map_err(|_| Unanswered::Dismissed)
    }
}

/// Why a prompt got no answer.
enum Unanswered {
    /// The sign-in is over, so nothing shows prompts any more.
    Closed,
    /// The prompt was abandoned.
    Dismissed,
}

/// How long the person signing in has spent on prompts, which the sign-in
/// deadline leaves out.
#[derive(Default)]
pub(super) struct PromptTime(Mutex<PromptClock>);

#[derive(Default)]
struct PromptClock {
    spent: Duration,
    /// When the prompt being answered now was shown.
    since: Option<tokio::time::Instant>,
}

impl PromptTime {
    /// Counts the time until the returned guard is dropped, as a prompt's.
    pub(super) fn start(&self) -> Answering<'_> {
        self.0.lock().expect("prompt time lock poisoned").since = Some(tokio::time::Instant::now());
        Answering(self)
    }

    /// The time set aside by `now`, a prompt still open included.
    pub(super) fn spent(&self, now: tokio::time::Instant) -> Duration {
        let clock = self.0.lock().expect("prompt time lock poisoned");
        clock.spent
            + clock
                .since
                .map_or(Duration::ZERO, |since| now.saturating_duration_since(since))
    }
}

/// A prompt being answered; dropping it stops counting its time.
pub(super) struct Answering<'a>(&'a PromptTime);

impl Drop for Answering<'_> {
    fn drop(&mut self) {
        let mut clock = self.0.0.lock().expect("prompt time lock poisoned");
        if let Some(since) = clock.since.take() {
            clock.spent += since.elapsed();
        }
    }
}

/// Runs `sign_in` for at most `limit` of its own time: what `prompt_time`
/// sets aside does not count. `None` when that time runs out first.
pub(super) async fn within<F: Future>(
    limit: Duration,
    prompt_time: &PromptTime,
    sign_in: F,
) -> Option<F::Output> {
    let started = tokio::time::Instant::now();
    let mut deadline = started + limit;
    tokio::pin!(sign_in);
    loop {
        if let Ok(output) = tokio::time::timeout_at(deadline, &mut sign_in).await {
            return Some(output);
        }
        let now = tokio::time::Instant::now();
        let elapsed = now.duration_since(started);
        let worked = elapsed.saturating_sub(prompt_time.spent(now));
        if worked >= limit {
            return None;
        }
        deadline = now + (limit - worked);
    }
}

pub(super) fn map_sign_in_error(error: Error, supplied: Supplied) -> SignInOutcome {
    match error {
        Error::MailboxPasswordRequired => SignInOutcome::NeedsMailboxPassword,
        Error::SecurityKeyRequired => SignInOutcome::Failed(AuthError::SecurityKeyUnsupported),
        Error::HumanVerification(challenge) => {
            SignInOutcome::Failed(verification_error(&challenge))
        }
        Error::Unauthorized if supplied.totp => SignInOutcome::Failed(AuthError::InvalidTotp),
        Error::Unauthorized => SignInOutcome::Failed(AuthError::InvalidCredentials),
        Error::Api(error) if supplied.totp && mentions_totp(&error.message) => {
            SignInOutcome::Failed(AuthError::InvalidTotp)
        }
        Error::Api(error) if error.refuses_session() || mentions_credentials(&error.message) => {
            SignInOutcome::Failed(AuthError::InvalidCredentials)
        }
        Error::UserKeysLocked if supplied.mailbox_password => {
            SignInOutcome::Failed(AuthError::InvalidMailboxPassword)
        }
        error => SignInOutcome::Failed(map_auth_error(error)),
    }
}

impl ProtonMailService {
    /// Resumes the session saved under `profile`.
    pub async fn resume(profile: String) -> ResumeOutcome {
        resume_within(Client::resume(&profile), SIGN_IN_TIMEOUT).await
    }

    /// Whether a session saved under `profile` waits to be resumed, from its
    /// file alone, so the answer comes at once.
    pub fn has_saved_session(profile: &str) -> bool {
        Client::has_saved_session(profile)
    }

    /// Signs in, forwarding interactive prompts and always emitting `Finished`.
    pub async fn sign_in(request: LoginRequest, mut events: mpsc::Sender<SignInEvent>) {
        let given = Supplied {
            totp: request.totp.is_some(),
            mailbox_password: request.mailbox_password.is_some(),
        };
        let answered_totp = Arc::new(AtomicBool::new(false));
        let answered_mailbox_password = Arc::new(AtomicBool::new(false));
        let prompter = Prompter::new(events.clone());
        let options = LoginOptions {
            username: request.username,
            password: request.password,
            totp: request.totp,
            mailbox_password: request.mailbox_password,
            profile: request.profile,
            base_url: None,
            app_version: None,
            user_agent: Some(user_agent()),
            hv: Some(prompter.human_verification()),
        };
        // The mailbox password is asked for in the same sign-in, so the
        // password and the code are not asked for, and checked, again.
        let login_prompts = LoginPrompts {
            totp: Some(prompter.totp(answered_totp.clone())),
            mailbox_password: Some(prompter.mailbox_password(answered_mailbox_password.clone())),
        };

        let outcome = match within(
            SIGN_IN_TIMEOUT,
            &prompter.prompt_time,
            Client::login_with_prompts(options, login_prompts),
        )
        .await
        {
            Some(Ok(client)) => SignInOutcome::Authenticated(Arc::new(Self::from_client(client))),
            Some(Err(_)) if prompter.cancelled.load(Ordering::Relaxed) => SignInOutcome::Cancelled,
            Some(Err(error)) => {
                let supplied = Supplied {
                    totp: given.totp || answered_totp.load(Ordering::Relaxed),
                    mailbox_password: given.mailbox_password
                        || answered_mailbox_password.load(Ordering::Relaxed),
                };
                map_sign_in_error(error, supplied)
            }
            None => SignInOutcome::Failed(AuthError::TimedOut),
        };
        let _ = events.send(SignInEvent::Finished(outcome)).await;
        // The client keeps the verification resolver; closing the channel ends
        // the sign-in stream, and a later challenge is reported as Proton's
        // requirement, as it is for a resumed session.
        events.close_channel();
    }

    /// Waits for local cleanup; core bounds only the remote revocation.
    /// Whether Proton confirmed it changes nothing here: either way this
    /// device has forgotten the session.
    pub async fn logout(&self) -> Result<(), AuthError> {
        self.client
            .logout()
            .await
            .map(|_revoked| ())
            .map_err(|_| AuthError::LogoutUnavailable)
    }
}
