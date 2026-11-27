mod accounts;
mod attachments;
mod auth;
mod compose;
mod effect;
mod inspection;
mod keys;
mod layout;
mod links;
mod listing;
mod mailbox;
mod mailto;
mod notifications;
mod pictures;
mod reader;
mod reads;
mod updates;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use futures::future::{AbortHandle, Abortable};
use inspection::Inspections;

use crate::downloads::SaveError;
use crate::releases::{self, Version};
use crate::settings::{Appearance, ComposePlacement, Panels, Settings, StartFolder, Window};

/// How far the window must change before its new size is worth storing.
const RESIZE_STEP: f32 = 8.0;

use crate::mail::{
    AuthError, BodyFormat, ConversationDetail, ConversationPage, ConversationSummary, Folder,
    IncomingMail, ListFilter, MailAction, MailBackend, MailboxCounts, MailboxError, OwnPlaces,
    PlaceLook, ProtectionCheck, ProtonMailService, ResumeOutcome, SendError, SignInOutcome,
    SignInPrompt, SummaryKind, demo::DemoMailbox,
};

use attachments::AttachmentSave;
pub use attachments::AttachmentSaveRequest;
use auth::LoginForm;
pub use auth::{AuthState, SignInStep};
use compose::ParkedDraft;
pub use compose::{AnswerKind, Answering, Compose, ComposeField, ComposeId, Sending};
pub use effect::{Effect, Effects, UiEffect};
pub use keys::{Key, KeyPress};
pub use layout::{
    MIN_PANEL_WIDTH, MIN_READER_WIDTH, compact as compact_layout, ratios as panel_ratios,
    widths as panel_widths, widths_beside_rail,
};
pub use links::PendingLink;
use mailbox::ActionStart;
#[cfg(test)]
pub use mailbox::MovedRow;
use mailbox::PageRequest;
pub(crate) use mailbox::RequestId;
pub use mailbox::{
    ActionRequest, DeviceIndex, ListAttempt, ListStatus, Mailbox, ReaderRequest, SearchRequest,
    Step, UndoMove,
};
pub use mailto::MailtoError;
pub use pictures::InlineImage;
pub use reader::{ConversationReader, ReaderState};
pub use updates::UpdateStatus;

type SessionEpoch = u64;
pub(crate) type AuthAttempt = u64;
const PROTON_SIGNUP_URL: &str = "https://account.proton.me/mail/signup";

/// The parts Settings is split into, each shown on its own, in the order
/// its navigation lists them. One draft covers them all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SettingsSection {
    #[default]
    Reading,
    Writing,
    Appearance,
    General,
    Advanced,
    Keyboard,
    About,
}

impl SettingsSection {
    pub const ALL: [Self; 7] = [
        Self::Reading,
        Self::Writing,
        Self::Appearance,
        Self::General,
        Self::Advanced,
        Self::Keyboard,
        Self::About,
    ];
}

#[derive(Clone)]
pub enum Message {
    Submit,
    CancelChallenge,
    /// What resuming the session saved under the profile found.
    SessionChecked(String, ResumeOutcome),
    /// Tries the saved session again after a failure that may pass.
    RetrySavedSession,
    /// Opens the account whose session is saved under this profile.
    SwitchAccount(String),
    /// Leaves the open account for the sign-in screen, to add another.
    AddAccount,
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
    /// What signing out of the session just made for an account added
    /// already came to.
    DuplicateLogoutFinished(SessionEpoch, Result<(), AuthError>),
    SelectFolder(Folder),
    /// Lists only the open folder's unread conversations, or all of them
    /// again.
    ShowOnly(ListFilter),
    SelectConversation(String),
    /// Adds a row to the rows chosen, or takes it out, as a Command click
    /// (Ctrl on Windows and Linux) does.
    ToggleSelected(String),
    /// Chooses the rows from the last one clicked through this one, as a
    /// Shift click does.
    SelectThrough(String),
    RetryConversation,
    SearchChanged(String),
    /// Asks the server for the first page matching what was typed, across folders.
    SearchSubmitted,
    SearchLoaded(SearchRequest, Result<ConversationPage, MailboxError>),
    /// What this device's index found for a search Proton could not answer.
    DeviceSearchLoaded(
        SearchRequest,
        Result<Option<ConversationPage>, MailboxError>,
    ),
    ToggleMessageExpanded(String),
    /// Folds or unfolds one quoted passage of a message.
    ToggleQuoteExpanded(String, usize),
    /// Applies one action to the rows chosen: those selected together, or
    /// the open one. Adding an action is a line in the toolbar rather than a
    /// message and an arm of its own.
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
    ConversationPreloaded(ReaderRequest, Result<ConversationDetail, MailboxError>),
    CountsLoaded(RequestId, Result<MailboxCounts, MailboxError>),
    NewMailLoaded(
        SessionEpoch,
        RequestId,
        Result<Vec<IncomingMail>, MailboxError>,
    ),
    /// The folders the account made, fetched when the mailbox opens and, if
    /// that fails, again with the next refresh.
    FoldersLoaded(SessionEpoch, Result<OwnPlaces, MailboxError>),
    /// Proton's answer to one request: the actions sent in it, in order.
    ActionFinished(Vec<ActionRequest>, Result<(), MailboxError>),
    MarkReadFinished(ActionRequest, Result<(), MailboxError>),
    PanelsResized(Panels),
    /// Folds the sidebar into a strip of icons, or unfolds it.
    FoldSidebar(bool),
    /// A settings write finished: the revision it wrote, and whether the file
    /// holds it now.
    SettingsWritten(u64, bool),
    /// Restores the default window size and mailbox pane widths.
    ResetLayout,
    /// A key the focused widget did not take.
    KeyPressed(KeyPress),
    /// The window changed size, which is worth remembering for next time.
    WindowResized(Window),
    /// Fetches one attachment of a message and saves it in the downloads
    /// folder.
    SaveAttachment {
        message_id: String,
        attachment_id: String,
    },
    /// Asks where to save an attachment before fetching it, offering `name`.
    SaveAttachmentAs {
        message_id: String,
        attachment_id: String,
        name: String,
    },
    AttachmentDestinationChosen(AttachmentSaveRequest, Option<PathBuf>),
    /// Where an attachment landed, or why it did not.
    AttachmentSaved(AttachmentSaveRequest, Result<PathBuf, SaveError>),
    /// The reader reached a picture the message carries: its message and
    /// the attachment it is.
    LoadInlineImage(String, String),
    /// A picture arrived, decoded, or could not be shown.
    InlineImageLoaded(SessionEpoch, String, Option<Arc<crate::pictures::Picture>>),
    /// Reveals a downloaded file in the system file manager.
    RevealAttachment(PathBuf),
    /// Shows or hides Settings beside the mailbox sidebar.
    ShowSettings(bool),
    /// Shows one part of Settings in place of the one shown.
    ShowSettingsSection(SettingsSection),
    /// Opens the project's own repository from the About card.
    OpenSourceCode,
    /// Opens the notices of the third-party software the package shipped,
    /// from the About card.
    OpenThirdPartyNotices,
    /// Asks GitHub whether a newer release is out, which the window does
    /// while checking for updates is on.
    CheckForUpdates,
    /// The latest published release's number, or none when it could not be
    /// read.
    UpdatesChecked(Option<Version>),
    /// Opens the page of the latest release, from its notice or About.
    OpenReleasePage,
    /// Marks opened mail as read, or leaves it unread.
    SetMarkReadOnOpen(bool),
    /// Asks where a link goes before opening it, or opens it straight away.
    SetConfirmLinks(bool),
    /// Opens every message in a conversation, or only the newest.
    SetExpandAllMessages(bool),
    /// Unfolds quoted passages, or leaves them behind their button.
    SetShowQuotedText(bool),
    /// Shows text the sender hid, set apart, or leaves it out.
    SetShowHiddenText(bool),
    /// Follows the sender's style for bold, italic, struck and centered
    /// text, or only its tags.
    SetSenderTextStyles(bool),
    /// Lays tables of data out in columns, or reads them as lines.
    SetDataTables(bool),
    /// Shows the pictures a message carries within it, or their
    /// descriptions alone.
    SetShowInlineImages(bool),
    /// Scales the whole interface.
    SetZoom(f32),
    /// Draws text at one of the sizes Settings offers, apart from the zoom.
    SetTextScale(f32),
    /// Chooses the desktop color scheme, which stops following the system's.
    SetAppearance(Appearance),
    /// Follows the system's light or dark mode, or the chosen scheme.
    SetFollowSystemAppearance(bool),
    SetSystemFont(bool),
    /// Shows or hides the unread mail badge on the dock/app icon.
    SetShowUnreadBadge(bool),
    /// Shows or hides desktop notifications for incoming mail.
    SetDesktopNotifications(bool),
    /// Keeps a notice at the foot of the window up for this many seconds.
    SetNoticeSeconds(u64),
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
    /// Searches this device's index when Proton cannot be reached, or not.
    SetSearchDeviceOffline(bool),
    /// Asks GitHub as the app starts and once a day whether a newer release
    /// is out, or never.
    SetCheckForUpdates(bool),
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
    /// Leaves one of the original's files, by ID, out of a forward.
    LeaveOutForwarded(String),
    ComposeChanged(ComposeField, String),
    /// Sends the draft from another of the account's addresses.
    ComposeSender(String),
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
    /// A newer release is out, with this number.
    UpdateAvailable(Version),
}

pub struct App {
    /// Set for a process started for the fictional mailbox. Such a process
    /// never signs in to Proton, even after its demo mailbox is closed.
    demo_process: bool,
    auth_state: AuthState,
    /// A saved session was found when the app started, so checking it is
    /// opening the mailbox rather than finding out whether to sign in.
    session_saved: bool,
    login_form: LoginForm,
    auth_error: Option<AuthError>,
    /// The saved session could not open for a reason that may pass, such as
    /// no network, and is still saved; the sign-in screen offers to try it
    /// again.
    resume_may_retry: bool,
    /// The profile a new account signs in under, while one is being added.
    adding_account: Option<String>,
    /// Changes for every authentication attempt so queued results from an
    /// aborted task cannot affect its replacement.
    auth_attempt: AuthAttempt,
    /// The question a running sign-in is waiting on, if any.
    pending_prompt: Option<SignInPrompt>,
    /// A link clicked in a message, waiting for the user to confirm it.
    pending_link: Option<PendingLink>,
    pending_mailto: std::collections::VecDeque<crate::mailto::Request>,
    mailto_error: Option<MailtoError>,
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
    /// The conversation loading into the reader cache in the background. It
    /// stops whenever the reader's load would, and when another replaces it.
    preload: Option<AbortHandle>,
    /// The folders the account made, which the sidebar shows below Proton's
    /// own. Empty until they arrive, and in demo mode for good.
    folders: Vec<Folder>,
    /// How each of `folders` looks in the sidebar, by custom ID.
    place_looks: HashMap<String, PlaceLook>,
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
    /// The part of Settings shown, which it opens to again while the app
    /// runs.
    settings_section: SettingsSection,
    /// The notices of third-party software that came with the app, which
    /// the About card opens. A build run from Cargo has none.
    third_party_notices: Option<PathBuf>,
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
    /// The pictures of the open conversation, by attachment.
    inline_images: HashMap<String, InlineImage>,
    /// The turns those pictures take to be fetched.
    picture_turns: pictures::Turns,
    new_mail: notifications::NewMail,
    /// What checking for updates last found, while it is on.
    update_status: Option<UpdateStatus>,
}

impl App {
    /// In demo mode the app opens a local fictional mailbox and never starts
    /// the Proton session resume.
    pub fn boot(demo: bool, settings: Settings) -> (Self, Effects) {
        let mut app = Self::new(settings);
        app.demo_process = demo;
        app.third_party_notices = crate::licenses::third_party_notices();
        let task = if demo {
            app.open_mailbox(MailBackend::demo(), None, None)
        } else {
            app.session_saved = ProtonMailService::has_saved_session(&app.session_profile());
            let profile = app.session_profile();
            Effects::perform(ProtonMailService::resume(profile.clone()), move |outcome| {
                Message::SessionChecked(profile, outcome)
            })
        };

        (app, task)
    }

    fn new(settings: Settings) -> Self {
        Self {
            demo_process: false,
            auth_state: AuthState::CheckingSession,
            session_saved: false,
            login_form: LoginForm::default(),
            auth_error: None,
            resume_may_retry: false,
            adding_account: None,
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
            preload: None,
            folders: Vec::new(),
            place_looks: HashMap::new(),
            refetch_folders: false,
            automatic_refresh: None,
            settings,
            settings_revision: 0,
            settings_written: 0,
            settings_failed: None,
            settings_writing: false,
            settings_on_disk: Arc::new(Mutex::new(0)),
            showing_settings: false,
            settings_section: SettingsSection::default(),
            third_party_notices: None,
            compose: None,
            parked_draft: None,
            read_once_shown: None,
            last_compose_id: 0,
            notice: None,
            failed_save: None,
            saving_attachment: None,
            inline_images: HashMap::new(),
            picture_turns: pictures::Turns::default(),
            new_mail: notifications::NewMail::default(),
            update_status: None,
        }
    }

    fn next_compose_id(&mut self) -> ComposeId {
        self.last_compose_id = self
            .last_compose_id
            .checked_add(1)
            .expect("compose id exhausted");
        self.last_compose_id
    }

    /// The session profile the account signs in and resumes under: a new
    /// account's while one is being added, otherwise the open account's.
    fn session_profile(&self) -> String {
        self.adding_account
            .clone()
            .unwrap_or_else(|| self.settings.accounts.active_profile().to_owned())
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

    /// The confirmation to show at the foot of the mailbox.
    pub fn notice(&self) -> Option<&Notice> {
        self.notice.as_ref()
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// The folders the account made.
    pub fn folders(&self) -> &[Folder] {
        &self.folders
    }

    /// How the sidebar shows `folder`: plain for Proton's own.
    pub fn place_look(&self, folder: &Folder) -> PlaceLook {
        folder
            .custom_id()
            .and_then(|id| self.place_looks.get(id))
            .copied()
            .unwrap_or_default()
    }

    pub fn showing_settings(&self) -> bool {
        self.showing_settings
    }

    pub fn settings_section(&self) -> SettingsSection {
        self.settings_section
    }

    /// The notices of third-party software that came with the app, if any.
    pub fn third_party_notices(&self) -> Option<&Path> {
        self.third_party_notices.as_deref()
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

    /// Keeps the window's size and place. A drag reports many sizes; only a
    /// real change is worth a write.
    fn remember_window(&mut self, window: Window) {
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
    ///
    /// The pictures shown are the open conversation's: whatever closes it or
    /// opens another, they go with it.
    pub fn update(&mut self, message: Message) -> Effects {
        let covered = self.draft_covers_reader();
        let open = self.open_conversation();
        let effects = self.dispatch(message);
        if self.open_conversation() != open {
            self.forget_inline_images();
        }
        if covered && !self.draft_covers_reader() {
            return Effects::batch([effects, self.mark_read_once_shown()]);
        }
        effects
    }

    fn dispatch(&mut self, message: Message) -> Effects {
        match message {
            Message::Submit => return self.sign_in(),
            Message::CancelChallenge => return self.cancel_sign_in(),
            Message::SessionChecked(profile, outcome) => {
                return self.finish_session_check(&profile, outcome);
            }
            Message::RetrySavedSession => return self.retry_saved_session(),
            Message::SwitchAccount(profile) => return self.switch_account(profile),
            Message::AddAccount => return self.add_account(),
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
            Message::OpenThirdPartyNotices => {
                if let Some(notices) = &self.third_party_notices {
                    return open_in_default_app(notices.clone());
                }
            }
            Message::CheckForUpdates => return self.check_for_updates(),
            Message::UpdatesChecked(latest) => self.updates_checked(latest),
            Message::OpenReleasePage => {
                return open_in_browser(releases::RELEASE_PAGE.to_owned());
            }
            Message::CopyVerificationLink => {
                if let AuthState::NeedsHumanVerification { url } = &self.auth_state {
                    return Effects::ui(UiEffect::CopyText(url.clone()));
                }
            }
            Message::Logout => return self.logout(),
            Message::LogoutFinished(epoch, result) => return self.finish_logout(epoch, result),
            Message::DuplicateLogoutFinished(epoch, result) => {
                return self.finish_duplicate_logout(epoch, result);
            }
            Message::SelectFolder(folder) => return self.select_folder(folder),
            Message::ShowOnly(filter) => return self.show_only(filter),
            Message::SelectConversation(id) => {
                // A conversation chosen while writing shows, with the draft
                // folded above it.
                self.fold_compose();
                return self.select_conversation(id);
            }
            // Rows chosen while writing show what can be done to them, with
            // the draft folded above.
            Message::ToggleSelected(id) => {
                self.fold_compose();
                return self.change_selection(|mailbox| mailbox.toggle_selected(&id));
            }
            Message::SelectThrough(id) => {
                self.fold_compose();
                return self.change_selection(|mailbox| mailbox.select_through(&id));
            }
            Message::RetryConversation => return self.retry_conversation(),
            Message::SearchChanged(query) => {
                if let Some(mailbox) = self.active_mailbox() {
                    mailbox.set_search_query(query);
                }
            }
            Message::SearchSubmitted => return self.start_search(),
            Message::SearchLoaded(request, result) => return self.finish_search(request, result),
            Message::DeviceSearchLoaded(request, found) => {
                if let Some(mailbox) = self.mailbox.as_mut() {
                    mailbox.finish_device_search(&request, found);
                }
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
                return self.finish_page(request, result);
            }
            Message::ThreadInspected {
                epoch,
                request,
                revision,
                folder,
                conversation_id,
                result,
            } => {
                return self.finish_inspection(
                    epoch,
                    request,
                    revision,
                    folder,
                    conversation_id,
                    result,
                );
            }
            Message::ConversationLoaded(request, result) => {
                return self.finish_conversation(request, result);
            }
            Message::ConversationPreloaded(request, result) => {
                let error = self
                    .mailbox
                    .as_mut()
                    .and_then(|mailbox| mailbox.finish_preload(&request, result));
                self.end_session_if_expired(error);
            }
            Message::FoldersLoaded(epoch, result) => return self.finish_folders(epoch, result),
            Message::CountsLoaded(request, result) => {
                let error = self
                    .mailbox
                    .as_mut()
                    .and_then(|mailbox| mailbox.finish_counts(request, result));
                self.end_session_if_expired(error);
            }
            Message::NewMailLoaded(epoch, request, result) => {
                return self.finish_new_mail(epoch, request, result);
            }
            Message::ActionFinished(requests, result) => {
                return self.finish_mail_action(requests, result);
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
            Message::MailtoRejected => self.mailto_error = Some(MailtoError::Unreadable),
            Message::OpenPendingMailto => return self.open_pending_mailto(),
            Message::DismissPendingMailto => {
                self.pending_mailto.pop_front();
            }
            Message::DismissMailtoError => self.mailto_error = None,
            Message::PanelsResized(panels) => {
                self.remember(|settings| settings.panels = panels);
            }
            Message::FoldSidebar(folded) => {
                self.remember(|settings| settings.sidebar_folded = folded);
            }
            Message::SettingsWritten(revision, written) => {
                self.finish_settings_write(revision, written);
            }
            Message::ResetLayout => {
                self.remember(|settings| {
                    settings.window = Window::default();
                    settings.panels = Panels::default();
                    settings.sidebar_folded = false;
                });
                return Effects::ui(UiEffect::ResetLayout);
            }
            Message::KeyPressed(press) => return self.handle_key(press),
            Message::WindowResized(window) => self.remember_window(window),
            Message::SaveAttachment {
                message_id,
                attachment_id,
            } => {
                return self.save_attachment(message_id, attachment_id, None);
            }
            Message::SaveAttachmentAs {
                message_id,
                attachment_id,
                name,
            } => {
                return self.save_attachment(message_id, attachment_id, Some(name));
            }
            Message::AttachmentDestinationChosen(request, path) => {
                return self.attachment_destination_chosen(request, path);
            }
            Message::LoadInlineImage(message_id, attachment_id) => {
                return self.load_inline_image(message_id, attachment_id);
            }
            Message::InlineImageLoaded(epoch, attachment_id, picture) => {
                self.finish_inline_image(epoch, &attachment_id, picture);
            }
            Message::AttachmentSaved(request, outcome) => {
                self.finish_attachment_save(request, outcome);
            }
            Message::RevealAttachment(path) => return reveal_in_file_manager(path),
            Message::ShowSettings(showing) => self.showing_settings = showing,
            Message::ShowSettingsSection(section) => self.settings_section = section,
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
            Message::SetShowHiddenText(on) => {
                self.remember(|settings| settings.show_hidden_text = on);
            }
            Message::SetSenderTextStyles(on) => {
                self.remember(|settings| settings.sender_text_styles = on);
            }
            Message::SetDataTables(on) => self.remember(|settings| settings.data_tables = on),
            Message::SetShowInlineImages(on) => {
                self.remember(|settings| settings.show_inline_images = on);
                if !on {
                    self.forget_inline_images();
                }
            }
            Message::SetZoom(zoom) => self.remember(|settings| settings.zoom = zoom),
            Message::SetTextScale(scale) => {
                self.remember(|settings| settings.text_scale = scale);
            }
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
            Message::SetNoticeSeconds(seconds) => {
                self.remember(|settings| settings.notice_seconds = seconds);
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
            Message::SetSearchDeviceOffline(on) => {
                self.remember(|settings| settings.search_device_offline = on);
            }
            Message::SetCheckForUpdates(on) => self.set_check_for_updates(on),
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
            Message::LeaveOutForwarded(id) => self.leave_out_forwarded(&id),
            Message::ComposeChanged(field, value) => self.change_compose(field, value),
            Message::ComposeSender(address) => self.choose_compose_sender(address),
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

    /// Whether the refresh on its way is one the app started by itself, which
    /// the list leaves unannounced.
    pub fn refreshing_on_its_own(&self) -> bool {
        self.mailbox.as_ref().is_some_and(|mailbox| {
            matches!(mailbox.status(), ListStatus::Refreshing(id) if Some(id) == self.automatic_refresh)
        })
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
            && self.settings.mark_read_on_open
            && !self.postpone_read_while_covered(&id)
            && !self.mark_demo_read(&service, &id)
        {
            return Effects::none();
        }

        self.start_conversation_load(id)
    }

    fn apply_action(&mut self, action: MailAction) -> Effects {
        let rows = self
            .mailbox
            .as_ref()
            .map(Mailbox::rows_acted_on)
            .unwrap_or_default();
        self.apply_to_rows(rows, action)
    }

    /// Acts on one listed row, open or not. Only the open one, when it leaves
    /// the view, gives its place in the reader to the row after it.
    fn apply_row_action(&mut self, row_id: String, action: MailAction) -> Effects {
        self.apply_to_rows(vec![row_id], action)
    }

    fn apply_to_rows(&mut self, rows: Vec<String>, action: MailAction) -> Effects {
        if !self.auth_state.is_authenticated() || rows.is_empty() {
            return Effects::none();
        }
        match self.backend.clone() {
            Some(MailBackend::Demo(service)) => self.apply_demo_action(&service, &rows, action),
            Some(MailBackend::Proton(service)) => self.start_proton_action(service, &rows, action),
            None => Effects::none(),
        }
    }

    /// Opens the row a change of the rows chosen left alone, if any.
    fn change_selection(&mut self, change: impl FnOnce(&mut Mailbox) -> Option<String>) -> Effects {
        match self.active_mailbox().and_then(change) {
            Some(alone) => self.select_conversation(alone),
            None => Effects::none(),
        }
    }

    /// Demo actions change local data at once and reload the folder snapshot.
    fn apply_demo_action(
        &mut self,
        service: &DemoMailbox,
        rows: &[String],
        action: MailAction,
    ) -> Effects {
        let Some(mailbox) = self.mailbox.as_ref() else {
            return Effects::none();
        };
        if !mailbox.takes_actions() {
            return Effects::none();
        }
        let listed: Vec<(&str, SummaryKind)> = rows
            .iter()
            .filter_map(|id| Some((id.as_str(), mailbox.row(id)?.kind)))
            .collect();

        let applied: Vec<(&str, SummaryKind)> = listed
            .into_iter()
            .filter(|&(id, _)| match &action {
                MailAction::MoveTo(folder) => service.move_to(id, folder),
                MailAction::SetUnread(unread) => service.set_unread(id, *unread),
                MailAction::SetStarred(starred) => service.set_starred(id, *starred),
                MailAction::SetLabel { label, on } => service.set_label(id, label, *on),
            })
            .collect();
        if applied.is_empty() {
            return Effects::none();
        }
        // Demo actions never reach `finish_action`, so the offer to take the
        // move back, and the change itself, are recorded here instead.
        if let Some(mailbox) = self.active_mailbox() {
            for (id, _) in &applied {
                mailbox.record_action(id, &action);
            }
            mailbox.offer_undo(&applied, &action);
        }
        if let Some(next) = self.apply_demo_snapshot(service) {
            return self.select_conversation(next);
        }

        Effects::none()
    }

    /// Puts the rows of the last move back. They have left the list, so the
    /// action names them directly instead of going through the selection.
    fn undo_move(&mut self) -> Effects {
        let Some(undo) = self.active_mailbox().and_then(Mailbox::take_undo) else {
            return Effects::none();
        };
        match self.backend.clone() {
            Some(MailBackend::Demo(service)) => {
                let mut moved = false;
                for row in &undo.rows {
                    moved |= service.move_to(&row.row_id, &row.from);
                }
                if moved {
                    self.apply_demo_snapshot(&service);
                }

                Effects::none()
            }
            Some(MailBackend::Proton(service)) => {
                let starts: Vec<ActionStart> = undo
                    .rows
                    .into_iter()
                    .filter_map(|row| {
                        let request = self.next_request();
                        self.active_mailbox()?.start_move_back(row, request)
                    })
                    .collect();
                Effects::batch(
                    starts
                        .into_iter()
                        .map(|start| self.show_started_action(&service, start))
                        .collect::<Vec<_>>(),
                )
            }
            None => Effects::none(),
        }
    }

    /// Proton actions show at once and run asynchronously, one after
    /// another; a refused one is taken back when Proton answers.
    fn start_proton_action(
        &mut self,
        service: Arc<ProtonMailService>,
        rows: &[String],
        action: MailAction,
    ) -> Effects {
        let requests: Vec<RequestId> = rows.iter().map(|_| self.next_request()).collect();
        let starts = self
            .active_mailbox()
            .map(|mailbox| mailbox.start_actions_on(rows, action, &requests))
            .unwrap_or_default();
        Effects::batch(
            starts
                .into_iter()
                .map(|start| self.show_started_action(&service, start))
                .collect::<Vec<_>>(),
        )
    }

    /// Sends what is ready, and opens the row that took the place of one the
    /// action took out of the view.
    fn show_started_action(
        &mut self,
        service: &Arc<ProtonMailService>,
        start: ActionStart,
    ) -> Effects {
        let send = start.send.map_or_else(Effects::none, |request| {
            let group = self.request_group(&request);
            run_action(Arc::clone(service), group, Message::ActionFinished)
        });
        let open = start
            .opened
            .map_or_else(Effects::none, |id| self.open_row(id));
        Effects::batch([send, open])
    }

    /// Sends the next action in line, once the one before it has answered,
    /// with those that go in the same request.
    fn send_action(&self, request: Option<ActionRequest>) -> Effects {
        match (request, self.backend.clone()) {
            (Some(request), Some(MailBackend::Proton(service))) => run_action(
                service,
                self.request_group(&request),
                Message::ActionFinished,
            ),
            _ => Effects::none(),
        }
    }

    /// The actions that go to Proton in one request with `head`.
    fn request_group(&self, head: &ActionRequest) -> Vec<ActionRequest> {
        self.mailbox
            .as_ref()
            .map_or_else(|| vec![head.clone()], |mailbox| mailbox.request_group(head))
    }

    /// Settles each action of a request Proton answered, in order, then does
    /// once what they need: the next request, the counts, or a reload.
    fn finish_mail_action(
        &mut self,
        requests: Vec<ActionRequest>,
        result: Result<(), MailboxError>,
    ) -> Effects {
        let mut next = None;
        let mut failed = None;
        // Proton is where a row really is: one a refused move took out of the
        // view comes back with the list.
        let mut restore = false;
        // Undo can return a row to the visible folder only a reload lists.
        let mut arrived = false;
        let mut opened = None;
        for request in &requests {
            let Some(mailbox) = self.mailbox.as_mut() else {
                return Effects::none();
            };
            let finish = mailbox.finish_action(request, result);
            next = finish.next_action.or(next);
            match finish.result {
                Err(error) => {
                    failed = Some(error);
                    restore |= mailbox.took_out(request);
                }
                Ok(()) => {
                    arrived |= mailbox.brought_back(request);
                    opened = finish.opened.or(opened);
                }
            }
        }
        if let Some(error) = failed {
            // An expired session closes the mailbox, and nothing is sent.
            if self.end_session_if_expired(Some(error)) {
                return Effects::none();
            }
            let reload = if restore {
                self.refresh_mailbox()
            } else {
                Effects::none()
            };
            return Effects::batch([self.send_action(next), reload]);
        }
        let send_next = self.send_action(next);
        if arrived {
            return Effects::batch([send_next, self.refresh_mailbox()]);
        }

        let counts = self.reload_counts();
        let open_next = opened.map_or_else(Effects::none, |id| self.open_row(id));

        Effects::batch([send_next, counts, open_next])
    }

    fn selected_conversation_id(&self) -> Option<String> {
        self.mailbox
            .as_ref()
            .and_then(Mailbox::selected_conversation)
            .map(str::to_owned)
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
            None => Effects::batch([self.mark_opened_read(), self.preload_successor()]),
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
        let (page, counts) = service.snapshot(
            &folder,
            mailbox.filter(),
            |id| mailbox.lists(id),
            page_size,
            crate::mail::demo::now(),
        );

        self.active_mailbox()?.apply_action_snapshot(page, counts)
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

    /// Shows a conversation that arrived, and then, if the reader waited for
    /// it, marks it read as opening it does and preloads the one after it.
    fn finish_conversation(
        &mut self,
        request: ReaderRequest,
        result: Result<ConversationDetail, MailboxError>,
    ) -> Effects {
        // Only the load the reader waits for opens anything. A late answer for
        // a conversation left behind must not mark the one on screen read: it
        // may have been marked unread meanwhile.
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
            return Effects::batch([self.mark_opened_read(), self.preload_successor()]);
        }
        Effects::none()
    }

    /// Loads into the reader's cache, in the background, the conversation
    /// that would open next were the open one archived or deleted, so it
    /// shows at once then. Only opening it marks it read.
    fn preload_successor(&mut self) -> Effects {
        let Some(backend) = self.backend.clone() else {
            return Effects::none();
        };
        let request = self.next_request();
        let Some(request) = self
            .active_mailbox()
            .and_then(|mailbox| mailbox.start_preload(request))
        else {
            return Effects::none();
        };
        if let Some(before) = self.preload.take() {
            before.abort();
        }
        let (handle, registration) = AbortHandle::new_pair();
        self.preload = Some(handle);
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
            move |result| {
                Message::ConversationPreloaded(
                    request.clone(),
                    result.unwrap_or(Err(MailboxError::Unavailable)),
                )
            },
        )
    }

    /// Stops the conversation loads in flight, the reader's and the one
    /// into its cache, if any: nobody will read them.
    fn abandon_reader_load(&mut self) {
        if let Some(load) = self.reader_load.take() {
            load.abort();
        }
        if let Some(preload) = self.preload.take() {
            preload.abort();
            if let Some(mailbox) = self.mailbox.as_mut() {
                mailbox.forget_preload();
            }
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

/// Runs one mailbox action on Proton and reports the outcome under
/// `finished`, which is what tells an automatic read from a deliberate one.
/// Sends `requests`, the same action on rows of one kind read in one folder,
/// in one request.
fn run_action(
    service: Arc<ProtonMailService>,
    requests: Vec<ActionRequest>,
    finished: fn(Vec<ActionRequest>, Result<(), MailboxError>) -> Message,
) -> Effects {
    let Some(first) = requests.first().cloned() else {
        return Effects::none();
    };
    let ids: Vec<String> = requests
        .iter()
        .map(|request| request.row_id.clone())
        .collect();

    Effects::perform(
        async move {
            service
                .apply_action(first.kind, &ids, first.context.as_ref(), first.action)
                .await
        },
        move |result| finished(requests.clone(), result),
    )
}

fn open_in_browser(url: String) -> Effects {
    Effects::background(async move {
        let _ = open_with_system(&url).await;
    })
}

fn open_in_default_app(file: PathBuf) -> Effects {
    Effects::background(async move {
        let _ = open_with_system(&file).await;
    })
}

/// Opens a web address in the default browser, or a file in the default app
/// for its type, without going through a shell: the system's opener does
/// both.
async fn open_with_system(target: impl AsRef<std::ffi::OsStr>) -> std::io::Result<()> {
    let mut opener = ruston_core::browser::opener()?;
    opener.arg(target);
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
