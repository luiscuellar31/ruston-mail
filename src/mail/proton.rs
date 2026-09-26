use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Semaphore;

use futures::SinkExt;
use futures::channel::{mpsc, oneshot};
use ruston_core::model::enums::{label_ids, message_flag};
use ruston_core::model::message::Attachment;
use ruston_core::{
    Client, Conversation, Error, HvChallenge, HvResolver, Label, LabelCount, LoginOptions,
    MessageMetadata, Recipient, SearchOpts, SendOptions, TotpPrompt,
};
use secrecy::SecretString;

use super::MailAction;
use super::outgoing::{Kind, Outgoing, SendError};
use super::threading::{self, MessageFacts, OwnAddresses};
use super::{
    AuthError, ConversationDetail, ConversationPage, ConversationSummary, Folder, LoginRequest,
    MailAddress, MailAttachment, MailFolder, MailMessage, MailboxCounts, MailboxError, MessageBody,
    SummaryKind, html,
};

/// Conversations requested per page from Proton.
pub const PAGE_SIZE: u32 = 50;

/// Maximum number of background metadata inspection requests in flight simultaneously.
const METADATA_CONCURRENCY: usize = 4;

/// Longest wait for any Proton request a view is waiting on. Without it a
/// stuck call leaves the view loading forever, with no way back.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// A complete authentication attempt, including interactive challenges, must
/// eventually release its credentials even when the server stops responding.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(120);
/// Sending is several requests: a draft, a key for every recipient, then the
/// message. It is given longer than a request that only reads.
const SEND_TIMEOUT: Duration = Duration::from_secs(90);

/// Shared deadline for page metadata inspection; timeouts keep Proton grouping.
const INSPECTION_BUDGET: Duration = Duration::from_secs(15);

const PROFILE: &str = "ruston";
const SIGN_IN_CANCELLED: &str = "sign-in cancelled";
/// Proton's standalone human-verification page.
const VERIFY_ORIGIN: &str = "https://verify.proton.me";
const MAILBOX_PASSWORD_REQUIRED: &str = "this account uses a separate mailbox password";
const SECURITY_KEY_REQUIRED: &str = "FIDO2/WebAuthn";
const USER_KEY_UNLOCK_FAILED: &str = "no user key could be unlocked";
const MAX_LISTED_CORRESPONDENTS: usize = 3;
/// Conservative relabel batch size; Proton publishes no endpoint limit.
const LABEL_BATCH: usize = 50;

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
    Totp(Reply<SecretString>),
    /// Complete Proton's check at `url` in a browser, then confirm.
    HumanVerification { url: String, done: Reply<()> },
}

impl SignInPrompt {
    /// Abandons the prompt; the waiting sign-in ends as cancelled.
    pub fn cancel(self) {
        match self {
            Self::Totp(reply) => reply.cancel(),
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
pub struct Reply<T>(Arc<Mutex<Option<oneshot::Sender<T>>>>);

impl<T> Reply<T> {
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
        match Client::resume(PROFILE).await {
            Ok(client) => ResumeOutcome::Authenticated(Arc::new(Self::from_client(client))),
            Err(Error::Unauthorized | Error::Crypto(_) | Error::Srp(_)) => ResumeOutcome::SignedOut,
            Err(Error::Api(error)) if matches!(error.http_status, 401 | 403) => {
                ResumeOutcome::SignedOut
            }
            Err(error) => ResumeOutcome::Failed(map_error(error)),
        }
    }

    /// Signs in, forwarding interactive prompts and always emitting `Finished`.
    pub async fn sign_in(request: LoginRequest, mut events: mpsc::Sender<SignInEvent>) {
        let has_totp = request.totp.is_some();
        let has_mailbox_password = request.mailbox_password.is_some();
        let prompted_totp = Arc::new(AtomicBool::new(false));
        let options = LoginOptions {
            username: request.username,
            password: request.password,
            totp: request.totp,
            mailbox_password: request.mailbox_password,
            profile: PROFILE.to_owned(),
            base_url: None,
            app_version: None,
            user_agent: Some(user_agent()),
            hv: Some(human_verification(events.clone())),
        };
        let totp_prompt = totp_prompt(events.clone(), prompted_totp.clone());

        let outcome = match tokio::time::timeout(
            SIGN_IN_TIMEOUT,
            Client::login_with_totp_prompt(options, totp_prompt),
        )
        .await
        {
            Ok(Ok(client)) => SignInOutcome::Authenticated(Arc::new(Self::from_client(client))),
            Ok(Err(error)) => {
                let has_totp = has_totp || prompted_totp.load(Ordering::Relaxed);
                map_sign_in_error(error, has_totp, has_mailbox_password)
            }
            Err(_) => SignInOutcome::Failed(AuthError::Connection),
        };
        let _ = events.send(SignInEvent::Finished(outcome)).await;
        // The client keeps the verification resolver; closing the channel ends
        // the sign-in stream and makes any later prompt fail as cancelled.
        events.close_channel();
    }

    /// Revokes the session within the standard request timeout.
    pub async fn logout(&self) -> Result<(), AuthError> {
        timed_with(
            self.client.logout(),
            REQUEST_TIMEOUT,
            AuthError::Connection,
            map_error,
        )
        .await
    }

    pub fn email(&self) -> Option<&str> {
        self.email.as_deref()
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
        let _permit = self
            .inspection_limiter
            .acquire()
            .await
            .map_err(|_| MailboxError::Unavailable)?;

        let inspection = tokio::time::timeout(
            INSPECTION_BUDGET,
            self.client.conversation_messages(conversation_id),
        )
        .await
        .ok();

        match inspection.map(|res| res.map_err(map_mailbox_error)) {
            Some(Ok(messages)) => Ok(inspect_messages(messages, folder, &self.own_addresses)),
            Some(Err(MailboxError::SessionExpired)) => Err(MailboxError::SessionExpired),
            Some(Err(_)) | None => Ok(None),
        }
    }

    /// Lists account-owned folders and labels for the sidebar.
    pub async fn list_folders(&self) -> Result<Vec<Folder>, MailboxError> {
        let folders = timed(self.client.list_folders()).await?;
        let labels = timed(self.client.list_labels()).await?;

        Ok(custom_places(folders, Folder::custom)
            .into_iter()
            .chain(custom_places(labels, Folder::label))
            .collect())
    }

    /// Sends a message. The reply is Proton's id for it, which nothing here
    /// needs yet.
    pub async fn send(&self, outgoing: &Outgoing) -> Result<(), SendError> {
        let options = SendOptions {
            to: outgoing.to.clone(),
            cc: outgoing.cc.clone(),
            bcc: outgoing.bcc.clone(),
            subject: outgoing.subject.clone(),
            body: outgoing.wire_body(),
            html: outgoing.is_html(),
            attachments: outgoing.attachments.clone(),
            // The account's own address, chosen by ruston-core.
            // Scheduling and self-destruct are not offered yet.
            ..SendOptions::default()
        };

        // Proton owns reply recipients, threading and forwarded attachments.
        let call = async {
            match &outgoing.kind {
                Kind::New => self.client.send(&options).await,
                Kind::Reply {
                    message_id,
                    everyone,
                } => self.client.reply(message_id, *everyone, &options).await,
                Kind::Forward { message_id } => self.client.forward(message_id, &options).await,
            }
        };

        timed_with(
            call,
            SEND_TIMEOUT,
            SendError::Unconfirmed,
            |error| match error {
                Error::SendUnconfirmed { .. } => SendError::Unconfirmed,
                other => SendError::Mailbox(map_mailbox_error(other)),
            },
        )
        .await
        .map(|_| ())
    }

    pub async fn conversation_counts(
        &self,
        folders: &[Folder],
    ) -> Result<MailboxCounts, MailboxError> {
        let counts = timed(self.client.conversation_counts()).await?;

        Ok(map_counts(&counts, folders))
    }

    /// Decrypts a conversation oldest-first; ruston-core sanitizes its HTML.
    pub async fn conversation_detail(&self, id: &str) -> Result<ConversationDetail, MailboxError> {
        let (conversation, messages) = timed(self.client.read_conversation(id)).await?;
        let subject = conversation.subject.trim();
        let labels = conversation
            .labels
            .into_iter()
            .map(|label| label.id)
            .collect();

        Ok(ConversationDetail {
            id: id.to_owned(),
            subject: (!subject.is_empty()).then(|| subject.to_owned()),
            labels,
            messages: messages
                .into_iter()
                .map(|message| {
                    mail_message(
                        message.meta,
                        message.body,
                        &message.mime_type,
                        message.attachments,
                    )
                })
                .collect(),
        })
    }

    /// Fetches and decrypts one message that the list shows on its own.
    pub async fn message_detail(&self, id: &str) -> Result<ConversationDetail, MailboxError> {
        let message = timed(self.client.read_message(id)).await?;
        let subject = message.meta.subject.trim().to_owned();
        let labels = message.meta.label_ids.clone();

        Ok(ConversationDetail {
            id: id.to_owned(),
            subject: (!subject.is_empty()).then_some(subject),
            labels,
            messages: vec![mail_message(
                message.meta,
                message.body,
                &message.mime_type,
                message.attachments,
            )],
        })
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
        timed(self.client.download_attachment(message_id, attachment_id)).await
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
                (SummaryKind::Conversation, ActionCall::Label(label, on)) => {
                    // Proton relabels messages, so first list the thread's ids.
                    let thread: Vec<String> = client
                        .conversation_messages(id)
                        .await?
                        .into_iter()
                        .map(|message| message.id)
                        .collect();

                    set_label(client, &thread, label, on).await
                }
                (SummaryKind::Message, ActionCall::Label(label, on)) => {
                    set_label(client, &ids, label, on).await
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
            inspection_limiter: Arc::new(Semaphore::new(METADATA_CONCURRENCY)),
        }
    }
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

/// Asks for the TOTP code only when Proton requires it, after any human
/// verification, so the code is still fresh.
fn totp_prompt(events: mpsc::Sender<SignInEvent>, prompted: Arc<AtomicBool>) -> TotpPrompt {
    Arc::new(move || {
        let events = events.clone();
        let prompted = prompted.clone();
        Box::pin(async move {
            let code = ask(events, SignInPrompt::Totp).await?;
            prompted.store(true, Ordering::Relaxed);
            Ok(code)
        })
    })
}

/// Proton Bridge's external-browser flow: the verification page attaches the
/// solved captcha to the challenge, so the retry echoes the challenge token.
fn human_verification(events: mpsc::Sender<SignInEvent>) -> HvResolver {
    Arc::new(move |challenge: HvChallenge| {
        let events = events.clone();
        Box::pin(async move {
            let captcha = challenge.methods.is_empty()
                || challenge.methods.iter().any(|method| method == "captcha");
            if !captcha {
                return Err(Error::HumanVerification(challenge));
            }
            let url = verification_url(&challenge.web_url, &challenge.token);
            ask(events, |done| SignInPrompt::HumanVerification { url, done }).await?;
            Ok((challenge.token, "captcha".to_owned()))
        })
    })
}

/// Sends a prompt to the UI and waits for the answer.
async fn ask<T: Send + 'static>(
    mut events: mpsc::Sender<SignInEvent>,
    prompt: impl FnOnce(Reply<T>) -> SignInPrompt,
) -> ruston_core::Result<T> {
    let (reply, answer) = Reply::channel();
    events
        .send(SignInEvent::Prompt(prompt(reply)))
        .await
        .map_err(|_| cancelled())?;
    answer.await.map_err(|_| cancelled())
}

fn cancelled() -> Error {
    Error::Other(SIGN_IN_CANCELLED.to_owned())
}

/// The verification page for a challenge. The server's `WebUrl` host is kept
/// only when it is an https `proton.me` host; anything else uses the default.
fn verification_url(web_url: &str, token: &str) -> String {
    let origin = web_url
        .strip_prefix("https://")
        .and_then(|rest| rest.split(['/', '?', '#']).next())
        .filter(|host| {
            host.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
                && (*host == "proton.me" || host.ends_with(".proton.me"))
        })
        .map_or_else(
            || VERIFY_ORIGIN.to_owned(),
            |host| format!("https://{host}"),
        );

    format!("{origin}/?methods=captcha&token={}", percent_encode(token))
}

fn percent_encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
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

/// Adds or removes a label in batches. Earlier successful batches stay applied.
async fn set_label(
    client: &Client,
    ids: &[String],
    label: &str,
    on: bool,
) -> Result<(), ruston_core::Error> {
    for batch in ids.chunks(LABEL_BATCH) {
        if on {
            client.apply_label(batch, label).await?;
        } else {
            client.remove_label(batch, label).await?;
        }
    }

    Ok(())
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
    let subject = conversation.subject.trim();

    ConversationSummary {
        subject: (!subject.is_empty()).then(|| subject.to_owned()),
        correspondents: display_names(correspondents),
        participants,
        preview: None,
        time: (time > 0).then_some(time),
        unread: unread > 0,
        starred,
        message_count: u32::try_from(conversation.num_messages).unwrap_or(0),
        has_attachments: conversation.num_attachments > 0,
        id: conversation.id,
        kind: SummaryKind::Conversation,
    }
}

/// Rows for a conversation whose messages were inspected. `None` means the
/// inspection ran out of time.
#[cfg(test)]
fn inspected_rows(
    conversation: Conversation,
    inspection: Option<ruston_core::Result<Vec<MessageMetadata>>>,
    folder: &Folder,
    own: &OwnAddresses,
) -> Result<Vec<ConversationSummary>, MailboxError> {
    match inspection.map(|result| result.map_err(map_mailbox_error)) {
        Some(Ok(messages)) => Ok(split_rows(conversation, messages, folder, own)),
        // An expired session ends the page load like any other request.
        Some(Err(MailboxError::SessionExpired)) => Err(MailboxError::SessionExpired),
        // Any other failure, or no answer in time, keeps Proton's grouping.
        Some(Err(_)) | None => Ok(vec![summarize(conversation, folder)]),
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

/// Splits proven independent inbound messages; otherwise keeps the conversation.
#[cfg(test)]
fn split_rows(
    conversation: Conversation,
    messages: Vec<MessageMetadata>,
    folder: &Folder,
    own: &OwnAddresses,
) -> Vec<ConversationSummary> {
    inspect_messages(messages, folder, own).unwrap_or_else(|| vec![summarize(conversation, folder)])
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
    let subject = message.subject.trim();

    ConversationSummary {
        subject: (!subject.is_empty()).then(|| subject.to_owned()),
        correspondents,
        participants,
        preview: None,
        time: (message.time > 0).then_some(message.time),
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

fn mail_address(person: &Recipient) -> MailAddress {
    MailAddress {
        name: (!person.name.trim().is_empty()).then(|| person.name.trim().to_owned()),
        address: person.address.trim().to_owned(),
    }
}

fn mail_message(
    meta: MessageMetadata,
    body: String,
    mime_type: &str,
    attachments: Vec<Attachment>,
) -> MailMessage {
    let body = if mime_type.to_ascii_lowercase().starts_with("text/html") {
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
        sender: mail_address(&meta.sender),
        recipients: meta
            .to_list
            .iter()
            .chain(&meta.cc_list)
            .chain(&meta.bcc_list)
            .map(mail_address)
            .collect(),
        time: (meta.time > 0).then_some(meta.time),
        body,
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
        Error::Api(error) if error.http_status == 401 => MailboxError::SessionExpired,
        Error::Http(_) => MailboxError::Connection,
        Error::Api(_) | Error::HumanVerification(_) => MailboxError::Service,
        _ => MailboxError::Unavailable,
    }
}

fn map_sign_in_error(error: Error, has_totp: bool, has_mailbox_password: bool) -> SignInOutcome {
    match error {
        Error::Other(message) if message == SIGN_IN_CANCELLED => SignInOutcome::Cancelled,
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
    use ruston_core::ApiError;
    use ruston_core::model::ConversationLabel;

    use super::*;

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

        let message = mail_message(
            meta,
            "<p>Hello &amp; welcome</p>".into(),
            "text/html",
            Vec::new(),
        );

        assert_eq!(message.id, "message");
        assert_eq!(message.sender.name, None);
        assert_eq!(message.sender.address, "alex@example.com");
        assert_eq!(message.recipients.len(), 3);
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
            MessageMetadata::default(),
            body.into(),
            "text/plain",
            Vec::new(),
        );

        assert_eq!(message.body, MessageBody::PlainText(body.into()));
        assert_eq!(message.time, None);
    }

    #[test]
    fn abandoned_prompt_is_a_cancelled_outcome() {
        let outcome = map_sign_in_error(cancelled(), false, false);

        assert!(matches!(outcome, SignInOutcome::Cancelled));
    }

    #[test]
    fn verification_page_stays_on_proton() {
        assert_eq!(
            verification_url("", "a b/c"),
            "https://verify.proton.me/?methods=captcha&token=a%20b%2Fc"
        );
        assert_eq!(
            verification_url("https://verify-api.proton.me/x?y=1", "t"),
            "https://verify-api.proton.me/?methods=captcha&token=t"
        );
        for untrusted in [
            "http://verify.proton.me/",
            "https://proton.me.example.com/",
            "https://evil@verify.proton.me/",
            "https://example.com/",
        ] {
            assert!(
                verification_url(untrusted, "t").starts_with("https://verify.proton.me/?"),
                "{untrusted}"
            );
        }
    }

    #[test]
    fn replies_are_delivered_once_or_cancelled() {
        let (reply, mut answer) = Reply::channel();
        reply.clone().send(1);
        reply.send(2);
        assert_eq!(answer.try_recv(), Ok(Some(1)));

        let (reply, mut answer) = Reply::<u8>::channel();
        reply.cancel();
        assert!(answer.try_recv().is_err());
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

    fn bank_conversation() -> Conversation {
        Conversation {
            id: "conversation".into(),
            subject: "Account statement".into(),
            num_messages: 3,
            senders: vec![person("", BANK)],
            labels: vec![label(label_ids::INBOX)],
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
        let grouped = |inspection| {
            inspected_rows(bank_conversation(), inspection, &Folder::INBOX, &own())
                .map(|rows| ids(&rows).join(","))
        };

        assert_eq!(grouped(None), Ok("conversation".into()));
        assert_eq!(
            grouped(Some(Err(Error::Other("unexpected".into())))),
            Ok("conversation".into())
        );
        assert_eq!(
            grouped(Some(Err(api_error(500)))),
            Ok("conversation".into())
        );
        assert_eq!(
            grouped(Some(Err(Error::Unauthorized))),
            Err(MailboxError::SessionExpired)
        );
        assert_eq!(
            grouped(Some(Err(api_error(401)))),
            Err(MailboxError::SessionExpired)
        );
    }

    #[test]
    fn repeated_inbound_mail_becomes_individual_rows() {
        let messages = vec![
            metadata("m1", BANK, 10, &[label_ids::INBOX]),
            metadata("m2", BANK, 30, &[label_ids::INBOX, label_ids::STARRED]),
            metadata("m3", BANK, 20, &[label_ids::INBOX]),
        ];

        let rows = split_rows(bank_conversation(), messages, &Folder::INBOX, &own());

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

        let rows = split_rows(bank_conversation(), messages, &Folder::INBOX, &own());

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
            let rows = split_rows(bank_conversation(), messages, &Folder::INBOX, &own());

            assert_eq!(ids(&rows), ["conversation"]);
            assert_eq!(rows[0].kind, SummaryKind::Conversation);
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

        let grouped = split_rows(bank_conversation(), same_subject, &Folder::INBOX, &own());
        let split = split_rows(
            bank_conversation(),
            different_subjects,
            &Folder::INBOX,
            &own(),
        );

        assert_eq!(grouped.len(), 1);
        assert_eq!(split.len(), 2);
    }

    #[test]
    fn missing_folder_labels_fall_back_to_the_conversation() {
        let messages = vec![
            metadata("m1", BANK, 10, &[label_ids::ARCHIVE]),
            metadata("m2", BANK, 20, &[label_ids::ARCHIVE]),
        ];

        let rows = split_rows(bank_conversation(), messages, &Folder::INBOX, &own());

        assert_eq!(ids(&rows), ["conversation"]);
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
