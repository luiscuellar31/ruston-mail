mod auth;
mod compose;
mod effect;
mod inspection;
mod keys;
mod layout;
mod mailbox;
mod mailto;
mod notifications;
mod reader;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use futures::future::{AbortHandle, Abortable};
use inspection::Inspections;

use crate::downloads::{self, SaveError};
use crate::settings::{Appearance, ComposePlacement, Panels, Settings, StartFolder, Window};

/// How far the window must change before its new size is worth storing.
const RESIZE_STEP: f32 = 8.0;

use crate::mail::{
    AuthError, BodyFormat, ConversationDetail, ConversationPage, ConversationSummary, Folder,
    IncomingMail, MailAction, MailBackend, MailboxCounts, MailboxError, ProtectionCheck,
    ProtonMailService, ResumeOutcome, SendError, SignInOutcome, SignInPrompt, demo::DemoMailbox,
};

use auth::LoginForm;
pub use auth::{AuthState, SignInStep};
use compose::ParkedDraft;
pub use compose::{AnswerKind, Answering, Compose, ComposeField, ComposeId, Sending};
pub use effect::{Effect, Effects, UiEffect};
pub use keys::{Key, KeyPress};
pub use layout::{
    MIN_PANEL_WIDTH, compact as compact_layout, ratios as panel_ratios, widths as panel_widths,
};
use mailbox::ActionStart;
use mailbox::PageRequest;
pub(crate) use mailbox::RequestId;
pub use mailbox::{
    ActionRequest, ListAttempt, ListStatus, Mailbox, ReaderRequest, SearchRequest, Step, UndoMove,
};
pub use reader::{ConversationReader, ReaderState};

/// Identifies one file operation across a destination dialog and download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentSaveRequest {
    id: RequestId,
    session: SessionEpoch,
    message_id: String,
    attachment_id: String,
}

struct AttachmentSave {
    request: AttachmentSaveRequest,
    choosing_destination: bool,
}

type SessionEpoch = u64;
pub(crate) type AuthAttempt = u64;
const PROTON_SIGNUP_URL: &str = "https://account.proton.me/mail/signup";

#[derive(Clone)]
pub enum Message {
    Submit,
    CancelChallenge,
    SessionChecked(ResumeOutcome),
    /// Tries the saved session again after a failure that may pass.
    RetrySavedSession,
    SignInFinished(AuthAttempt, SignInOutcome),
    SignInPrompt(AuthAttempt, SignInPrompt),
    OpenVerificationPage,
    OpenSignupPage,
    CopyVerificationLink,
    LinkClicked(String),
    OpenLink,
    CopyLink,
    DismissLink,
    /// Copies the address of a link in a message, from the link's menu.
    CopyLinkAddress(String),
    /// Copies email addresses from a message header's menu, separated by
    /// commas as the composer's fields take them.
    CopyAddresses(Vec<String>),
    Logout,
    LogoutFinished(SessionEpoch, Result<(), AuthError>),
    SelectFolder(Folder),
    SelectConversation(String),
    RetryConversation,
    SearchChanged(String),
    /// Asks the server for the first page matching what was typed, across folders.
    SearchSubmitted,
    SearchLoaded(SearchRequest, Result<ConversationPage, MailboxError>),
    ToggleMessageExpanded(String),
    /// Folds or unfolds one quoted passage of a message.
    ToggleQuoteExpanded(String, usize),
    /// Applies one action to the selected row. Adding an action is a line in
    /// the toolbar rather than a message and an arm of its own.
    ApplyAction(MailAction),
    /// Applies one action to a listed row without opening it, as the row's
    /// menu does.
    ApplyRowAction(String, MailAction),
    /// Puts the last moved conversation back where it came from.
    UndoMove,
    /// Lets this temporary undo offer expire if it is still current.
    DismissUndo(UndoMove),
    /// Lets this confirmation go if it is still the one shown.
    DismissNotice(Notice),
    RefreshMailbox,
    /// Periodic refresh, every minute in the foreground and less often in the
    /// background. Unlike a manual refresh, this also marks the other folder
    /// caches stale so they catch up when opened.
    AutoRefreshMailbox,
    LoadMoreConversations,
    ConversationsLoaded(RequestId, Result<ConversationPage, MailboxError>),
    ThreadInspected {
        epoch: SessionEpoch,
        request: RequestId,
        revision: u64,
        folder: Folder,
        conversation_id: String,
        result: Result<Option<Vec<ConversationSummary>>, MailboxError>,
    },
    ConversationLoaded(ReaderRequest, Result<ConversationDetail, MailboxError>),
    CountsLoaded(RequestId, Result<MailboxCounts, MailboxError>),
    NewMailLoaded(
        SessionEpoch,
        RequestId,
        Result<Vec<IncomingMail>, MailboxError>,
    ),
    /// The folders the account made, fetched when the mailbox opens and, if
    /// that fails, again with the next refresh.
    FoldersLoaded(SessionEpoch, Result<Vec<Folder>, MailboxError>),
    ActionFinished(ActionRequest, Result<(), MailboxError>),
    MarkReadFinished(ActionRequest, Result<(), MailboxError>),
    PanelsResized(Panels),
    /// A settings write finished: the revision it wrote, and whether the file
    /// holds it now.
    SettingsWritten(u64, bool),
    /// Restores the default window size and mailbox pane widths.
    ResetLayout,
    /// A key the focused widget did not take.
    KeyPressed(KeyPress),
    /// The window changed size, which is worth remembering for next time.
    WindowResized(Window),
    /// Fetches one attachment and saves it, by message and attachment id.
    SaveAttachment(String, String),
    /// Selects a destination before fetching the attachment.
    SaveAttachmentAs(String, String, String),
    AttachmentDestinationChosen(AttachmentSaveRequest, Option<PathBuf>),
    /// Where an attachment landed, or why it did not.
    AttachmentSaved(AttachmentSaveRequest, Result<PathBuf, SaveError>),
    /// Reveals a downloaded file in the system file manager.
    RevealAttachment(PathBuf),
    /// Shows or hides Settings beside the mailbox sidebar.
    ShowSettings(bool),
    /// Opens the project's own repository from the About card.
    OpenSourceCode,
    /// Marks opened mail as read, or leaves it unread.
    SetMarkReadOnOpen(bool),
    /// Asks where a link goes before opening it, or opens it straight away.
    SetConfirmLinks(bool),
    /// Opens every message in a conversation, or only the newest.
    SetExpandAllMessages(bool),
    /// Unfolds quoted passages, or leaves them behind their button.
    SetShowQuotedText(bool),
    /// Scales the whole interface.
    SetZoom(f32),
    /// Chooses the desktop color scheme, which stops following the system's.
    SetAppearance(Appearance),
    /// Follows the system's light or dark mode, or the chosen scheme.
    SetFollowSystemAppearance(bool),
    SetSystemFont(bool),
    /// Shows or hides the unread mail badge on the dock/app icon.
    SetShowUnreadBadge(bool),
    /// Shows or hides desktop notifications for incoming mail.
    SetDesktopNotifications(bool),
    /// Picks the folder a run opens in.
    SetStartFolder(StartFolder),
    /// Writes a message out as HTML, or as plain text.
    SetComposeFormat(BodyFormat),
    /// Opens the composer in the reading pane or in its own window.
    SetComposePlacement(ComposePlacement),
    /// Looks up recipients' encryption while writing, or leaves it to sending.
    SetCheckRecipientProtection(bool),
    /// Takes recipient domains written in another script, sent in their
    /// ASCII form, or refuses them.
    SetAcceptInternationalDomains(bool),
    /// Starts a new message.
    OpenCompose,
    ReceiveMailto(crate::mailto::Request),
    MailtoRejected,
    OpenPendingMailto,
    DismissPendingMailto,
    DismissMailtoError,
    /// Answers the open message, or passes it on.
    Answer {
        message_id: String,
        kind: AnswerKind,
    },
    /// Puts an unsent message away. Untouched messages close; written ones ask first.
    CloseCompose,
    /// Brings a draft folded over the reader back into the reading pane.
    ContinueWriting,
    /// Discards an unsent message after confirmation.
    DiscardCompose,
    /// Cancels discard confirmation and returns to editing.
    CancelDiscard,
    /// Shows or hides the copy fields.
    ToggleComposeCopies,
    /// Puts the original under an answer's text, or leaves it out.
    ToggleComposeQuote,
    ComposeChanged(ComposeField, String),
    /// A recipient field lost focus; its addresses are complete enough to look up.
    ComposeRecipientsLeft,
    /// How Proton will protect the message for the looked-up recipients.
    ComposeProtectionChecked(SessionEpoch, ComposeId, Vec<ProtectionCheck>),
    /// Tells the desktop to open the native file dialog for selecting attachments.
    PickComposeAttachments,
    /// Adds picked local files as attachments to the active draft.
    AddComposeAttachments(ComposeId, Vec<PathBuf>),
    /// The sizes of a draft's attached files, read on the blocking pool.
    AttachmentSizesRead(ComposeId, std::collections::HashMap<PathBuf, Option<u64>>),
    /// Removes an attachment by index from the active draft.
    RemoveComposeAttachment(usize),
    /// Hands the message over to be sent.
    Send,
    Sent(SessionEpoch, Result<(), SendError>),
}

/// A short confirmation shown at the foot of the mailbox until it times out
/// or another takes its place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    /// The message being written went to Proton.
    Sent,
    /// An attachment was saved at this path.
    Saved(PathBuf),
}

pub struct App {
    /// Set for a process started for the fictional mailbox. Such a process
    /// never signs in to Proton, even after its demo mailbox is closed.
    demo_process: bool,
    auth_state: AuthState,
    login_form: LoginForm,
    auth_error: Option<AuthError>,
    /// The saved session could not open for a reason that may pass, such as
    /// no network, and is still saved; the sign-in screen offers to try it
    /// again.
    resume_may_retry: bool,
    /// Changes for every authentication attempt so queued results from an
    /// aborted task cannot affect its replacement.
    auth_attempt: AuthAttempt,
    /// The question a running sign-in is waiting on, if any.
    pending_prompt: Option<SignInPrompt>,
    /// A link clicked in a message, waiting for the user to confirm it.
    pending_link: Option<PendingLink>,
    pending_mailto: std::collections::VecDeque<crate::mailto::Request>,
    mailto_error: Option<&'static str>,
    backend: Option<MailBackend>,
    /// The Proton account the open session belongs to. The demo has none.
    account: Option<String>,
    mailbox: Option<Mailbox>,
    /// Changes whenever an account opens or closes so late async results can
    /// never mutate another session.
    session_epoch: SessionEpoch,
    last_request: RequestId,
    inspections: Inspections,
    /// The reader's conversation load in flight. A newer load, a folder
    /// change or the end of the session abandons it.
    reader_load: Option<AbortHandle>,
    /// The folders the account made, which the sidebar shows below Proton's
    /// own. Empty until they arrive, and in demo mode for good.
    folders: Vec<Folder>,
    /// The folder list failed to load, so the next refresh asks again.
    refetch_folders: bool,
    /// The page a refresh the app started on its own asked for. The list
    /// does not show such a refresh unless someone asks for one meanwhile.
    automatic_refresh: Option<RequestId>,
    /// What the app remembers between runs.
    settings: Settings,
    /// Settings changes and the last revision written to disk.
    /// The UI waits for resizing to settle before calling [`Self::save_settings`].
    settings_revision: u64,
    settings_written: u64,
    /// The revision whose write failed, tried again with a later change or on
    /// quitting rather than on every frame.
    settings_failed: Option<u64>,
    /// A write runs off the interface's thread; the next waits for it.
    settings_writing: bool,
    /// The newest revision the file holds, shared with the write running, so
    /// one that finishes late never undoes a newer one.
    settings_on_disk: Arc<Mutex<u64>>,
    /// Whether Settings replaces the conversation list and reader.
    showing_settings: bool,
    /// The message being written, if there is one.
    compose: Option<Compose>,
    /// The message being written when Proton ended the session, kept in
    /// memory only until its own account signs in again.
    parked_draft: Option<ParkedDraft>,
    /// A conversation opened while a draft covered the reader, so nobody saw
    /// it. It is marked read once the reader shows it, if still open there.
    read_once_shown: Option<String>,
    /// Changes for each opened draft so asynchronous attachment pickers from a
    /// closed or discarded draft cannot inject files into a subsequent draft.
    last_compose_id: ComposeId,
    /// The confirmation shown at the foot of the mailbox.
    notice: Option<Notice>,
    /// The attachment whose last save failed, by ID, and why; its row says so.
    failed_save: Option<(String, SaveError)>,
    /// The attachment in flight, preventing duplicate downloads.
    saving_attachment: Option<AttachmentSave>,
    new_mail: notifications::NewMail,
}

impl App {
    /// In demo mode the app opens a local fictional mailbox and never starts
    /// the Proton session resume.
    pub fn boot(demo: bool, settings: Settings) -> (Self, Effects) {
        let mut app = Self::new(settings);
        app.demo_process = demo;
        let task = if demo {
            app.open_mailbox(MailBackend::demo(), None, None)
        } else {
            Effects::perform(ProtonMailService::resume(), Message::SessionChecked)
        };

        (app, task)
    }

    fn new(settings: Settings) -> Self {
        Self {
            demo_process: false,
            auth_state: AuthState::CheckingSession,
            login_form: LoginForm::default(),
            auth_error: None,
            resume_may_retry: false,
            auth_attempt: 0,
            pending_prompt: None,
            pending_link: None,
            pending_mailto: std::collections::VecDeque::new(),
            mailto_error: None,
            backend: None,
            account: None,
            mailbox: None,
            session_epoch: 0,
            last_request: 0,
            inspections: Inspections::default(),
            reader_load: None,
            folders: Vec::new(),
            refetch_folders: false,
            automatic_refresh: None,
            settings,
            settings_revision: 0,
            settings_written: 0,
            settings_failed: None,
            settings_writing: false,
            settings_on_disk: Arc::new(Mutex::new(0)),
            showing_settings: false,
            compose: None,
            parked_draft: None,
            read_once_shown: None,
            last_compose_id: 0,
            notice: None,
            failed_save: None,
            saving_attachment: None,
            new_mail: notifications::NewMail::default(),
        }
    }

    fn next_compose_id(&mut self) -> ComposeId {
        self.last_compose_id = self
            .last_compose_id
            .checked_add(1)
            .expect("compose id exhausted");
        self.last_compose_id
    }

    fn advance_session_epoch(&mut self) {
        self.inspections.cancel();
        self.abandon_reader_load();
        self.session_epoch = self
            .session_epoch
            .checked_add(1)
            .expect("session epoch exhausted");
    }

    fn advance_auth_attempt(&mut self) -> AuthAttempt {
        self.auth_attempt = self
            .auth_attempt
            .checked_add(1)
            .expect("authentication attempt exhausted");
        self.auth_attempt
    }

    fn is_current_session(&self, epoch: SessionEpoch) -> bool {
        self.session_epoch == epoch
    }

    /// An app that has finished looking for a session and found none, which
    /// is the state the sign-in screen is drawn for.
    #[cfg(test)]
    pub(crate) fn signed_out() -> Self {
        let mut app = Self::new(Settings::default());
        app.auth_state = AuthState::SignedOut;
        app
    }

    /// A session of the Proton account `account`, at `address`, with the demo
    /// mailbox standing in for its mail, so tests can follow one account
    /// from a session to the next.
    #[cfg(test)]
    pub(crate) fn signed_in_as(account: &str, address: &str) -> Self {
        let mut app = Self::new(Settings::default());
        let _ = app.open_mailbox(
            MailBackend::demo(),
            Some(address.to_owned()),
            Some(account.to_owned()),
        );
        app
    }

    /// The attachment now being fetched, so its row can say so.
    pub fn saving_attachment(&self) -> Option<&str> {
        self.saving_attachment
            .as_ref()
            .map(|save| save.request.attachment_id.as_str())
    }

    /// The confirmation to show at the foot of the mailbox.
    pub fn notice(&self) -> Option<&Notice> {
        self.notice.as_ref()
    }

    /// The attachment whose last save failed, by ID, and why.
    pub fn failed_save(&self) -> Option<(&str, SaveError)> {
        self.failed_save
            .as_ref()
            .map(|(attachment_id, error)| (attachment_id.as_str(), *error))
    }

    /// Fetches an attachment and writes it off the UI thread.
    fn save_attachment(
        &mut self,
        message_id: String,
        attachment_id: String,
        suggested_name: Option<String>,
    ) -> Effects {
        if !self.mailbox_actions_available() || self.saving_attachment.is_some() {
            return Effects::none();
        }
        let request = AttachmentSaveRequest {
            id: self.next_request(),
            session: self.session_epoch,
            message_id,
            attachment_id,
        };
        self.failed_save = None;
        self.saving_attachment = Some(AttachmentSave {
            request: request.clone(),
            choosing_destination: suggested_name.is_some(),
        });
        if let Some(name) = suggested_name {
            Effects::ui(UiEffect::PickAttachmentDestination {
                request,
                suggested_name: ruston_core::mail::attachments::safe_attachment_name(&name),
            })
        } else {
            self.download_attachment(request, None)
        }
    }

    fn attachment_destination_chosen(
        &mut self,
        request: AttachmentSaveRequest,
        destination: Option<PathBuf>,
    ) -> Effects {
        if !self.is_current_session(request.session)
            || !self
                .saving_attachment
                .as_ref()
                .is_some_and(|save| save.request == request && save.choosing_destination)
        {
            return Effects::none();
        }
        let Some(path) = destination else {
            self.saving_attachment = None;
            return Effects::none();
        };
        self.saving_attachment
            .as_mut()
            .unwrap()
            .choosing_destination = false;
        self.download_attachment(request, Some(path))
    }

    fn download_attachment(
        &self,
        request: AttachmentSaveRequest,
        destination: Option<PathBuf>,
    ) -> Effects {
        let Some(backend) = self.backend.clone() else {
            return Effects::none();
        };
        let completion = request.clone();

        Effects::perform(
            async move {
                match backend
                    .download_attachment(&request.message_id, &request.attachment_id)
                    .await
                {
                    // File I/O is blocking, so keep it off both the interface
                    // thread and Tokio's asynchronous workers.
                    Ok((name, contents)) => {
                        tokio::task::spawn_blocking(move || match destination {
                            Some(path) => downloads::save_selected(&path, &contents),
                            None => downloads::save(&name, &contents),
                        })
                        .await
                        .unwrap_or(Err(SaveError::Failed))
                    }
                    Err(error) => Err(unsaved(error)),
                }
            },
            move |outcome| Message::AttachmentSaved(completion, outcome),
        )
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// The folders the account made.
    pub fn folders(&self) -> &[Folder] {
        &self.folders
    }

    pub fn showing_settings(&self) -> bool {
        self.showing_settings
    }

    /// Keeps a changed setting. The change applies at once; putting it in the
    /// file is [`Self::save_settings`]'s job.
    fn remember(&mut self, change: impl FnOnce(&mut Settings)) {
        let before = self.settings.clone();
        change(&mut self.settings);
        if self.settings != before {
            self.settings_revision += 1;
        }
    }

    /// How many times the settings have changed this run. A caller watches
    /// this to tell a settled value from one still being dragged.
    pub fn settings_revision(&self) -> u64 {
        self.settings_revision
    }

    /// Whether a change is waiting to reach the file.
    pub fn settings_unsaved(&self) -> bool {
        self.settings_written != self.settings_revision
    }

    /// Starts writing changed settings, best-effort, off the interface's
    /// thread: the write waits for the disk, longer while something scans the
    /// file. One write runs at a time; a change made meanwhile is written
    /// once it ends. A write that failed stays to be done, tried again with
    /// the next change. In-memory changes always remain.
    pub fn save_settings(&mut self) -> Effects {
        let revision = self.settings_revision;
        if !self.settings_unsaved()
            || self.settings_writing
            || self.settings_failed == Some(revision)
        {
            return Effects::none();
        }
        self.settings_writing = true;
        let settings = self.settings.clone();
        let on_disk = Arc::clone(&self.settings_on_disk);
        Effects::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    write_settings_revision(&on_disk, revision, || settings.save())
                })
                .await
                .unwrap_or(false)
            },
            move |written| Message::SettingsWritten(revision, written),
        )
    }

    /// Writes changed settings before quitting, trying again even a write
    /// that failed: there is no later. A write still running finishes first,
    /// and this newer one is the last to reach the file.
    pub fn save_settings_before_quitting(&mut self) {
        let revision = self.settings_revision;
        if !self.settings_unsaved() {
            return;
        }
        let settings = &self.settings;
        let written = write_settings_revision(&self.settings_on_disk, revision, || settings.save());
        self.finish_settings_write(revision, written);
    }

    fn finish_settings_write(&mut self, revision: u64, written: bool) {
        self.settings_writing = false;
        if written {
            self.settings_written = self.settings_written.max(revision);
            self.settings_failed = None;
        } else {
            self.settings_failed = Some(revision);
        }
    }

    /// Handles one message. One that uncovers the reader, as closing a draft
    /// in the reading pane does, shows what was opened behind the draft, which
    /// is then read.
    pub fn update(&mut self, message: Message) -> Effects {
        let covered = self.draft_covers_reader();
        let effects = self.dispatch(message);
        if covered && !self.draft_covers_reader() {
            return Effects::batch([effects, self.mark_read_once_shown()]);
        }
        effects
    }

    fn dispatch(&mut self, message: Message) -> Effects {
        match message {
            Message::Submit => return self.sign_in(),
            Message::CancelChallenge => return self.cancel_sign_in(),
            Message::SessionChecked(outcome) => return self.finish_session_check(outcome),
            Message::RetrySavedSession => return self.retry_saved_session(),
            Message::SignInFinished(attempt, outcome) => {
                return self.finish_sign_in(attempt, outcome);
            }
            Message::SignInPrompt(attempt, prompt) => {
                return self.show_prompt(attempt, prompt);
            }
            Message::OpenVerificationPage => {
                if let AuthState::NeedsHumanVerification { url } = &self.auth_state {
                    return open_in_browser(url.clone());
                }
            }
            Message::OpenSignupPage => return open_in_browser(PROTON_SIGNUP_URL.to_owned()),
            Message::OpenSourceCode => {
                return open_in_browser(env!("CARGO_PKG_REPOSITORY").to_owned());
            }
            Message::CopyVerificationLink => {
                if let AuthState::NeedsHumanVerification { url } = &self.auth_state {
                    return Effects::ui(UiEffect::CopyText(url.clone()));
                }
            }
            Message::Logout => return self.logout(),
            Message::LogoutFinished(epoch, result) => self.finish_logout(epoch, result),
            Message::SelectFolder(folder) => return self.select_folder(folder),
            Message::SelectConversation(id) => {
                // A conversation chosen while writing shows, with the draft
                // folded above it.
                self.fold_compose();
                return self.select_conversation(id);
            }
            Message::RetryConversation => return self.retry_conversation(),
            Message::SearchChanged(query) => {
                if let Some(mailbox) = self.active_mailbox() {
                    mailbox.set_search_query(query);
                }
            }
            Message::SearchSubmitted => return self.start_search(),
            Message::SearchLoaded(request, result) => {
                let error = self
                    .mailbox
                    .as_mut()
                    .and_then(|mailbox| mailbox.finish_search(&request, result));
                self.end_session_if_expired(error);
            }
            Message::ToggleMessageExpanded(id) => {
                if let Some(mailbox) = self.active_mailbox() {
                    mailbox.toggle_message(&id);
                }
            }
            Message::ToggleQuoteExpanded(id, index) => {
                if let Some(mailbox) = self.active_mailbox() {
                    mailbox.toggle_quote(&id, index);
                }
            }
            Message::ApplyAction(action) => return self.apply_action(action),
            Message::ApplyRowAction(row_id, action) => {
                return self.apply_row_action(row_id, action);
            }
            Message::UndoMove => return self.undo_move(),
            // The notice stays on screen while signing out, and its timer must
            // still clear it there.
            Message::DismissUndo(offer) => {
                if let Some(mailbox) = &mut self.mailbox {
                    mailbox.dismiss_undo(&offer);
                }
            }
            Message::DismissNotice(notice) => {
                if self.notice.as_ref() == Some(&notice) {
                    self.notice = None;
                }
            }
            Message::RefreshMailbox => {
                // Asked for during a refresh the app started, it shows that one.
                self.automatic_refresh = None;
                return self.refresh_mailbox();
            }
            Message::AutoRefreshMailbox => return self.auto_refresh_mailbox(),
            Message::LoadMoreConversations => return self.load_more_conversations(),
            Message::ConversationsLoaded(request, result) => {
                if !self.accepts_page(request) {
                    return Effects::none();
                }
                let mut result = result;
                let candidates = match &mut result {
                    Ok(page) => std::mem::take(&mut page.inspect_candidates),
                    Err(_) => Vec::new(),
                };
                let error = self
                    .mailbox
                    .as_mut()
                    .and_then(|mailbox| mailbox.finish_page(request, result));
                self.end_session_if_expired(error);
                if error.is_none() {
                    let revalidating = self
                        .mailbox
                        .as_ref()
                        .is_some_and(Mailbox::needs_revalidation);
                    if revalidating {
                        return self.refresh_mailbox();
                    }
                }
                // A search submitted while this page loaded has waited for it.
                if self
                    .mailbox
                    .as_mut()
                    .is_some_and(Mailbox::take_deferred_search)
                {
                    return self.start_search();
                }
                if error.is_none() {
                    let accepted = self
                        .mailbox
                        .as_ref()
                        .is_some_and(|m| m.status() == ListStatus::Loaded);
                    if accepted && !candidates.is_empty() {
                        return self.inspect_candidates(candidates);
                    }
                }
            }
            Message::ThreadInspected {
                epoch,
                request,
                revision,
                folder,
                conversation_id,
                result,
            } => {
                // Finishing first frees the answer's place even when the
                // folder was renamed meanwhile, which keeps the folder.
                if !self.is_current_session(epoch)
                    || !self.inspections.finish(request, &conversation_id)
                    || !self
                        .mailbox
                        .as_ref()
                        .is_some_and(|mailbox| mailbox.shows(&folder))
                {
                    return Effects::none();
                }
                match result {
                    Ok(Some(split_rows)) => {
                        if let Some(mailbox) = self.mailbox.as_mut()
                            && mailbox.inspection_revision() == revision
                        {
                            mailbox.split_conversation(&folder, &conversation_id, split_rows);
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        self.end_session_if_expired(Some(error));
                    }
                }
                return self.start_inspections();
            }
            Message::ConversationLoaded(request, result) => {
                // Only the load the reader waits for opens anything. A late
                // answer for a conversation left behind must not mark the one
                // on screen read: it may have been marked unread meanwhile.
                let awaited = self
                    .mailbox
                    .as_ref()
                    .is_some_and(|mailbox| mailbox.awaits(&request));
                let error = self
                    .mailbox
                    .as_mut()
                    .and_then(|mailbox| mailbox.finish_conversation(&request, result));
                self.end_session_if_expired(error);
                if awaited {
                    return self.mark_opened_read();
                }
            }
            Message::FoldersLoaded(epoch, result) => {
                if !self.is_current_session(epoch) {
                    return Effects::none();
                }
                match result {
                    Ok(folders) => self.folders = folders,
                    // Without them the sidebar shows Proton's own until the
                    // next refresh asks again. A list that did not arrive says
                    // nothing about the open folder, which stays open.
                    Err(error) => {
                        self.refetch_folders = true;
                        self.end_session_if_expired(Some(error));
                        return Effects::none();
                    }
                }
                // Counts were asked for before the folders were known, so ask
                // again now that they are.
                return Effects::batch([self.reconcile_open_folder(), self.reload_counts()]);
            }
            Message::CountsLoaded(request, result) => {
                let error = self
                    .mailbox
                    .as_mut()
                    .and_then(|mailbox| mailbox.finish_counts(request, result));
                self.end_session_if_expired(error);
            }
            Message::NewMailLoaded(epoch, request, result) => {
                if !self.is_current_session(epoch)
                    || !self.auth_state.is_authenticated()
                    || !self.settings.desktop_notifications
                    || self.new_mail.pending != Some(request)
                {
                    return Effects::none();
                }
                self.new_mail.pending = None;
                match result {
                    Ok(messages) => {
                        if let Some(mail) = self.new_mail.observe(&messages) {
                            return Effects::ui(UiEffect::NotifyNewMail {
                                sender: mail.sender.clone(),
                                subject: mail.subject.clone(),
                            });
                        }
                    }
                    Err(error) => {
                        self.end_session_if_expired(Some(error));
                    }
                }
            }
            Message::ActionFinished(request, result) => {
                return self.finish_mail_action(request, result);
            }
            Message::MarkReadFinished(request, result) => {
                return self.finish_mark_read(request, result);
            }
            Message::LinkClicked(target) => {
                let Some(link) = PendingLink::parse(&target) else {
                    return Effects::none();
                };
                if !self.settings.confirm_links {
                    return self.open_link_target(link.url);
                }
                self.pending_link = Some(link);
            }
            Message::OpenLink => {
                if let Some(link) = self.pending_link.take() {
                    return self.open_link_target(link.url);
                }
            }
            Message::CopyLink => {
                if let Some(link) = self.pending_link.take() {
                    return Effects::ui(UiEffect::CopyText(link.url));
                }
            }
            Message::DismissLink => self.pending_link = None,
            // Copying takes the reader nowhere, so it needs no confirmation.
            Message::CopyLinkAddress(target) => {
                if let Some(link) = PendingLink::parse(&target) {
                    return Effects::ui(UiEffect::CopyText(link.url));
                }
            }
            Message::CopyAddresses(addresses) => {
                return Effects::ui(UiEffect::CopyText(addresses.join(", ")));
            }
            Message::ReceiveMailto(request) => self.receive_mailto(request),
            Message::MailtoRejected => {
                self.mailto_error = Some("This email link could not be opened.")
            }
            Message::OpenPendingMailto => self.open_pending_mailto(),
            Message::DismissPendingMailto => {
                self.pending_mailto.pop_front();
            }
            Message::DismissMailtoError => self.mailto_error = None,
            Message::PanelsResized(panels) => {
                self.remember(|settings| settings.panels = panels);
            }
            Message::SettingsWritten(revision, written) => {
                self.finish_settings_write(revision, written);
            }
            Message::ResetLayout => {
                self.remember(|settings| {
                    settings.window = Window::default();
                    settings.panels = Panels::default();
                });
                return Effects::ui(UiEffect::ResetLayout);
            }
            Message::KeyPressed(press) => return self.handle_key(press),
            // A drag reports many sizes; only a real change is worth a write.
            Message::WindowResized(window) => {
                let saved = self.settings.window;
                let far = |a: f32, b: f32| (a - b).abs() >= RESIZE_STEP;
                let moved = match (window.position, saved.position) {
                    (Some([x, y]), Some([saved_x, saved_y])) => far(x, saved_x) || far(y, saved_y),
                    (now, before) => now != before,
                };
                let changed = far(window.width, saved.width)
                    || far(window.height, saved.height)
                    || window.maximized != saved.maximized
                    || moved;
                if changed {
                    self.remember(|settings| settings.window = window);
                }
            }
            Message::SaveAttachment(message_id, attachment_id) => {
                return self.save_attachment(message_id, attachment_id, None);
            }
            Message::SaveAttachmentAs(message_id, attachment_id, name) => {
                return self.save_attachment(message_id, attachment_id, Some(name));
            }
            Message::AttachmentDestinationChosen(request, path) => {
                return self.attachment_destination_chosen(request, path);
            }
            Message::AttachmentSaved(request, outcome) => {
                if !self.is_current_session(request.session)
                    || !self
                        .saving_attachment
                        .as_ref()
                        .is_some_and(|save| save.request == request && !save.choosing_destination)
                {
                    return Effects::none();
                }
                self.saving_attachment = None;
                // An expired session ends here as it does for any request.
                if matches!(outcome, Err(SaveError::SessionExpired)) {
                    self.close_mailbox(Some(AuthError::SessionExpired));
                    return Effects::none();
                }
                // A save that landed after the session ended has nobody left
                // to tell, and its news must not surface in the next one.
                if self.mailbox.is_some() {
                    match outcome {
                        Ok(path) => self.notice = Some(Notice::Saved(path)),
                        Err(error) => self.failed_save = Some((request.attachment_id, error)),
                    }
                }
            }
            Message::RevealAttachment(path) => return reveal_in_file_manager(path),
            Message::ShowSettings(showing) => self.showing_settings = showing,
            Message::SetMarkReadOnOpen(on) => {
                self.remember(|settings| settings.mark_read_on_open = on);
            }
            Message::SetConfirmLinks(on) => self.remember(|settings| settings.confirm_links = on),
            Message::SetExpandAllMessages(on) => {
                self.remember(|settings| settings.expand_all_messages = on);
            }
            Message::SetShowQuotedText(on) => {
                self.remember(|settings| settings.show_quoted_text = on);
            }
            Message::SetZoom(zoom) => self.remember(|settings| settings.zoom = zoom),
            Message::SetAppearance(appearance) => {
                self.remember(|settings| {
                    settings.appearance = appearance;
                    settings.follow_system_appearance = false;
                });
            }
            Message::SetFollowSystemAppearance(on) => {
                self.remember(|settings| settings.follow_system_appearance = on);
            }
            Message::SetSystemFont(enabled) => {
                self.remember(|settings| settings.system_font = enabled)
            }
            Message::SetShowUnreadBadge(on) => {
                self.remember(|settings| settings.show_unread_badge = on);
            }
            Message::SetDesktopNotifications(on) => {
                self.remember(|settings| settings.desktop_notifications = on);
                self.new_mail = notifications::NewMail::default();
                if on && !self.is_demo() {
                    return self.fetch_new_mail_notification();
                }
            }
            Message::SetStartFolder(start) => self.remember(|settings| settings.start = start),
            Message::SetComposeFormat(format) => {
                self.remember(|settings| settings.compose_format = format);
            }
            Message::SetComposePlacement(placement) => {
                self.remember(|settings| settings.compose_placement = placement);
            }
            Message::SetCheckRecipientProtection(on) => self.set_check_recipient_protection(on),
            Message::SetAcceptInternationalDomains(on) => {
                self.set_accept_international_domains(on);
            }
            Message::OpenCompose => {
                self.showing_settings = false;
                return self.open_compose();
            }
            Message::Answer { message_id, kind } => return self.answer(&message_id, kind),
            Message::CloseCompose => self.close_compose(),
            Message::ContinueWriting => self.continue_writing(),
            Message::DiscardCompose => self.discard_compose(),
            Message::CancelDiscard => self.cancel_discard(),
            Message::ToggleComposeCopies => self.toggle_compose_copies(),
            Message::ToggleComposeQuote => self.toggle_compose_quote(),
            Message::ComposeChanged(field, value) => self.change_compose(field, value),
            Message::ComposeRecipientsLeft => return self.check_compose_protection(),
            Message::ComposeProtectionChecked(epoch, id, checks) => {
                self.finish_protection_check(epoch, id, checks);
            }
            Message::PickComposeAttachments => return self.pick_compose_attachments(),
            Message::AddComposeAttachments(id, paths) => {
                return self.add_compose_attachments(id, paths);
            }
            Message::AttachmentSizesRead(id, sizes) => self.finish_attachment_sizes(id, sizes),
            Message::RemoveComposeAttachment(index) => self.remove_compose_attachment(index),
            Message::Send => return self.send_compose(),
            Message::Sent(epoch, result) => return self.finish_send(epoch, result),
        }

        Effects::none()
    }

    pub fn mailbox(&self) -> Option<&Mailbox> {
        self.mailbox.as_ref()
    }

    pub(crate) fn accepts_page(&self, request: RequestId) -> bool {
        self.auth_state.is_authenticated()
            && self
                .mailbox
                .as_ref()
                .is_some_and(|mailbox| mailbox.status().request_in_flight() == Some(request))
    }

    pub fn is_demo(&self) -> bool {
        matches!(self.backend, Some(MailBackend::Demo(_)))
    }

    pub fn mailbox_actions_available(&self) -> bool {
        self.backend.is_some()
    }

    /// Whether a background-triggered list request can start without racing
    /// another list-changing operation.
    pub fn auto_refresh_available(&self) -> bool {
        self.auth_state.is_authenticated()
            && self.backend.is_some()
            && self
                .mailbox
                .as_ref()
                .is_some_and(Mailbox::auto_refresh_available)
            && !self.compose.as_ref().is_some_and(Compose::in_flight)
    }

    /// Changes Proton has not confirmed yet, which quitting now would lose.
    pub fn unsent_changes(&self) -> usize {
        self.mailbox.as_ref().map_or(0, Mailbox::unsent_changes)
    }

    /// The mailbox, for tests outside this module that put actions on their
    /// way as Proton's would be.
    #[cfg(test)]
    pub(crate) fn mailbox_mut(&mut self) -> Option<&mut Mailbox> {
        self.mailbox.as_mut()
    }

    /// Puts the app in `state`, for tests outside this module that need one,
    /// such as signing out, which the demo never stays in.
    #[cfg(test)]
    pub(crate) fn set_auth_state(&mut self, state: AuthState) {
        self.auth_state = state;
    }

    pub fn panels(&self) -> Panels {
        self.settings.panels
    }

    pub fn pending_link(&self) -> Option<&PendingLink> {
        self.pending_link.as_ref()
    }

    /// Whether the refresh on its way is one the app started by itself, which
    /// the list leaves unannounced.
    pub fn refreshing_on_its_own(&self) -> bool {
        self.mailbox.as_ref().is_some_and(|mailbox| {
            matches!(mailbox.status(), ListStatus::Refreshing(id) if Some(id) == self.automatic_refresh)
        })
    }

    fn open_link_target(&mut self, url: String) -> Effects {
        if url
            .get(..7)
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case("mailto:"))
        {
            self.update(
                crate::mailto::Request::parse(&url)
                    .map(Message::ReceiveMailto)
                    .unwrap_or(Message::MailtoRejected),
            )
        } else {
            open_in_browser(url)
        }
    }

    fn next_request(&mut self) -> RequestId {
        self.last_request += 1;
        self.last_request
    }

    /// Mailbox actions are accepted only while fully authenticated.
    fn active_mailbox(&mut self) -> Option<&mut Mailbox> {
        match self.auth_state {
            AuthState::Authenticated { .. } => self.mailbox.as_mut(),
            _ => None,
        }
    }

    fn select_conversation(&mut self, id: String) -> Effects {
        if !self.auth_state.is_authenticated() {
            return Effects::none();
        }
        self.pending_link = None;
        if let Some(MailBackend::Demo(service)) = self.backend.clone()
            && !self.defer_read(&id)
        {
            if !service.set_unread(&id, false) {
                return Effects::none();
            }
            self.apply_demo_snapshot(&service);
        }

        self.start_conversation_load(id)
    }

    fn apply_action(&mut self, action: MailAction) -> Effects {
        let Some(selected) = self
            .mailbox
            .as_ref()
            .and_then(Mailbox::selected_summary)
            .map(|summary| summary.id.clone())
        else {
            return Effects::none();
        };
        self.apply_row_action(selected, action)
    }

    /// Acts on one listed row, open or not. Only the open one, when it leaves
    /// the view, gives its place in the reader to the row after it.
    fn apply_row_action(&mut self, row_id: String, action: MailAction) -> Effects {
        if !self.auth_state.is_authenticated() {
            return Effects::none();
        }
        match self.backend.clone() {
            Some(MailBackend::Demo(service)) => self.apply_demo_action(&service, &row_id, action),
            Some(MailBackend::Proton(service)) => self.start_proton_action(service, row_id, action),
            None => Effects::none(),
        }
    }

    /// Demo actions change local data at once and reload the folder snapshot.
    fn apply_demo_action(
        &mut self,
        service: &DemoMailbox,
        id: &str,
        action: MailAction,
    ) -> Effects {
        let Some(mailbox) = self.mailbox.as_ref() else {
            return Effects::none();
        };
        if !mailbox.takes_actions() {
            return Effects::none();
        }
        let Some(kind) = mailbox.row(id).map(|row| row.kind) else {
            return Effects::none();
        };

        let applied = match &action {
            MailAction::MoveTo(folder) => service.move_to(id, folder),
            MailAction::SetUnread(unread) => service.set_unread(id, *unread),
            MailAction::SetStarred(starred) => service.set_starred(id, *starred),
            // The demo account owns no labels, so the reader offers none.
            MailAction::SetLabel { .. } => false,
        };
        if applied {
            // Demo actions never reach `finish_action`, so the offer to take
            // the move back, and the change itself, are recorded here instead.
            if let Some(mailbox) = self.active_mailbox() {
                mailbox.record_action(id, &action);
                mailbox.offer_undo(id, kind, action);
            }
            if let Some(next) = self.apply_demo_snapshot(service) {
                return self.select_conversation(next);
            }
        }

        Effects::none()
    }

    /// Puts the last moved conversation back. The row has left the list, so
    /// the action names it directly instead of going through the selection.
    fn undo_move(&mut self) -> Effects {
        let Some(undo) = self.active_mailbox().and_then(Mailbox::take_undo) else {
            return Effects::none();
        };
        match self.backend.clone() {
            Some(MailBackend::Demo(service)) => {
                if service.move_to(&undo.row_id, &undo.from) {
                    self.apply_demo_snapshot(&service);
                }

                Effects::none()
            }
            Some(MailBackend::Proton(service)) => {
                let request = self.next_request();
                let Some(start) = self
                    .active_mailbox()
                    .and_then(|mailbox| mailbox.start_move_back(undo, request))
                else {
                    return Effects::none();
                };
                self.show_started_action(&service, start)
            }
            None => Effects::none(),
        }
    }

    /// Proton actions show at once and run asynchronously, one after
    /// another; a refused one is taken back when Proton answers.
    fn start_proton_action(
        &mut self,
        service: Arc<ProtonMailService>,
        row_id: String,
        action: MailAction,
    ) -> Effects {
        let request = self.next_request();
        let Some(start) = self.active_mailbox().and_then(|mailbox| {
            let kind = mailbox.row(&row_id)?.kind;
            mailbox.start_action_on(row_id, kind, action, request)
        }) else {
            return Effects::none();
        };
        self.show_started_action(&service, start)
    }

    /// Sends what is ready, and opens the row that took the place of one the
    /// action took out of the view.
    fn show_started_action(
        &mut self,
        service: &Arc<ProtonMailService>,
        start: ActionStart,
    ) -> Effects {
        let send = start.send.map_or_else(Effects::none, |request| {
            run_action(Arc::clone(service), request, Message::ActionFinished)
        });
        let open = start
            .opened
            .map_or_else(Effects::none, |id| self.open_row(id));
        Effects::batch([send, open])
    }

    /// Sends the next action in line, once the one before it has answered.
    fn send_action(&self, request: Option<ActionRequest>) -> Effects {
        match (request, self.backend.clone()) {
            (Some(request), Some(MailBackend::Proton(service))) => {
                run_action(service, request, Message::ActionFinished)
            }
            _ => Effects::none(),
        }
    }

    fn finish_mail_action(
        &mut self,
        request: ActionRequest,
        result: Result<(), MailboxError>,
    ) -> Effects {
        let Some(finish) = self
            .mailbox
            .as_mut()
            .map(|mailbox| mailbox.finish_action(&request, result))
        else {
            return Effects::none();
        };
        if let Err(error) = finish.result {
            // Proton is where the row really is: a row a refused move took
            // out of the view comes back with the list.
            let restore = self
                .mailbox
                .as_ref()
                .is_some_and(|mailbox| mailbox.took_out(&request));
            // An expired session closes the mailbox, and nothing is sent.
            if self.end_session_if_expired(Some(error)) {
                return Effects::none();
            }
            let reload = if restore {
                self.refresh_mailbox()
            } else {
                Effects::none()
            };
            return Effects::batch([self.send_action(finish.next_action), reload]);
        }
        let send_next = self.send_action(finish.next_action);

        // Reload only when undo returns a missing row to the visible folder.
        let arrived = self
            .mailbox
            .as_ref()
            .is_some_and(|mailbox| mailbox.brought_back(&request));
        if arrived {
            return Effects::batch([send_next, self.refresh_mailbox()]);
        }

        let counts = self.reload_counts();
        let open_next = finish
            .opened
            .map_or_else(Effects::none, |id| self.open_row(id));

        Effects::batch([send_next, counts, open_next])
    }

    /// Once an unread Proton row's content is shown, marks it read on Proton.
    /// Demo rows are already marked read when they are selected, unless a
    /// draft covers the reader.
    fn mark_opened_read(&mut self) -> Effects {
        if !self.settings.mark_read_on_open {
            return Effects::none();
        }
        let Some(MailBackend::Proton(service)) = self.backend.clone() else {
            return Effects::none();
        };
        if let Some(id) = self.selected_conversation_id()
            && self.defer_read(&id)
        {
            return Effects::none();
        }
        let request = self.next_request();
        let Some(request) = self
            .active_mailbox()
            .and_then(|mailbox| mailbox.start_mark_read(request))
        else {
            return Effects::none();
        };
        run_action(service, request, Message::MarkReadFinished)
    }

    fn selected_conversation_id(&self) -> Option<String> {
        self.mailbox
            .as_ref()
            .and_then(Mailbox::selected_conversation)
            .map(str::to_owned)
    }

    /// Whether a draft covers the reader, so the conversation `id` opens
    /// where nobody sees it. It then stays unread until the reader shows it.
    fn defer_read(&mut self, id: &str) -> bool {
        if !self.draft_covers_reader() {
            return false;
        }
        self.read_once_shown = Some(id.to_owned());
        true
    }

    /// Marks read what was opened while a draft covered the reader, now that
    /// the reader shows it; nothing if another conversation is open by then.
    fn mark_read_once_shown(&mut self) -> Effects {
        let Some(id) = self.read_once_shown.take() else {
            return Effects::none();
        };
        if self.selected_conversation_id().as_deref() != Some(id.as_str()) {
            return Effects::none();
        }
        match self.backend.clone() {
            Some(MailBackend::Demo(service)) => {
                if service.set_unread(&id, false) {
                    self.apply_demo_snapshot(&service);
                }
                Effects::none()
            }
            Some(MailBackend::Proton(_)) => self.mark_opened_read(),
            None => Effects::none(),
        }
    }

    fn finish_mark_read(
        &mut self,
        request: ActionRequest,
        result: Result<(), MailboxError>,
    ) -> Effects {
        let Some(completion) = self
            .mailbox
            .as_mut()
            .map(|mailbox| mailbox.finish_mark_read(&request, result))
        else {
            return Effects::none();
        };
        // The mailbox also records an error for any queued explicit action.
        // An expired session closes the mailbox, and then nothing is sent.
        if let Some(error) = completion.error {
            if self.end_session_if_expired(Some(error)) {
                return Effects::none();
            }
            return self.send_action(completion.next_action);
        }
        let next = self.send_action(completion.next_action);
        let counts = if completion.read_confirmed {
            self.reload_counts()
        } else {
            Effects::none()
        };
        Effects::batch([next, counts])
    }

    /// Proton owns the folder counts; reload them after a change.
    fn reload_counts(&mut self) -> Effects {
        let counts_request = self.next_request();
        let counts = self
            .active_mailbox()
            .map(|mailbox| mailbox.refresh_counts(counts_request));

        self.fetch_counts(counts)
    }

    fn start_conversation_load(&mut self, id: String) -> Effects {
        let request = self.next_request();
        let Some(mailbox) = self.active_mailbox() else {
            return Effects::none();
        };
        let previous = mailbox.selected_conversation().map(str::to_owned);
        let request = mailbox.start_conversation_load(id, request);
        let selection_changed = mailbox.selected_conversation() != previous.as_deref();
        if request.is_none() && !selection_changed {
            return Effects::none();
        }

        let content = match request {
            Some(request) => self.fetch_conversation(Some(request)),
            None => self.mark_opened_read(),
        };

        // A new conversation starts at its top, not where the last one was
        // left; the reader pane keeps its scroll offset otherwise.
        Effects::batch([Effects::ui(UiEffect::ScrollReaderTop), content])
    }

    /// Opens a row the user did not click, such as the neighbour a list key
    /// reaches or the one after a row a move took away, and brings it into
    /// view.
    fn open_row(&mut self, id: String) -> Effects {
        Effects::batch([self.reveal_in_list(&id), self.select_conversation(id)])
    }

    /// Brings a row the app opened on its own into view. A row the user
    /// clicked is already on screen, so only these are worth scrolling to.
    fn reveal_in_list(&self, row_id: &str) -> Effects {
        let visible = self
            .mailbox
            .as_ref()
            .is_some_and(|mailbox| mailbox.visible_position(row_id).is_some());
        if !visible {
            return Effects::none();
        }

        Effects::ui(UiEffect::RevealConversation(row_id.to_owned()))
    }

    fn retry_conversation(&mut self) -> Effects {
        let request = self.next_request();
        let request = self
            .active_mailbox()
            .and_then(|mailbox| mailbox.retry_conversation(request));

        self.fetch_conversation(request)
    }

    fn apply_demo_snapshot(&mut self, service: &DemoMailbox) -> Option<String> {
        let mailbox = self.mailbox.as_ref()?;
        let folder = mailbox.folder().clone();
        let page_size = mailbox.loaded_conversation_limit();
        let (page, counts) = service.snapshot(&folder, page_size, crate::mail::demo::now());

        self.active_mailbox()?.apply_action_snapshot(page, counts)
    }

    /// Reconciles the saved folder with the account's current folders.
    fn reconcile_open_folder(&mut self) -> Effects {
        let Some(open) = self
            .mailbox
            .as_ref()
            .map(|mailbox| mailbox.folder().clone())
        else {
            return Effects::none();
        };
        // Proton's own folders are always there, so only the account's own
        // can have gone stale.
        let Some(id) = open.custom_id() else {
            return Effects::none();
        };
        let current = self
            .folders
            .iter()
            .find(|place| place.custom_id() == Some(id))
            .cloned();
        // Gone from the account means gone from the sidebar, and the Inbox is
        // the one place that is always there to fall back to.
        let Some(current) = current else {
            return self.select_folder(Folder::INBOX);
        };
        if current == open {
            return Effects::none();
        }

        // A rename keeps the stable id, loaded mail and reader state.
        if let Some(mailbox) = self.active_mailbox() {
            mailbox.rename_folder(current.clone());
        }
        self.remember_folder(&current);

        Effects::none()
    }

    /// Remembers the last real folder; demo navigation is process-local.
    fn remember_folder(&mut self, folder: &Folder) {
        if !self.is_demo() {
            self.remember(|settings| settings.folder = folder.clone());
        }
    }

    fn select_folder(&mut self, folder: Folder) -> Effects {
        if self.auth_state.is_authenticated()
            && self
                .mailbox
                .as_ref()
                .is_some_and(|mailbox| mailbox.folder() != &folder)
        {
            self.inspections.cancel();
            self.abandon_reader_load();
        }
        self.showing_settings = false;
        self.pending_link = None;
        self.failed_save = None;
        self.remember_folder(&folder);
        let request = self.next_request();
        let page = self
            .active_mailbox()
            .and_then(|mailbox| mailbox.select_folder(folder, request));

        self.fetch_page(page)
    }

    /// Runs what is typed in the search field against the server, which sees
    /// every folder and every conversation, not just the loaded ones.
    fn start_search(&mut self) -> Effects {
        let request = self.next_request();
        let Some(request) = self
            .active_mailbox()
            .and_then(|mailbox| mailbox.start_search(request))
        else {
            return Effects::none();
        };

        self.inspections.cancel();
        self.fetch_search(request)
    }

    fn fetch_search(&self, request: SearchRequest) -> Effects {
        let Some(backend) = self.backend.clone() else {
            return Effects::none();
        };
        let query = request.query.clone();
        let (page, page_size) = (request.page, request.page_size);

        Effects::perform(
            async move { backend.search(&query, page, page_size).await },
            move |result| Message::SearchLoaded(request.clone(), result),
        )
    }

    fn refresh_mailbox(&mut self) -> Effects {
        let page_request = self.next_request();
        let counts_request = self.next_request();
        let Some(mailbox) = self.active_mailbox() else {
            return Effects::none();
        };
        let page = mailbox.refresh(page_request);
        let counts = mailbox.refresh_counts(counts_request);

        if page.is_some() {
            self.inspections.cancel();
        }
        let folders = if self.refetch_folders {
            self.fetch_folders()
        } else {
            Effects::none()
        };

        Effects::batch([
            self.fetch_page(page),
            self.fetch_counts(Some(counts)),
            folders,
        ])
    }

    fn auto_refresh_mailbox(&mut self) -> Effects {
        if !self.auto_refresh_available() {
            return Effects::none();
        }
        if let Some(mailbox) = &mut self.mailbox {
            mailbox.invalidate_cached_listings();
        }

        let refresh = self.refresh_mailbox();
        // Nothing was loading before, so a refresh on its way is this one.
        self.automatic_refresh = self
            .mailbox
            .as_ref()
            .and_then(|mailbox| match mailbox.status() {
                ListStatus::Refreshing(request) => Some(request),
                _ => None,
            });
        let notifications = if self.is_demo() {
            Effects::none()
        } else {
            self.fetch_new_mail_notification()
        };
        Effects::batch([refresh, notifications])
    }

    fn load_more_conversations(&mut self) -> Effects {
        let request = self.next_request();
        let Some(mailbox) = self.active_mailbox() else {
            return Effects::none();
        };
        if mailbox.shown_search_query().is_some() {
            let search = mailbox.load_more_search(request);
            search.map_or_else(Effects::none, |search| self.fetch_search(search))
        } else {
            let page = mailbox.load_more(request);
            self.fetch_page(page)
        }
    }

    fn fetch_page(&self, request: Option<PageRequest>) -> Effects {
        let (Some(request), Some(backend)) = (request, self.backend.clone()) else {
            return Effects::none();
        };

        let (id, folder, page, page_size) =
            (request.id, request.folder, request.page, request.page_size);

        Effects::perform(
            async move { backend.list_conversations(&folder, page, page_size).await },
            move |result| Message::ConversationsLoaded(id, result),
        )
    }

    fn inspect_candidates(&mut self, candidates: Vec<String>) -> Effects {
        let Some(mailbox) = self.mailbox.as_ref() else {
            return Effects::none();
        };
        if mailbox.shown_search_query().is_some() {
            return Effects::none();
        }
        self.inspections.enqueue(
            candidates
                .into_iter()
                .filter(|id| mailbox.has_inspection_target(id)),
        );
        self.start_inspections()
    }

    fn start_inspections(&mut self) -> Effects {
        if !self.auth_state.is_authenticated() {
            return Effects::none();
        }
        let (Some(backend), Some(mailbox)) = (self.backend.clone(), self.mailbox.as_ref()) else {
            return Effects::none();
        };
        let epoch = self.session_epoch;
        let folder = mailbox.folder().clone();
        let revision = mailbox.inspection_revision();
        let mut effects = Vec::new();
        // Each inspection takes the next request ID itself, asked for only
        // when there is one to start, so no ID goes unused.
        while self.inspections.can_start() {
            let request = self.next_request();
            let Some((id, registration)) = self.inspections.start(request) else {
                break;
            };
            // A queued row may have been moved or removed before its turn.
            if !self
                .mailbox
                .as_ref()
                .is_some_and(|mailbox| mailbox.has_inspection_target(&id))
            {
                self.inspections.finish(request, &id);
                continue;
            }
            let backend = backend.clone();
            let target_folder = folder.clone();
            let callback_folder = folder.clone();
            let conversation_id = id.clone();
            let future = Abortable::new(
                async move {
                    backend
                        .inspect_conversation(&conversation_id, &target_folder)
                        .await
                },
                registration,
            );
            effects.push(Effects::perform(future, move |result| {
                Message::ThreadInspected {
                    epoch,
                    request,
                    revision,
                    folder: callback_folder,
                    conversation_id: id,
                    result: result.unwrap_or(Ok(None)),
                }
            }));
        }
        Effects::batch(effects)
    }

    /// Asks for the folders the account made. They change rarely, so only a
    /// list that failed to load is asked for again, by the next refresh.
    fn fetch_folders(&mut self) -> Effects {
        self.refetch_folders = false;
        let Some(backend) = self.backend.clone() else {
            return Effects::none();
        };
        let epoch = self.session_epoch;

        Effects::perform(async move { backend.list_folders().await }, move |result| {
            Message::FoldersLoaded(epoch, result)
        })
    }

    fn fetch_counts(&self, request: Option<RequestId>) -> Effects {
        let (Some(request), Some(backend)) = (request, self.backend.clone()) else {
            return Effects::none();
        };

        let folders = self.folders.clone();

        Effects::perform(
            async move { backend.conversation_counts(&folders).await },
            move |result| Message::CountsLoaded(request, result),
        )
    }

    fn fetch_new_mail_notification(&mut self) -> Effects {
        if !self.auth_state.is_authenticated()
            || !self.settings.desktop_notifications
            || self.new_mail.pending.is_some()
        {
            return Effects::none();
        }
        let Some(backend) = self.backend.clone() else {
            return Effects::none();
        };
        let epoch = self.session_epoch;
        let request = self.next_request();
        self.new_mail.pending = Some(request);

        Effects::perform(
            async move { backend.recent_inbox(notifications::RECENT_LIMIT).await },
            move |result| Message::NewMailLoaded(epoch, request, result),
        )
    }

    fn fetch_conversation(&mut self, request: Option<ReaderRequest>) -> Effects {
        let (Some(request), Some(backend)) = (request, self.backend.clone()) else {
            return Effects::none();
        };
        // Moving through rows with j/k starts a load per row; only the newest
        // is still wanted, so the others stop downloading and decrypting.
        self.abandon_reader_load();
        let (handle, registration) = AbortHandle::new_pair();
        self.reader_load = Some(handle);
        let (kind, id, messages) = (
            request.kind,
            request.conversation_id.clone(),
            request.messages,
        );

        Effects::perform(
            Abortable::new(
                async move { backend.conversation_detail(kind, &id, messages).await },
                registration,
            ),
            // The reader no longer waits for an abandoned load and ignores
            // its answer.
            move |result| {
                Message::ConversationLoaded(
                    request.clone(),
                    result.unwrap_or(Err(MailboxError::Unavailable)),
                )
            },
        )
    }

    /// Stops the conversation load in flight, if any: nobody will read it.
    fn abandon_reader_load(&mut self) {
        if let Some(load) = self.reader_load.take() {
            load.abort();
        }
    }

    /// Signs out when Proton ended the session, and says whether it did. Any
    /// other error is left as it is. A session that could not be read or
    /// saved only shows its message: unlocking the password storage fixes
    /// the next request, and signing out would drop the draft.
    fn end_session_if_expired(&mut self, error: Option<MailboxError>) -> bool {
        let expired = error == Some(MailboxError::SessionExpired);
        if expired {
            self.close_mailbox(Some(AuthError::SessionExpired));
        }
        expired
    }
}

/// Why an attachment that did not arrive was not saved.
fn unsaved(error: MailboxError) -> SaveError {
    match error {
        MailboxError::AttachmentVerificationFailed => SaveError::VerificationFailed,
        MailboxError::SessionExpired => SaveError::SessionExpired,
        MailboxError::SessionUnavailable => SaveError::SessionUnavailable,
        MailboxError::Connection | MailboxError::Service | MailboxError::Unavailable => {
            SaveError::NotFetched
        }
    }
}

/// A link clicked in a message, waiting for the user to confirm it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingLink {
    /// The complete, normalized address that opens.
    pub url: String,
    /// Where the link really goes: the web host or the mail address.
    pub target: String,
}

impl PendingLink {
    /// Accepts only web and mail links.
    fn parse(link: &str) -> Option<Self> {
        let url = url::Url::parse(link.trim()).ok()?;
        let target = match url.scheme() {
            "http" | "https" => url.host_str()?.to_owned(),
            "mailto" => mail_target(url.as_str()).unwrap_or_else(|| url.path().to_owned()),
            _ => return None,
        };

        Some(Self {
            url: url.into(),
            target,
        })
    }
}

/// Who a mail link writes to: the addresses the composer would take from it,
/// those after `?to=` included, as the first few and how many more. `None`
/// when the composer would refuse the link.
fn mail_target(link: &str) -> Option<String> {
    const NAMED: usize = 3;
    let (to, _, _) = crate::mailto::Request::parse(link)?.into_fields();
    let addresses: Vec<_> = to
        .split(", ")
        .filter(|address| !address.is_empty())
        .collect();
    if addresses.len() <= NAMED {
        return Some(addresses.join(", "));
    }
    Some(format!(
        "{} and {} more",
        addresses[..NAMED].join(", "),
        addresses.len() - NAMED
    ))
}

/// Runs one mailbox action on Proton and reports the outcome under
/// `finished`, which is what tells an automatic read from a deliberate one.
fn run_action(
    service: Arc<ProtonMailService>,
    request: ActionRequest,
    finished: fn(ActionRequest, Result<(), MailboxError>) -> Message,
) -> Effects {
    let pending = request.clone();

    Effects::perform(
        async move {
            service
                .apply_action(
                    pending.kind,
                    &pending.row_id,
                    pending.context.as_ref(),
                    pending.action,
                )
                .await
        },
        move |result| finished(request.clone(), result),
    )
}

fn open_in_browser(url: String) -> Effects {
    Effects::background(async move {
        let _ = open_url(&url).await;
    })
}

/// Opens `url` in the default browser without going through a shell.
async fn open_url(url: &str) -> std::io::Result<()> {
    let mut opener = ruston_core::browser::opener()?;
    opener.arg(url);
    crate::runtime::run_to_exit(opener).await.map(drop)
}

fn reveal_in_file_manager(path: PathBuf) -> Effects {
    Effects::background(async move {
        let _ = crate::file_manager::reveal(&path).await;
    })
}

/// Writes revision `revision` of the settings with `write`, unless the file
/// already holds it or a newer one. The lock keeps writes in turn, and the
/// check keeps one that started early and finishes late, such as the last
/// write running when the app quits, from undoing a newer one.
fn write_settings_revision(
    on_disk: &Mutex<u64>,
    revision: u64,
    write: impl FnOnce() -> bool,
) -> bool {
    let mut newest = on_disk
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if *newest >= revision {
        return true;
    }
    let written = write();
    if written {
        *newest = revision;
    }
    written
}

#[cfg(test)]
mod tests;
