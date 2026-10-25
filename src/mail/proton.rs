use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Semaphore;

use futures::channel::{mpsc, oneshot};
use futures::{SinkExt, StreamExt, stream};
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
/// How many recipients' keys a draft's protection check asks for at once.
const PROTECTION_CONCURRENCY: usize = 4;
/// Saving an attachment fetches its message and the sender's keys, then the
/// file itself, which may take core's whole transfer deadline.
const ATTACHMENT_SAVE_TIMEOUT: Duration = ruston_core::api::attachments::ATTACHMENT_REQUEST_TIMEOUT
    .saturating_add(REQUEST_TIMEOUT.saturating_mul(2));
/// A complete authentication attempt must eventually release its credentials
/// even when the server stops responding. The time a person spends on a
/// prompt, solving a CAPTCHA or finding a code, does not count.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(120);
/// Sending is several requests: the keys of every recipient, a draft, then
/// the message. It is given longer than a request that only reads; uploads
/// add their own time (see `send_deadline`).
const SEND_TIMEOUT: Duration = Duration::from_secs(90);

/// Per-inspection deadline, including semaphore wait; timeouts keep Proton grouping.
const INSPECTION_BUDGET: Duration = Duration::from_secs(15);

const PROFILE: &str = ruston_core::DEFAULT_SESSION_PROFILE;
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

        // Core writes the subjects of answers and keys forwarded attachments
        // for the draft, and Proton threads them; a reply goes to the
        // recipients typed, which opened with whom core would answer.
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

    /// Asks Proton for each address's keys, a few at a time and all within
    /// one `REQUEST_TIMEOUT`. A failed lookup is reported for its own address
    /// and does not stop the others.
    pub async fn protection(&self, addresses: &[String]) -> Vec<ProtectionCheck> {
        protection_of(addresses, |address| {
            self.client.recipient_protection(address)
        })
        .await
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

/// Each address's protection through `lookup`, in the addresses' order, with
/// a few lookups at a time. The batch shares one `REQUEST_TIMEOUT`: a lookup
/// still waiting when it runs out, or not started by then, reports a failed
/// connection.
async fn protection_of<'a, F>(
    addresses: &'a [String],
    lookup: impl Fn(&'a str) -> F,
) -> Vec<ProtectionCheck>
where
    F: Future<Output = ruston_core::Result<RecipientProtection>>,
{
    let deadline = tokio::time::Instant::now() + REQUEST_TIMEOUT;
    let lookup = &lookup;
    // Built before the await: with the closure kept in a stream across it,
    // the compiler cannot show that the future is `Send`.
    let checks: Vec<_> = addresses
        .iter()
        .map(|address| async move {
            let protection = if tokio::time::Instant::now() < deadline {
                tokio::time::timeout_at(deadline, lookup(address))
                    .await
                    .map_err(|_| MailboxError::Connection)
                    .and_then(|result| result.map_err(map_mailbox_error))
                    .map(|protection| match protection {
                        RecipientProtection::EndToEnd => Protection::EndToEnd,
                        RecipientProtection::Pgp => Protection::Pgp,
                        RecipientProtection::Unencrypted => Protection::Unencrypted,
                    })
            } else {
                Err(MailboxError::Connection)
            };
            (address.clone(), protection)
        })
        .collect();
    stream::iter(checks)
        .buffered(PROTECTION_CONCURRENCY)
        .collect()
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
        Error::Session(_) | Error::SessionAddressMismatch => MailboxError::SessionUnavailable,
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
        Err(Error::Unauthorized | Error::Crypto(_) | Error::UserKeysLocked | Error::Srp(_)) => {
            ResumeOutcome::SignedOut
        }
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
        Error::Api(error)
            if matches!(error.http_status, 401 | 403) || mentions_credentials(&error.message) =>
        {
            SignInOutcome::Failed(AuthError::InvalidCredentials)
        }
        Error::UserKeysLocked if supplied.mailbox_password => {
            SignInOutcome::Failed(AuthError::InvalidMailboxPassword)
        }
        error => SignInOutcome::Failed(map_auth_error(error)),
    }
}

fn map_auth_error(error: Error) -> AuthError {
    match error {
        Error::HumanVerification(challenge) => verification_error(&challenge),
        Error::Http(_) => AuthError::Connection,
        Error::SessionAddressMismatch => AuthError::SessionAddressMismatch,
        Error::Session(_) | Error::Io(_) => AuthError::SessionUnavailable,
        Error::Api(error) => AuthError::Service {
            http_status: error.http_status,
            code: error.code,
        },
        Error::SecurityKeyRequired => AuthError::SecurityKeyUnsupported,
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
mod tests;
