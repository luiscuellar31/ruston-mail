use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Semaphore;

use futures::SinkExt;
use futures::channel::{mpsc, oneshot};
use ruston_core::model::enums::{human_verification, label_ids, message_flag};
use ruston_core::model::message::Attachment;
use ruston_core::{
    Client, Conversation, ConversationMessage, Error, FullMessage, HvChallenge, HvResolver, Label,
    LabelCount, LoginOptions, LoginPrompts, MailboxPasswordPrompt, MessageMetadata, Recipient,
    RecipientProtection, SearchOpts, SendOptions, TotpPrompt, UnreadableMessage, Verdict,
};
use secrecy::SecretString;

use super::outgoing::{Outgoing, OutgoingKind, Protection, ProtectionCheck, SendError};
use super::threading::{self, MessageFacts, OwnAddresses};
use super::{
    AuthError, ConversationDetail, ConversationPage, ConversationSummary, Folder, IncomingMail,
    LoginRequest, MailAddress, MailAttachment, MailFolder, MailMessage, MailboxCounts,
    MailboxError, MessageBody, SummaryKind, html,
};
use super::{INSPECTION_CONCURRENCY, MailAction};

/// Conversations requested per page from Proton.
pub const PAGE_SIZE: u32 = 50;

/// Longest wait for any Proton request a view is waiting on. Without it a
/// stuck call leaves the view loading forever, with no way back.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Saving an attachment fetches its message and the sender's keys, then the
/// file itself, which may take core's whole transfer deadline.
const ATTACHMENT_SAVE_TIMEOUT: Duration = ruston_core::api::attachments::ATTACHMENT_REQUEST_TIMEOUT
    .saturating_add(REQUEST_TIMEOUT.saturating_mul(2));
/// A complete authentication attempt must eventually release its credentials
/// even when the server stops responding. The time a person spends on a
/// prompt, solving a CAPTCHA or finding a code, does not count.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(120);
/// Sending is several requests: a draft, a key for every recipient, then the
/// message. It is given longer than a request that only reads; uploads add
/// their own time (see `send_deadline`).
const SEND_TIMEOUT: Duration = Duration::from_secs(90);

/// Per-inspection deadline, including semaphore wait; timeouts keep Proton grouping.
const INSPECTION_BUDGET: Duration = Duration::from_secs(15);

const PROFILE: &str = ruston_core::DEFAULT_SESSION_PROFILE;
const MAILBOX_PASSWORD_REQUIRED: &str = "this account uses a separate mailbox password";
const SECURITY_KEY_REQUIRED: &str = "FIDO2/WebAuthn";
const USER_KEY_UNLOCK_FAILED: &str = "no user key could be unlocked";
const SESSION_ADDRESS_MISMATCH: &str = "does not match the API address it was created for";
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

pub struct ProtonMailService {
    client: Client,
    email: Option<String>,
    own_addresses: OwnAddresses,
    inspection_limiter: Arc<Semaphore>,
}

impl ProtonMailService {
    pub async fn resume() -> ResumeOutcome {
        resume_within(Client::resume(PROFILE), SIGN_IN_TIMEOUT).await
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
            profile: PROFILE.to_owned(),
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

    pub fn email(&self) -> Option<&str> {
        self.email.as_deref()
    }

    /// The Proton account signed in, the same from one session to the next.
    pub fn user_id(&self) -> &str {
        self.client.user_id()
    }

    /// Reads metadata without downloading message bodies or resolving sender keys.
    pub async fn recent_inbox(&self, limit: u32) -> Result<Vec<IncomingMail>, MailboxError> {
        let (_, messages) =
            timed(
                self.client
                    .list_messages(label_id(&Folder::INBOX), 0, limit, false),
            )
            .await?;
        Ok(messages
            .into_iter()
            .map(|message| IncomingMail {
                id: message.id,
                time: message.time,
                unread: message.unread != 0,
                sender: if message.sender.name.trim().is_empty() {
                    message.sender.address
                } else {
                    message.sender.name
                },
                subject: message.subject,
            })
            .collect())
    }

    /// Lists a zero-based page and identifies conversations qualifying for progressive thread inspection.
    pub async fn list_conversations(
        &self,
        folder: &Folder,
        page: u32,
        page_size: u32,
    ) -> Result<ConversationPage, MailboxError> {
        let (total, conversations) =
            timed(
                self.client
                    .list_conversations(label_id(folder), page, page_size, false),
            )
            .await?;

        let mut inspect_candidates = Vec::new();
        let mut summaries = Vec::with_capacity(conversations.len());

        for conversation in conversations {
            let senders: Vec<String> = conversation
                .senders
                .iter()
                .map(|sender| threading::normalize_address(&sender.address))
                .collect();
            if threading::needs_inspection(conversation.num_messages, &senders, &self.own_addresses)
            {
                inspect_candidates.push(conversation.id.clone());
            }
            summaries.push(summarize(conversation, folder));
        }

        Ok(ConversationPage {
            conversations: summaries,
            total,
            inspect_candidates,
        })
    }

    /// Checks whether an ambiguous multi-message conversation is only repeated
    /// inbound mail. Returns `Some(split_rows)` if proven independent inbound messages,
    /// or `None` if grouping should be kept or inspection failed/timed out.
    pub async fn inspect_conversation(
        &self,
        conversation_id: &str,
        folder: &Folder,
    ) -> Result<Option<Vec<ConversationSummary>>, MailboxError> {
        let inspection = limited_inspection(
            &self.inspection_limiter,
            INSPECTION_BUDGET,
            self.client.conversation_messages(conversation_id),
        )
        .await;
        inspection_outcome(inspection, folder, &self.own_addresses)
    }

    /// Lists account-owned folders and labels for the sidebar.
    pub async fn list_folders(&self) -> Result<Vec<Folder>, MailboxError> {
        // Neither request needs the other, so both are in flight at once.
        let (folders, labels) = futures::try_join!(
            timed(self.client.list_folders()),
            timed(self.client.list_labels())
        )?;

        Ok(custom_places(folders, Folder::custom)
            .into_iter()
            .chain(custom_places(labels, Folder::label))
            .collect())
    }

    /// Sends a message. Proton answers with the sent message's ID, which
    /// nothing here uses yet.
    pub async fn send(&self, outgoing: &Outgoing) -> Result<(), SendError> {
        let options = SendOptions {
            to: outgoing.to.clone(),
            cc: outgoing.cc.clone(),
            bcc: outgoing.bcc.clone(),
            subject: outgoing.subject.clone(),
            body: outgoing.wire_body(),
            html: outgoing.is_html(),
            attachments: outgoing.attachments.clone(),
            // ruston-core picks the sender: the primary address, or for an
            // answer the address the original reached.
            // Scheduling and self-destruct are not offered yet.
            ..SendOptions::default()
        };

        // Core addresses answers, writes their subjects and keys forwarded
        // attachments for the draft; Proton threads them.
        let call = async {
            match &outgoing.kind {
                OutgoingKind::New => self.client.send(&options).await,
                OutgoingKind::Reply {
                    message_id,
                    everyone,
                } => self.client.reply(message_id, *everyone, &options).await,
                OutgoingKind::Forward { message_id } => {
                    self.client.forward(message_id, &options).await
                }
            }
        };

        timed_with(
            call,
            send_deadline(outgoing.attachments.len()),
            SendError::Unconfirmed,
            |error| match error {
                Error::SendUnconfirmed { .. } => SendError::Unconfirmed,
                other => SendError::from_attachment(&other)
                    .unwrap_or_else(|| SendError::Mailbox(map_mailbox_error(other))),
            },
        )
        .await
        .map(|_| ())
    }

    /// Asks Proton for each address's keys, one request at a time. A failed
    /// lookup is reported for its own address and does not stop the others.
    pub async fn protection(&self, addresses: &[String]) -> Vec<ProtectionCheck> {
        let mut checks = Vec::with_capacity(addresses.len());
        for address in addresses {
            let protection =
                timed(self.client.recipient_protection(address))
                    .await
                    .map(|protection| match protection {
                        RecipientProtection::EndToEnd => Protection::EndToEnd,
                        RecipientProtection::Pgp => Protection::Pgp,
                        RecipientProtection::Unencrypted => Protection::Unencrypted,
                    });
            checks.push((address.clone(), protection));
        }
        checks
    }

    pub async fn conversation_counts(
        &self,
        folders: &[Folder],
    ) -> Result<MailboxCounts, MailboxError> {
        let counts = timed(self.client.conversation_counts()).await?;

        Ok(map_counts(&counts, folders))
    }

    /// Decrypts a conversation of about `message_count` messages oldest-first;
    /// ruston-core sanitizes its HTML. A message that cannot be decrypted
    /// stays in the thread without its body.
    pub async fn conversation_detail(
        &self,
        id: &str,
        message_count: u32,
    ) -> Result<ConversationDetail, MailboxError> {
        let (conversation, messages) = timed_with(
            self.client.read_conversation_each(id),
            conversation_deadline(message_count),
            MailboxError::Connection,
            map_mailbox_error,
        )
        .await?;
        let labels = conversation
            .labels
            .into_iter()
            .map(|label| label.id)
            .collect();

        Ok(ConversationDetail {
            id: id.to_owned(),
            subject: non_blank(&conversation.subject),
            labels,
            messages: messages
                .into_iter()
                .map(|message| conversation_message(message, &self.own_addresses))
                .collect(),
        })
    }

    /// Fetches and decrypts one message that the list shows on its own. One
    /// that cannot be opened shows as it does in a conversation.
    pub async fn message_detail(&self, id: &str) -> Result<ConversationDetail, MailboxError> {
        let message = timed_with(
            self.client.read_message_or_unreadable(id),
            conversation_deadline(1),
            MailboxError::Connection,
            map_mailbox_error,
        )
        .await?;
        Ok(single_message_detail(id, message, &self.own_addresses))
    }

    /// Searches one page across all folders.
    pub async fn search(
        &self,
        query: &str,
        page: u32,
        page_size: u32,
    ) -> Result<ConversationPage, MailboxError> {
        let options = SearchOpts {
            keyword: Some(query.to_owned()),
            ..SearchOpts::default()
        };
        let (total, found) = timed(
            self.client
                .search_conversations_page(&options, page, page_size),
        )
        .await?;

        // Results span folders, so there is no one folder to read them from.
        // Inbox is the perspective that describes incoming mail by its sender.
        let conversations: Vec<ConversationSummary> = found
            .into_iter()
            .map(|conversation| summarize(conversation, &Folder::INBOX))
            .collect();

        Ok(ConversationPage {
            total,
            conversations,
            inspect_candidates: Vec::new(),
        })
    }

    /// Fetches and decrypts one attachment, returning the name the sender gave
    /// it and its contents. Nothing is written to disk here.
    pub async fn download_attachment(
        &self,
        message_id: &str,
        attachment_id: &str,
    ) -> Result<(String, Vec<u8>), MailboxError> {
        timed_with(
            self.client.download_attachment(message_id, attachment_id),
            ATTACHMENT_SAVE_TIMEOUT,
            MailboxError::Connection,
            map_mailbox_error,
        )
        .await
    }

    /// Applies a user action to one row: its whole conversation, or the single
    /// message the list shows on its own. Moves relabel; nothing is deleted.
    pub async fn apply_action(
        &self,
        kind: SummaryKind,
        id: &str,
        context: Option<&Folder>,
        action: MailAction,
    ) -> Result<(), MailboxError> {
        let ids = [id.to_owned()];
        let client = &self.client;
        let call = async {
            match (kind, action_call(&action)) {
                (SummaryKind::Conversation, ActionCall::Move(label)) => {
                    client.move_conversations(&ids, label).await
                }
                (SummaryKind::Conversation, ActionCall::MarkRead(read)) => {
                    client
                        .mark_conversations_read(&ids, read, context_label(context))
                        .await
                }
                (SummaryKind::Conversation, ActionCall::Star(starred)) => {
                    client.star_conversations(&ids, starred).await
                }
                (SummaryKind::Message, ActionCall::Move(label)) => {
                    client.move_messages(&ids, label).await
                }
                (SummaryKind::Message, ActionCall::MarkRead(read)) => {
                    client.mark_messages_read(&ids, read).await
                }
                (SummaryKind::Message, ActionCall::Star(starred)) => {
                    client.star_messages(&ids, starred).await
                }
                // Proton puts a conversation's label on every message in it.
                (SummaryKind::Conversation, ActionCall::Label(label, true)) => {
                    client.label_conversations(&ids, label).await
                }
                (SummaryKind::Conversation, ActionCall::Label(label, false)) => {
                    client.unlabel_conversations(&ids, label).await
                }
                (SummaryKind::Message, ActionCall::Label(label, true)) => {
                    client.apply_label(&ids, label).await
                }
                (SummaryKind::Message, ActionCall::Label(label, false)) => {
                    client.remove_label(&ids, label).await
                }
            }
        };

        timed(call).await
    }

    fn from_client(client: Client) -> Self {
        let email = client.primary_email().map(str::to_owned);
        let own_addresses = client
            .addresses()
            .iter()
            .map(|address| threading::normalize_address(&address.email))
            .collect();

        Self {
            client,
            email,
            own_addresses,
            inspection_limiter: Arc::new(Semaphore::new(INSPECTION_CONCURRENCY)),
        }
    }
}

/// Admission and request share a deadline; dropping this future releases the
/// permit and the request, even when cancelled before admission.
async fn limited_inspection<T>(
    limiter: &Semaphore,
    within: Duration,
    call: impl Future<Output = ruston_core::Result<T>>,
) -> Result<T, MailboxError> {
    tokio::time::timeout(within, async {
        let _permit = limiter
            .acquire()
            .await
            .map_err(|_| MailboxError::Unavailable)?;
        call.await.map_err(map_mailbox_error)
    })
    .await
    .map_err(|_| MailboxError::Unavailable)?
}

/// How long a send with `attachments` new files may take. Each upload gets
/// core's own transfer deadline on top of the other steps, so a stalled upload
/// ends with core's error, which removes the draft and says nothing was sent.
/// Cutting it off here instead could only report an unknown outcome.
fn send_deadline(attachments: usize) -> Duration {
    let uploads = u32::try_from(attachments).unwrap_or(u32::MAX);
    SEND_TIMEOUT.saturating_add(
        ruston_core::api::attachments::ATTACHMENT_REQUEST_TIMEOUT.saturating_mul(uploads),
    )
}

/// How long opening a conversation of `messages` may take: a request's time
/// for the conversation, and one for each message, whose body and sender keys
/// may each need one. A fixed budget would never open a long thread on a slow
/// connection, and retrying starts over.
fn conversation_deadline(messages: u32) -> Duration {
    REQUEST_TIMEOUT.saturating_mul(messages.max(1).saturating_add(1))
}

/// Runs one Proton request under `REQUEST_TIMEOUT`. A call that outlives it
/// fails as a connection error, which every view offers to retry.
async fn timed<T>(call: impl Future<Output = ruston_core::Result<T>>) -> Result<T, MailboxError> {
    timed_with(
        call,
        REQUEST_TIMEOUT,
        MailboxError::Connection,
        map_mailbox_error,
    )
    .await
}

/// Applies the timeout to calls with non-mailbox error types.
async fn timed_with<T, E>(
    call: impl Future<Output = ruston_core::Result<T>>,
    within: Duration,
    timed_out: E,
    failed: impl FnOnce(Error) -> E,
) -> Result<T, E> {
    tokio::time::timeout(within, call)
        .await
        .map_err(|_| timed_out)?
        .map_err(failed)
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
struct Supplied {
    totp: bool,
    mailbox_password: bool,
}

/// Asks the person signing in, through the sign-in's events, for what the
/// sign-in needs. Clones ask through the same events, cancel the same
/// sign-in and count their time on the same clock.
#[derive(Clone)]
struct Prompter {
    events: mpsc::Sender<SignInEvent>,
    /// Set when a prompt is abandoned, which ends the sign-in as cancelled.
    cancelled: Arc<AtomicBool>,
    /// The time spent on prompts, which the sign-in deadline leaves out.
    prompt_time: Arc<PromptTime>,
}

impl Prompter {
    fn new(events: mpsc::Sender<SignInEvent>) -> Self {
        Self {
            events,
            cancelled: Arc::default(),
            prompt_time: Arc::default(),
        }
    }

    /// Asks for the TOTP code only when Proton requires it, after any human
    /// verification, so the code is still fresh. `answered` is set once the
    /// person gives one.
    fn totp(&self, answered: Arc<AtomicBool>) -> TotpPrompt {
        self.secret(SignInPrompt::Totp, "no TOTP code was given", answered)
    }

    /// Asks for the mailbox password only when the account has a separate
    /// one, once its password and any code are accepted. `answered` is set
    /// once the person gives it.
    fn mailbox_password(&self, answered: Arc<AtomicBool>) -> MailboxPasswordPrompt {
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
    fn human_verification(&self) -> HvResolver {
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
struct PromptTime(Mutex<PromptClock>);

#[derive(Default)]
struct PromptClock {
    spent: Duration,
    /// When the prompt being answered now was shown.
    since: Option<tokio::time::Instant>,
}

impl PromptTime {
    /// Counts the time until the returned guard is dropped, as a prompt's.
    fn start(&self) -> Answering<'_> {
        self.0.lock().expect("prompt time lock poisoned").since = Some(tokio::time::Instant::now());
        Answering(self)
    }

    /// The time set aside by `now`, a prompt still open included.
    fn spent(&self, now: tokio::time::Instant) -> Duration {
        let clock = self.0.lock().expect("prompt time lock poisoned");
        clock.spent
            + clock
                .since
                .map_or(Duration::ZERO, |since| now.saturating_duration_since(since))
    }
}

/// A prompt being answered; dropping it stops counting its time.
struct Answering<'a>(&'a PromptTime);

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
async fn within<F: Future>(
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

/// The Proton operation behind a mailbox action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActionCall<'a> {
    /// Relabel into a folder; never a permanent delete. The id borrows from
    /// the action, since a folder the account made carries its own.
    Move(&'a str),
    MarkRead(bool),
    Star(bool),
    /// Add or remove a label the account made, leaving the folder alone.
    Label(&'a str, bool),
}

fn action_call(action: &MailAction) -> ActionCall<'_> {
    match action {
        MailAction::MoveTo(folder) => ActionCall::Move(label_id(folder)),
        MailAction::SetUnread(unread) => ActionCall::MarkRead(!unread),
        MailAction::SetStarred(starred) => ActionCall::Star(*starred),
        MailAction::SetLabel { label, on } => ActionCall::Label(label_id(label), *on),
    }
}

/// Folder context for row actions; global search rows use All Mail.
fn context_label(context: Option<&Folder>) -> &str {
    context.map_or(label_ids::ALL_MAIL, label_id)
}

/// Maps folders to Proton wire identifiers.
fn label_id(folder: &Folder) -> &str {
    match folder {
        Folder::Custom { id, .. } => id,
        Folder::System(folder) => system_label_id(*folder),
    }
}

fn system_label_id(folder: MailFolder) -> &'static str {
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

fn summarize(conversation: Conversation, folder: &Folder) -> ConversationSummary {
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
    let correspondents = match folder.system() {
        // Only Proton's own outgoing folders describe mail by who it went to.
        Some(MailFolder::Sent | MailFolder::Drafts) => &conversation.recipients,
        _ => &conversation.senders,
    };
    let participants = conversation
        .senders
        .iter()
        .chain(&conversation.recipients)
        .map(mail_address)
        .collect();

    ConversationSummary {
        subject: non_blank(&conversation.subject),
        correspondents: display_names(correspondents),
        participants,
        preview: None,
        time: known_time(time),
        unread: unread > 0,
        starred,
        message_count: u32::try_from(conversation.num_messages).unwrap_or(0),
        has_attachments: conversation.num_attachments > 0,
        id: conversation.id,
        kind: SummaryKind::Conversation,
    }
}

/// What a conversation's inspection means for the list: the rows to split it
/// into, `None` to keep Proton's grouping, or the error that ends the page
/// load.
fn inspection_outcome(
    inspection: Result<Vec<MessageMetadata>, MailboxError>,
    folder: &Folder,
    own: &OwnAddresses,
) -> Result<Option<Vec<ConversationSummary>>, MailboxError> {
    match inspection {
        Ok(messages) => Ok(inspect_messages(messages, folder, own)),
        // An expired session ends the page load like any other request.
        Err(MailboxError::SessionExpired) => Err(MailboxError::SessionExpired),
        // Any other failure, or no answer in time, keeps Proton's grouping.
        Err(_) => Ok(None),
    }
}

/// Inspects message metadata to determine if it is repeated inbound mail.
fn inspect_messages(
    messages: Vec<MessageMetadata>,
    folder: &Folder,
    own: &OwnAddresses,
) -> Option<Vec<ConversationSummary>> {
    let facts: Vec<MessageFacts> = messages
        .iter()
        .map(|message| MessageFacts {
            sender: threading::normalize_address(&message.sender.address),
            answered: [
                message_flag::REPLIED,
                message_flag::REPLIED_ALL,
                message_flag::FORWARDED,
            ]
            .into_iter()
            .any(|flag| message_flag::has(message.flags, flag)),
        })
        .collect();
    if !threading::is_repeated_inbound(&facts, own) {
        return None;
    }

    let mut rows: Vec<_> = messages
        .into_iter()
        .filter(|message| message.label_ids.iter().any(|id| id == label_id(folder)))
        .map(message_summary)
        .collect();
    if rows.is_empty() {
        return None;
    }
    rows.sort_by_key(|row| std::cmp::Reverse(row.time));
    Some(rows)
}

/// A row for one message split out of a conversation. Only inbound mail is
/// ever split, so the sender is the correspondent in every folder.
fn message_summary(message: MessageMetadata) -> ConversationSummary {
    let correspondents = display_names(std::slice::from_ref(&message.sender));
    let participants = std::iter::once(&message.sender)
        .chain(&message.to_list)
        .chain(&message.cc_list)
        .map(mail_address)
        .collect();

    ConversationSummary {
        subject: non_blank(&message.subject),
        correspondents,
        participants,
        preview: None,
        time: known_time(message.time),
        unread: message.unread != 0,
        starred: message.label_ids.iter().any(|id| id == label_ids::STARRED),
        message_count: 1,
        has_attachments: message.num_attachments > 0,
        id: message.id,
        kind: SummaryKind::Message,
    }
}

fn mail_attachment(attachment: Attachment) -> MailAttachment {
    MailAttachment {
        id: attachment.id,
        name: attachment.name,
        size: attachment.size,
    }
}

/// Text Proton sends without its surrounding spaces, or `None` when nothing
/// is left.
fn non_blank(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// A time Proton sends, or `None` for the 0 it sends when the time is
/// unknown.
fn known_time(seconds: i64) -> Option<i64> {
    (seconds > 0).then_some(seconds)
}

fn mail_address(person: &Recipient) -> MailAddress {
    MailAddress {
        name: non_blank(&person.name),
        address: person.address.trim().to_owned(),
    }
}

fn mail_message(message: FullMessage, own: &OwnAddresses) -> MailMessage {
    let FullMessage {
        meta,
        body,
        mime_type,
        verdict,
        attachments,
        reply_tos,
    } = message;
    let body = if ruston_core::html::is_html_mime(&mime_type) {
        MessageBody::Rich(html::parse(&body))
    } else {
        MessageBody::PlainText(body)
    };

    MailMessage {
        // Inline parts are the body's own images, which Ruston Mail never
        // loads; only real attachments belong in the reader.
        attachments: attachments
            .into_iter()
            .filter(|attachment| !attachment.is_inline())
            .map(mail_attachment)
            .collect(),
        body,
        verdict,
        ..message_without_body(meta, &reply_tos, own)
    }
}

/// A message that could not be decrypted keeps its headers, so it can still
/// be answered.
fn unreadable_mail_message(message: UnreadableMessage, own: &OwnAddresses) -> MailMessage {
    message_without_body(message.meta, &message.reply_tos, own)
}

/// One message of the reader: opened, or with its headers and a notice.
fn conversation_message(message: ConversationMessage, own: &OwnAddresses) -> MailMessage {
    match message {
        ConversationMessage::Opened(message) => mail_message(message, own),
        ConversationMessage::Unreadable(unreadable) => unreadable_mail_message(unreadable, own),
    }
}

/// The reader's view of a message the list shows on its own.
fn single_message_detail(
    id: &str,
    message: ConversationMessage,
    own: &OwnAddresses,
) -> ConversationDetail {
    let meta = match &message {
        ConversationMessage::Opened(message) => &message.meta,
        ConversationMessage::Unreadable(message) => &message.meta,
    };
    let subject = non_blank(&meta.subject);
    let labels = meta.label_ids.clone();
    ConversationDetail {
        id: id.to_owned(),
        subject,
        labels,
        messages: vec![conversation_message(message, own)],
    }
}

/// Who sent a message, to whom, when, and where an answer goes, with no
/// body or attachments.
fn message_without_body(
    meta: MessageMetadata,
    reply_tos: &[Recipient],
    own: &OwnAddresses,
) -> MailMessage {
    let own_list: Vec<String> = own.iter().cloned().collect();
    let reply = ruston_core::reply_recipients(&meta, reply_tos, &own_list);
    let reply_redirected = ruston_core::reply_redirected(&meta, reply_tos);

    MailMessage {
        attachments: Vec::new(),
        sender: mail_address(&meta.sender),
        to: meta.to_list.iter().map(mail_address).collect(),
        cc: meta.cc_list.iter().map(mail_address).collect(),
        bcc: meta.bcc_list.iter().map(mail_address).collect(),
        reply_to: reply.to,
        reply_all_cc: reply.cc,
        reply_redirected,
        time: known_time(meta.time),
        unread: meta.unread != 0,
        body: MessageBody::Unreadable,
        verdict: Verdict::Unverified,
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

/// Builds sidebar counts for system and known account folders.
fn map_counts(counts: &[LabelCount], folders: &[Folder]) -> MailboxCounts {
    let mut counts_by_id = HashMap::with_capacity(counts.len());
    for count in counts {
        counts_by_id.entry(count.label_id.as_str()).or_insert(count);
    }

    MailFolder::ALL
        .into_iter()
        .map(Folder::System)
        .chain(folders.iter().cloned())
        .filter_map(|folder| {
            let count = counts_by_id.get(label_id(&folder))?;
            Some((folder, u32::try_from(count.unread).unwrap_or(0)))
        })
        .collect()
}

/// Maps one Proton folder/label batch into sidebar entries.
fn custom_places(labels: Vec<Label>, make: fn(String, String) -> Folder) -> Vec<Folder> {
    labels
        .into_iter()
        .map(|label| make(label.id, label.name))
        .collect()
}

fn map_mailbox_error(error: Error) -> MailboxError {
    match error {
        Error::Unauthorized => MailboxError::SessionExpired,
        Error::Api(error) if error.is_unauthorized() => MailboxError::SessionExpired,
        Error::Http(_) => MailboxError::Connection,
        Error::AttachmentVerificationFailed => MailboxError::AttachmentVerificationFailed,
        Error::Api(_) | Error::HumanVerification(_) => MailboxError::Service,
        // Loading or saving the session while renewing its tokens: a locked
        // password storage, or a session saved for another server.
        Error::Session(_) => MailboxError::SessionUnavailable,
        _ => MailboxError::Unavailable,
    }
}

/// What resuming the saved session comes to, with `deadline` to finish. A
/// session Proton refuses is signed out; any other failure, running out of
/// time included, keeps it saved for another try.
async fn resume_within(
    resume: impl Future<Output = ruston_core::Result<Client>>,
    deadline: Duration,
) -> ResumeOutcome {
    let Ok(resumed) = tokio::time::timeout(deadline, resume).await else {
        return ResumeOutcome::Failed(AuthError::TimedOut);
    };
    match resumed {
        Ok(client) => {
            ResumeOutcome::Authenticated(Arc::new(ProtonMailService::from_client(client)))
        }
        Err(Error::Unauthorized | Error::Crypto(_) | Error::Srp(_)) => ResumeOutcome::SignedOut,
        Err(Error::Api(error)) if matches!(error.http_status, 401 | 403) => {
            ResumeOutcome::SignedOut
        }
        Err(error) => ResumeOutcome::Failed(map_auth_error(error)),
    }
}

/// What a human verification that ended a sign-in or a session asks of the
/// person: signing in again answers a CAPTCHA, while any other method is one
/// Ruston cannot answer.
fn verification_error(challenge: &HvChallenge) -> AuthError {
    if challenge.offers_captcha() {
        AuthError::HumanVerificationRequired
    } else {
        AuthError::HumanVerificationUnsupported
    }
}

fn map_sign_in_error(error: Error, supplied: Supplied) -> SignInOutcome {
    match error {
        Error::Other(message) if message.contains(MAILBOX_PASSWORD_REQUIRED) => {
            SignInOutcome::NeedsMailboxPassword
        }
        Error::Other(message) if message.contains(SECURITY_KEY_REQUIRED) => {
            SignInOutcome::Failed(AuthError::SecurityKeyUnsupported)
        }
        Error::HumanVerification(challenge) => {
            SignInOutcome::Failed(verification_error(&challenge))
        }
        Error::Unauthorized if supplied.totp => SignInOutcome::Failed(AuthError::InvalidTotp),
        Error::Unauthorized => SignInOutcome::Failed(AuthError::InvalidCredentials),
        Error::Api(error) if supplied.totp && mentions_totp(&error.message) => {
            SignInOutcome::Failed(AuthError::InvalidTotp)
        }
        Error::Api(error)
            if matches!(error.http_status, 401 | 403) || mentions_credentials(&error.message) =>
        {
            SignInOutcome::Failed(AuthError::InvalidCredentials)
        }
        Error::Crypto(message)
            if supplied.mailbox_password && message.contains(USER_KEY_UNLOCK_FAILED) =>
        {
            SignInOutcome::Failed(AuthError::InvalidMailboxPassword)
        }
        error => SignInOutcome::Failed(map_auth_error(error)),
    }
}

fn map_auth_error(error: Error) -> AuthError {
    match error {
        Error::HumanVerification(challenge) => verification_error(&challenge),
        Error::Http(_) => AuthError::Connection,
        Error::Session(message) if message.contains(SESSION_ADDRESS_MISMATCH) => {
            AuthError::SessionAddressMismatch
        }
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
    use ruston_core::ApiError;
    use ruston_core::model::ConversationLabel;

    use super::*;

    #[tokio::test(start_paused = true)]
    async fn the_sign_in_deadline_leaves_out_the_time_spent_on_prompts() {
        let prompts = PromptTime::default();
        // A CAPTCHA that takes ten minutes between two quick requests.
        let sign_in = async {
            tokio::time::sleep(Duration::from_secs(30)).await;
            {
                let _answering = prompts.start();
                tokio::time::sleep(Duration::from_secs(600)).await;
            }
            tokio::time::sleep(Duration::from_secs(30)).await;
            "signed in"
        };

        assert_eq!(
            within(SIGN_IN_TIMEOUT, &prompts, sign_in).await,
            Some("signed in")
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_sign_in_runs_out_of_time_once_its_own_time_is_used_up() {
        let prompts = PromptTime::default();
        let sign_in = async {
            {
                let _answering = prompts.start();
                tokio::time::sleep(Duration::from_secs(600)).await;
            }
            // Proton stops answering once the prompt is solved.
            std::future::pending::<()>().await;
        };
        let started = tokio::time::Instant::now();

        assert_eq!(within(SIGN_IN_TIMEOUT, &prompts, sign_in).await, None);
        assert_eq!(
            started.elapsed(),
            Duration::from_secs(600) + SIGN_IN_TIMEOUT
        );

        // Without a prompt, the whole deadline is the sign-in's own.
        let started = tokio::time::Instant::now();
        let silent = std::future::pending::<()>();
        assert_eq!(
            within(SIGN_IN_TIMEOUT, &PromptTime::default(), silent).await,
            None
        );
        assert_eq!(started.elapsed(), SIGN_IN_TIMEOUT);
    }

    #[tokio::test]
    async fn inspection_deadline_includes_waiting_for_a_permit() {
        let limiter = Semaphore::new(1);
        let _permit = limiter.acquire().await.unwrap();
        let called = AtomicBool::new(false);
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            limited_inspection(&limiter, Duration::from_millis(20), async {
                called.store(true, Ordering::SeqCst);
                Ok(())
            }),
        )
        .await
        .expect("admission must also have a deadline");
        assert_eq!(result, Err(MailboxError::Unavailable));
        assert!(!called.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn cancelled_inspection_releases_request_and_permit() {
        use futures::FutureExt;

        let limiter = Semaphore::new(1);
        let client = Arc::new(());
        let weak = Arc::downgrade(&client);
        let mut inspection = Box::pin(limited_inspection(
            &limiter,
            INSPECTION_BUDGET,
            async move {
                let _client = client;
                std::future::pending::<ruston_core::Result<()>>().await
            },
        ));
        assert!(inspection.as_mut().now_or_never().is_none());
        assert_eq!(limiter.available_permits(), 0);
        drop(inspection);
        assert_eq!(limiter.available_permits(), 1);
        assert!(weak.upgrade().is_none());
    }

    #[tokio::test]
    async fn inspection_preserves_expired_session_errors() {
        let limiter = Semaphore::new(1);
        let result = limited_inspection(&limiter, INSPECTION_BUDGET, async {
            Err::<(), _>(Error::Unauthorized)
        })
        .await;
        assert_eq!(result, Err(MailboxError::SessionExpired));
        assert_eq!(limiter.available_permits(), 1);
    }

    #[test]
    fn a_message_on_its_own_that_cannot_be_opened_shows_as_in_a_conversation() {
        let unreadable = UnreadableMessage {
            meta: MessageMetadata {
                id: "m1".into(),
                subject: " Invoice ".into(),
                label_ids: vec!["0".into()],
                sender: person("Boss", "boss@corp.example"),
                ..Default::default()
            },
            reply_tos: Vec::new(),
            error: Error::Crypto("no key of the account opens it".into()),
        };

        let detail = single_message_detail(
            "m1",
            ConversationMessage::Unreadable(unreadable),
            &OwnAddresses::new(),
        );

        assert_eq!(detail.id, "m1");
        assert_eq!(detail.subject.as_deref(), Some("Invoice"));
        assert_eq!(detail.labels, ["0"]);
        let [message] = &detail.messages[..] else {
            panic!("the message itself");
        };
        assert_eq!(message.id, "m1");
        assert!(matches!(message.body, MessageBody::Unreadable));
    }

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

        let own = OwnAddresses::from(["demo@example.com".to_owned()]);
        let message = mail_message(
            FullMessage {
                meta,
                body: "<p>Hello &amp; welcome</p>".into(),
                mime_type: "text/html".into(),
                attachments: Vec::new(),
                verdict: ruston_core::Verdict::Invalid,
                reply_tos: vec![person("", "replies@example.com")],
            },
            &own,
        );

        assert_eq!(message.id, "message");
        assert_eq!(message.verdict, ruston_core::Verdict::Invalid);
        assert_eq!(message.sender.name, None);
        assert_eq!(message.sender.address, "alex@example.com");
        // To, Cc and Bcc stay apart, so the reader can say which is which.
        let addresses = |people: &[MailAddress]| -> Vec<String> {
            people.iter().map(|person| person.address.clone()).collect()
        };
        assert_eq!(addresses(&message.to), ["demo@example.com"]);
        assert_eq!(addresses(&message.cc), ["team@example.org"]);
        assert_eq!(addresses(&message.bcc), ["archive@example.net"]);
        // The reader knows where core will send a reply before it is written.
        assert_eq!(message.reply_to, ["replies@example.com"]);
        assert_eq!(message.reply_all_cc, ["team@example.org"]);
        assert!(message.reply_redirected, "Reply-To differs from the sender");
        assert_eq!(message.time, Some(1_700_000_000));
        let MessageBody::Rich(body) = &message.body else {
            panic!("expected a rich body for HTML");
        };
        assert_eq!(body.plain_text(), "Hello & welcome\n");
    }

    #[test]
    fn plain_text_bodies_are_kept_verbatim() {
        let body = "Line <one>\n\n  indented & raw";

        let message = mail_message(
            FullMessage {
                meta: MessageMetadata::default(),
                body: body.into(),
                mime_type: "text/plain".into(),
                attachments: Vec::new(),
                verdict: ruston_core::Verdict::Unverified,
                reply_tos: Vec::new(),
            },
            &OwnAddresses::new(),
        );

        assert_eq!(message.body, MessageBody::PlainText(body.into()));
        assert_eq!(message.time, None);
    }

    #[test]
    fn an_unreadable_message_keeps_its_headers_and_reply_recipients() {
        let meta = MessageMetadata {
            id: "message".into(),
            sender: person("Alex", "alex@example.com"),
            to_list: vec![person("Demo", "demo@example.com")],
            cc_list: vec![person("", "team@example.org")],
            time: 1_700_000_000,
            ..Default::default()
        };
        let own = OwnAddresses::from(["demo@example.com".to_owned()]);

        let message = unreadable_mail_message(
            UnreadableMessage {
                meta,
                reply_tos: vec![person("", "replies@example.com")],
                error: Error::Crypto("body decrypt failed".into()),
            },
            &own,
        );

        assert_eq!(message.id, "message");
        assert_eq!(message.body, MessageBody::Unreadable);
        assert_eq!(message.verdict, Verdict::Unverified);
        assert!(message.attachments.is_empty());
        assert_eq!(message.sender.address, "alex@example.com");
        assert_eq!(
            (message.to.len(), message.cc.len(), message.bcc.len()),
            (1, 1, 0)
        );
        // Answering still goes where core will send it.
        assert_eq!(message.reply_to, ["replies@example.com"]);
        assert_eq!(message.reply_all_cc, ["team@example.org"]);
        assert!(message.reply_redirected);
        assert_eq!(message.time, Some(1_700_000_000));
    }

    #[test]
    fn every_body_verdict_survives_plain_and_html_conversion() {
        use ruston_core::Verdict;

        for verdict in [
            Verdict::Verified,
            Verdict::Unsigned,
            Verdict::Unverified,
            Verdict::Invalid,
        ] {
            for mime_type in ["text/plain", "text/html"] {
                let message = mail_message(
                    FullMessage {
                        meta: MessageMetadata::default(),
                        body: "body".into(),
                        mime_type: mime_type.into(),
                        attachments: Vec::new(),
                        verdict,
                        reply_tos: Vec::new(),
                    },
                    &OwnAddresses::new(),
                );
                assert_eq!(message.verdict, verdict);
            }
        }
        assert_eq!(
            map_mailbox_error(Error::AttachmentVerificationFailed),
            MailboxError::AttachmentVerificationFailed
        );
    }

    fn captcha_challenge() -> HvChallenge {
        HvChallenge {
            token: "token".into(),
            methods: vec!["captcha".into()],
            web_url: String::new(),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_resume_that_outlasts_its_deadline_keeps_the_session() {
        let outcome = resume_within(std::future::pending(), SIGN_IN_TIMEOUT).await;
        assert!(matches!(
            outcome,
            ResumeOutcome::Failed(AuthError::TimedOut)
        ));
    }

    #[tokio::test]
    async fn a_resume_signs_out_only_when_proton_refuses_the_session() {
        let refused = resume_within(async { Err(Error::Unauthorized) }, SIGN_IN_TIMEOUT).await;
        assert!(matches!(refused, ResumeOutcome::SignedOut));

        let unreachable = resume_within(
            async { Err(Error::Session("keyring get: locked".into())) },
            SIGN_IN_TIMEOUT,
        )
        .await;
        assert!(matches!(
            unreachable,
            ResumeOutcome::Failed(AuthError::SessionUnavailable)
        ));
    }

    #[test]
    fn a_verification_says_whether_signing_in_again_answers_it() {
        let email = HvChallenge {
            methods: vec!["email".into(), "sms".into()],
            ..captcha_challenge()
        };
        let unnamed = HvChallenge {
            methods: Vec::new(),
            ..captcha_challenge()
        };

        for (challenge, expected) in [
            (captcha_challenge(), AuthError::HumanVerificationRequired),
            (unnamed, AuthError::HumanVerificationRequired),
            (email, AuthError::HumanVerificationUnsupported),
        ] {
            let methods = challenge.methods.clone();
            let outcome = map_sign_in_error(
                Error::HumanVerification(challenge.clone()),
                Supplied::default(),
            );
            assert!(
                matches!(outcome, SignInOutcome::Failed(error) if error == expected),
                "{methods:?}"
            );
            assert_eq!(
                map_auth_error(Error::HumanVerification(challenge)),
                expected,
                "{methods:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_challenge_after_sign_in_is_reported_as_for_a_resumed_session() {
        // The client keeps the sign-in's resolver for the whole session, but
        // once the sign-in is over nothing shows its prompt.
        let (events, receiver) = mpsc::channel(1);
        drop(receiver);
        let prompter = Prompter::new(events);
        let error = prompter.human_verification()(captcha_challenge())
            .await
            .unwrap_err();
        assert!(matches!(error, Error::HumanVerification(_)), "{error}");
        assert_eq!(map_mailbox_error(error), MailboxError::Service);
        assert!(!prompter.cancelled.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn an_abandoned_prompt_cancels_the_sign_in() {
        use futures::StreamExt;

        let (events, mut shown) = mpsc::channel(1);
        let prompter = Prompter::new(events.clone());
        let verification = tokio::spawn(prompter.human_verification()(captcha_challenge()));
        let Some(SignInEvent::Prompt(prompt)) = shown.next().await else {
            panic!("no verification prompt");
        };
        prompt.cancel();
        assert!(verification.await.unwrap().is_err());
        assert!(prompter.cancelled.load(Ordering::Relaxed));
        // An abandoned prompt stops counting as one.
        let now = tokio::time::Instant::now();
        assert_eq!(
            prompter.prompt_time.spent(now),
            prompter.prompt_time.spent(now + Duration::from_secs(60))
        );

        let prompter = Prompter {
            cancelled: Arc::default(),
            ..prompter
        };
        let answered = Arc::new(AtomicBool::new(false));
        let code = tokio::spawn(prompter.totp(answered.clone())());
        let Some(SignInEvent::Prompt(prompt)) = shown.next().await else {
            panic!("no TOTP prompt");
        };
        prompt.cancel();
        assert!(code.await.unwrap().is_err());
        assert!(prompter.cancelled.load(Ordering::Relaxed));
        assert!(!answered.load(Ordering::Relaxed));

        let prompter = Prompter::new(events);
        let answered = Arc::new(AtomicBool::new(false));
        let password = tokio::spawn(prompter.mailbox_password(answered.clone())());
        let Some(SignInEvent::Prompt(prompt @ SignInPrompt::MailboxPassword(_))) =
            shown.next().await
        else {
            panic!("no mailbox password prompt");
        };
        prompt.cancel();
        assert!(password.await.unwrap().is_err());
        assert!(prompter.cancelled.load(Ordering::Relaxed));
        assert!(!answered.load(Ordering::Relaxed));
    }

    #[test]
    fn answering_your_own_sent_message_addresses_its_recipients() {
        let meta = MessageMetadata {
            sender: person("Me", "me@proton.me"),
            to_list: vec![person("", "bob@corp.example")],
            flags: message_flag::SENT,
            ..Default::default()
        };
        let message = mail_message(
            FullMessage {
                meta,
                body: "Following up".into(),
                mime_type: "text/plain".into(),
                attachments: Vec::new(),
                verdict: ruston_core::Verdict::Unsigned,
                reply_tos: vec![person("Me", "me@proton.me")],
            },
            &OwnAddresses::from(["me@proton.me".to_owned()]),
        );

        assert_eq!(message.reply_to, ["bob@corp.example"]);
        assert!(!message.reply_redirected, "nothing redirects your own mail");
    }

    #[test]
    fn saving_an_attachment_outlasts_core_s_transfer_deadline() {
        use ruston_core::api::attachments::ATTACHMENT_REQUEST_TIMEOUT;

        // The file itself may take core's whole transfer deadline; the
        // message and the sender's keys come on top of it.
        assert!(ATTACHMENT_SAVE_TIMEOUT >= ATTACHMENT_REQUEST_TIMEOUT + REQUEST_TIMEOUT * 2);
    }

    #[test]
    fn a_long_conversation_gets_time_for_every_message() {
        // One message still fits a request for it and one for its sender's keys.
        assert_eq!(conversation_deadline(0), REQUEST_TIMEOUT * 2);
        assert_eq!(conversation_deadline(1), REQUEST_TIMEOUT * 2);
        for messages in [2, 10, 50] {
            assert!(
                conversation_deadline(messages) > REQUEST_TIMEOUT * messages,
                "{messages} messages share too little time"
            );
        }
        assert!(conversation_deadline(u32::MAX) >= conversation_deadline(50));
    }

    #[test]
    fn a_send_outlasts_core_s_own_upload_deadlines() {
        use ruston_core::api::attachments::ATTACHMENT_REQUEST_TIMEOUT;

        // Core ends a stalled upload with an error that removes the draft; the
        // desktop deadline must not cut in first and report an unknown outcome.
        for attachments in 0..4 {
            let uploads = ATTACHMENT_REQUEST_TIMEOUT * attachments;
            assert!(
                send_deadline(attachments as usize) > uploads + SEND_TIMEOUT / 2,
                "{attachments} attachment(s) leave too little time"
            );
        }
        assert_eq!(send_deadline(0), SEND_TIMEOUT);
    }

    #[test]
    fn replies_are_delivered_once_or_cancelled() {
        let (reply, mut answer) = PromptAnswer::channel();
        reply.clone().send(1);
        reply.send(2);
        assert_eq!(answer.try_recv(), Ok(Some(1)));

        let (reply, mut answer) = PromptAnswer::<u8>::channel();
        reply.cancel();
        assert!(answer.try_recv().is_err());
    }

    #[test]
    fn missing_mailbox_password_has_a_structured_outcome() {
        let outcome = map_sign_in_error(
            Error::Other(MAILBOX_PASSWORD_REQUIRED.into()),
            Supplied::default(),
        );

        assert!(matches!(outcome, SignInOutcome::NeedsMailboxPassword));
    }

    #[test]
    fn security_key_requirement_is_supported_failure() {
        let outcome = map_sign_in_error(
            Error::Other(SECURITY_KEY_REQUIRED.into()),
            Supplied::default(),
        );

        assert!(matches!(
            outcome,
            SignInOutcome::Failed(AuthError::SecurityKeyUnsupported)
        ));
    }

    #[test]
    fn unauthorized_totp_retry_is_an_invalid_code() {
        let outcome = map_sign_in_error(
            Error::Unauthorized,
            Supplied {
                totp: true,
                ..Supplied::default()
            },
        );

        assert!(matches!(
            outcome,
            SignInOutcome::Failed(AuthError::InvalidTotp)
        ));
    }

    #[test]
    fn failed_mailbox_unlock_is_an_invalid_mailbox_password() {
        let outcome = map_sign_in_error(
            Error::Crypto(format!("{USER_KEY_UNLOCK_FAILED} (wrong password?)")),
            Supplied {
                mailbox_password: true,
                ..Supplied::default()
            },
        );

        assert!(matches!(
            outcome,
            SignInOutcome::Failed(AuthError::InvalidMailboxPassword)
        ));
    }

    #[test]
    fn a_session_bound_to_another_address_asks_to_sign_in_again() {
        use ruston_core::session::{MemoryStore, Paths, Session, Tokens};

        let folder =
            std::env::temp_dir().join(format!("ruston-session-address-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        let paths = Paths::with_base(&folder);
        let save = |base_url: &str, store: &MemoryStore| {
            let session = Session {
                uid: "UID1".into(),
                app_version: "Other".into(),
                base_url: base_url.into(),
                password_mode: 1,
                user_agent: None,
                user_id: None,
            };
            let tokens = Tokens {
                uid: "UID1".into(),
                access: SecretString::from("access"),
                refresh: SecretString::from("refresh"),
            };
            session
                .save(&paths, PROFILE, store, &tokens, &SecretString::from("skp"))
                .unwrap();
        };
        // The secret store keeps the first address; the file now names another.
        let bound = MemoryStore::new();
        save("https://mail.proton.me/api", &bound);
        save("https://example.test/api", &MemoryStore::new());

        let Err(error) = Session::load(&paths, PROFILE, &bound) else {
            panic!("a session bound to another address must not load");
        };
        std::fs::remove_dir_all(&folder).unwrap();

        assert_eq!(map_auth_error(error), AuthError::SessionAddressMismatch);
        assert_eq!(
            map_auth_error(Error::Session("keyring get: locked".into())),
            AuthError::SessionUnavailable
        );
    }

    const BANK: &str = "notifications@bank.example.com";
    const ME: &str = "me@proton.me";

    fn own() -> OwnAddresses {
        [ME.to_owned()].into_iter().collect()
    }

    fn metadata(id: &str, sender: &str, time: i64, labels: &[&str]) -> MessageMetadata {
        MessageMetadata {
            id: id.into(),
            sender: person("", sender),
            subject: format!("Subject {id}"),
            time,
            unread: 1,
            label_ids: labels.iter().map(|label| (*label).to_owned()).collect(),
            ..Default::default()
        }
    }

    fn ids(rows: &[ConversationSummary]) -> Vec<&str> {
        rows.iter().map(|row| row.id.as_str()).collect()
    }

    #[test]
    fn actions_map_to_relabels_and_flags_never_deletes() {
        let moving = |folder: MailFolder| MailAction::MoveTo(Folder::System(folder));

        assert_eq!(
            action_call(&moving(MailFolder::Archive)),
            ActionCall::Move(label_ids::ARCHIVE)
        );
        assert_eq!(
            action_call(&moving(MailFolder::Spam)),
            ActionCall::Move(label_ids::SPAM)
        );
        assert_eq!(
            action_call(&moving(MailFolder::Trash)),
            ActionCall::Move(label_ids::TRASH)
        );
        assert_eq!(
            action_call(&moving(MailFolder::Inbox)),
            ActionCall::Move(label_ids::INBOX)
        );

        // A folder the account made carries its own id straight through.
        let custom = MailAction::MoveTo(Folder::custom("kZ9", "Invoices"));
        assert_eq!(action_call(&custom), ActionCall::Move("kZ9"));

        // A row only a search found has no folder to be read in, so Proton
        // is pointed at All Mail, which holds every conversation.
        assert_eq!(context_label(None), label_ids::ALL_MAIL);
        assert_eq!(context_label(Some(&Folder::INBOX)), label_ids::INBOX);
        assert_eq!(
            context_label(Some(&Folder::custom("kZ9", "Invoices"))),
            "kZ9"
        );

        // A label change names the label and says which way it goes.
        let receipts = Folder::label("wN2", "Receipts");
        assert_eq!(
            action_call(&MailAction::SetLabel {
                label: receipts.clone(),
                on: true,
            }),
            ActionCall::Label("wN2", true)
        );
        assert_eq!(
            action_call(&MailAction::SetLabel {
                label: receipts,
                on: false,
            }),
            ActionCall::Label("wN2", false)
        );
        assert_eq!(
            action_call(&MailAction::SetUnread(true)),
            ActionCall::MarkRead(false)
        );
        assert_eq!(
            action_call(&MailAction::SetUnread(false)),
            ActionCall::MarkRead(true)
        );
        assert_eq!(
            action_call(&MailAction::SetStarred(true)),
            ActionCall::Star(true)
        );
    }

    #[test]
    fn inspection_failures_keep_grouping_but_expired_sessions_fail() {
        let outcome = |inspection| inspection_outcome(inspection, &Folder::INBOX, &own());

        let repeated = vec![
            metadata("m1", BANK, 10, &[label_ids::INBOX]),
            metadata("m2", BANK, 20, &[label_ids::INBOX]),
        ];
        let split = outcome(Ok(repeated))
            .unwrap()
            .expect("repeated mail splits");
        assert_eq!(ids(&split), ["m2", "m1"]);
        // What `limited_inspection` hands over: core's errors as the mailbox
        // maps them, and Unavailable for an inspection that ran out of time.
        for kept in [
            MailboxError::Unavailable,
            map_mailbox_error(Error::Other("unexpected".into())),
            map_mailbox_error(api_error(500)),
        ] {
            assert_eq!(outcome(Err(kept)), Ok(None));
        }
        for expired in [Error::Unauthorized, api_error(401)] {
            assert_eq!(
                outcome(Err(map_mailbox_error(expired))),
                Err(MailboxError::SessionExpired)
            );
        }
    }

    #[test]
    fn repeated_inbound_mail_becomes_individual_rows() {
        let messages = vec![
            metadata("m1", BANK, 10, &[label_ids::INBOX]),
            metadata("m2", BANK, 30, &[label_ids::INBOX, label_ids::STARRED]),
            metadata("m3", BANK, 20, &[label_ids::INBOX]),
        ];

        let rows = inspect_messages(messages, &Folder::INBOX, &own()).expect("split");

        assert_eq!(ids(&rows), ["m2", "m3", "m1"]);
        assert!(rows.iter().all(|row| row.kind == SummaryKind::Message));
        assert!(rows.iter().all(|row| row.message_count == 1 && row.unread));
        assert!(rows[0].starred && !rows[1].starred);
        assert_eq!(rows[0].subject.as_deref(), Some("Subject m2"));
        assert_eq!(rows[0].correspondents.as_deref(), Some(BANK));
    }

    #[test]
    fn split_rows_only_include_messages_in_the_folder() {
        let messages = vec![
            metadata("m1", BANK, 10, &[label_ids::INBOX]),
            metadata("m2", BANK, 20, &[label_ids::TRASH]),
        ];

        let rows = inspect_messages(messages, &Folder::INBOX, &own()).expect("split");

        assert_eq!(ids(&rows), ["m1"]);
    }

    #[test]
    fn exchanges_and_answered_mail_stay_one_conversation() {
        let reply = vec![
            metadata("m1", BANK, 10, &[label_ids::INBOX]),
            metadata("m2", ME, 20, &[label_ids::SENT]),
        ];
        let mut answered = metadata("m3", BANK, 30, &[label_ids::INBOX]);
        answered.flags = message_flag::REPLIED;
        let flagged = vec![metadata("m1", BANK, 10, &[label_ids::INBOX]), answered];

        for messages in [reply, flagged] {
            assert_eq!(inspect_messages(messages, &Folder::INBOX, &own()), None);
        }
    }

    #[test]
    fn classification_ignores_subjects() {
        let mut same_subject = vec![
            metadata("m1", BANK, 10, &[label_ids::INBOX]),
            metadata("m2", ME, 20, &[label_ids::INBOX]),
        ];
        for message in &mut same_subject {
            message.subject = "Statement".into();
        }
        let different_subjects = vec![
            metadata("m1", BANK, 10, &[label_ids::INBOX]),
            metadata("m2", BANK, 20, &[label_ids::INBOX]),
        ];

        let grouped = inspect_messages(same_subject, &Folder::INBOX, &own());
        let split = inspect_messages(different_subjects, &Folder::INBOX, &own());

        assert_eq!(grouped, None);
        assert_eq!(split.map(|rows| rows.len()), Some(2));
    }

    #[test]
    fn missing_folder_labels_fall_back_to_the_conversation() {
        let messages = vec![
            metadata("m1", BANK, 10, &[label_ids::ARCHIVE]),
            metadata("m2", BANK, 20, &[label_ids::ARCHIVE]),
        ];

        assert_eq!(inspect_messages(messages, &Folder::INBOX, &own()), None);
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

        let summary = summarize(conversation, &Folder::INBOX);

        assert_eq!(
            summary,
            ConversationSummary {
                id: "conversation".into(),
                kind: SummaryKind::Conversation,
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
                has_attachments: false,
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

        let summary = summarize(conversation, &Folder::INBOX);

        assert_eq!(summary.subject, None);
        assert_eq!(summary.correspondents, None);
        assert_eq!(summary.time, None);
        assert_eq!(summary.message_count, 0);
        assert!(!summary.unread);
        assert!(!summary.starred);
    }

    #[test]
    fn a_message_row_trims_its_subject_and_names_and_drops_what_is_blank() {
        let padded = MessageMetadata {
            subject: "  Statement ready ".into(),
            sender: person("  Bank  ", &format!(" {BANK} ")),
            ..metadata("padded", BANK, 1_700_000_000, &[label_ids::INBOX])
        };
        let blank = MessageMetadata {
            subject: " ".into(),
            sender: person(" ", BANK),
            ..metadata("blank", BANK, 0, &[label_ids::INBOX])
        };

        let padded = message_summary(padded);
        let blank = message_summary(blank);

        assert_eq!(padded.subject.as_deref(), Some("Statement ready"));
        assert_eq!(padded.correspondents.as_deref(), Some("Bank"));
        assert_eq!(
            padded.participants,
            [MailAddress {
                name: Some("Bank".into()),
                address: BANK.into(),
            }]
        );
        assert_eq!(padded.time, Some(1_700_000_000));
        assert_eq!(blank.subject, None);
        assert_eq!(blank.correspondents.as_deref(), Some(BANK));
        assert_eq!(
            blank.participants,
            [MailAddress {
                name: None,
                address: BANK.into(),
            }]
        );
        assert_eq!(blank.time, None);
    }

    #[test]
    fn a_message_on_its_own_with_a_blank_subject_has_none() {
        let unreadable = UnreadableMessage {
            meta: MessageMetadata {
                subject: "  ".into(),
                ..metadata("m1", BANK, 0, &[label_ids::INBOX])
            },
            reply_tos: Vec::new(),
            error: Error::Crypto("no key of the account opens it".into()),
        };

        let detail = single_message_detail(
            "m1",
            ConversationMessage::Unreadable(unreadable),
            &OwnAddresses::new(),
        );

        assert_eq!(detail.subject, None);
        assert_eq!(detail.messages[0].time, None);
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

        let summary = summarize(conversation, &Folder::System(MailFolder::Sent));

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
            // The API should not duplicate identifiers, but preserving the
            // first one matches the former linear lookup if it ever does.
            LabelCount {
                label_id: label_ids::INBOX.into(),
                total: 10,
                unread: 9,
            },
        ];

        let invoices = Folder::custom("custom", "Invoices");
        let mapped = map_counts(&counts, std::slice::from_ref(&invoices));

        assert_eq!(mapped.unread(&Folder::INBOX), Some(4));
        assert_eq!(mapped.unread(&Folder::System(MailFolder::Trash)), None);
        // A folder the account made is counted like any other, once known.
        assert_eq!(mapped.unread(&invoices), Some(1));

        // Unknown to the sidebar means unknown to the counts.
        assert_eq!(map_counts(&counts, &[]).unread(&invoices), None);
    }

    #[test]
    fn a_session_that_cannot_be_read_or_saved_is_named_as_such() {
        for message in [
            "keyring get: locked".to_owned(),
            "keyring set: access denied".to_owned(),
            format!("saved session {SESSION_ADDRESS_MISMATCH}"),
        ] {
            assert_eq!(
                map_mailbox_error(Error::Session(message)),
                MailboxError::SessionUnavailable
            );
        }
        // Reading a file to attach has nothing to do with the session.
        assert_eq!(
            map_mailbox_error(Error::Io(std::io::Error::other("unreadable"))),
            MailboxError::Unavailable
        );
        let sending = SendError::Mailbox(MailboxError::SessionUnavailable).message();
        assert!(sending.contains("was not sent"), "{sending}");
        assert!(!sending.contains("would not accept"), "{sending}");
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
