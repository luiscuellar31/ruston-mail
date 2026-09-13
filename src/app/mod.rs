mod auth;
mod effect;
mod keys;
mod layout;
mod mailbox;
mod reader;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::downloads::{self, SaveError};
use crate::settings::{Panels, Settings, StartFolder, Window};

/// How far the window must change before its new size is worth storing.
const RESIZE_STEP: f32 = 8.0;

use crate::mail::{
    AuthError, ConversationDetail, ConversationPage, Folder, MailAction, MailBackend,
    MailboxCounts, MailboxError, ProtonMailService, ResumeOutcome, SignInOutcome, SignInPrompt,
    demo::DemoMailbox,
};

use auth::LoginForm;
pub use auth::{AuthState, SignInStep};
pub use effect::{Effect, Effects, UiEffect};
pub use keys::{Key, KeyPress};
pub use layout::{ratios as panel_ratios, widths as panel_widths};
pub use mailbox::{ActionRequest, ListStatus, Mailbox, ReaderRequest, SearchRequest, Step};
use mailbox::{PageRequest, RequestId};
pub use reader::{ConversationReader, ReaderState};

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
    SignInPrompt(SignInPrompt),
    OpenVerificationPage,
    CopyVerificationLink,
    LinkClicked(String),
    OpenLink,
    CopyLink,
    DismissLink,
    Logout,
    LogoutFinished(Result<(), AuthError>),
    SelectFolder(Folder),
    SelectConversation(String),
    RetryConversation,
    SearchChanged(String),
    /// Asks the server for everything matching what was typed, across folders.
    SearchSubmitted,
    SearchLoaded(SearchRequest, Result<ConversationPage, MailboxError>),
    ToggleMessageExpanded(String),
    /// Folds or unfolds one quoted passage of a message.
    ToggleQuoteExpanded(String, usize),
    /// Shows or hides the labels the open conversation does not carry.
    ToggleLabelsShown,
    /// Applies one action to the selected row. Adding an action is a line in
    /// the toolbar rather than a message and an arm of its own.
    ApplyAction(MailAction),
    /// Puts the last moved conversation back where it came from.
    UndoMove,
    RefreshMailbox,
    LoadMoreConversations,
    ConversationsLoaded(RequestId, Result<ConversationPage, MailboxError>),
    ConversationLoaded(ReaderRequest, Result<ConversationDetail, MailboxError>),
    CountsLoaded(RequestId, Result<MailboxCounts, MailboxError>),
    /// The folders the account made, fetched once when the mailbox opens.
    FoldersLoaded(Result<Vec<Folder>, MailboxError>),
    ActionFinished(ActionRequest, Result<(), MailboxError>),
    MarkReadFinished(ActionRequest, Result<(), MailboxError>),
    PanelsResized(Panels),
    /// A key the focused widget did not take.
    KeyPressed(KeyPress),
    /// The window changed size, which is worth remembering for next time.
    WindowResized(Window),
    /// Fetches one attachment and saves it, by message and attachment id.
    SaveAttachment(String, String),
    /// Where an attachment landed, or why it did not.
    AttachmentSaved(Result<PathBuf, SaveError>),
    /// Opens or closes the settings page.
    ShowSettings(bool),
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
    /// Picks the folder a run opens in.
    SetStartFolder(StartFolder),
}

pub struct App {
    auth_state: AuthState,
    login_form: LoginForm,
    auth_error: Option<AuthError>,
    /// The question a running sign-in is waiting on, if any.
    pending_prompt: Option<SignInPrompt>,
    /// A link clicked in a message, waiting for the user to confirm it.
    pending_link: Option<PendingLink>,
    backend: Option<MailBackend>,
    mailbox: Option<Mailbox>,
    last_request: RequestId,
    /// The folders the account made, which the sidebar shows below Proton's
    /// own. Empty until they arrive, and in demo mode for good.
    folders: Vec<Folder>,
    /// What the app remembers between runs.
    settings: Settings,
    /// Counts changes to the settings, and how many of them have reached the
    /// file. Dragging a divider or a window edge changes them many times a
    /// second, and each write is a serialize and a synchronous file write on
    /// the thread that is drawing, so the writing is left to whoever owns the
    /// clock: it asks through [`Self::settings_revision`] and calls
    /// [`Self::save_settings`] once the dragging has stopped.
    settings_revision: u64,
    settings_written: u64,
    /// Whether the settings page is covering the mailbox.
    showing_settings: bool,
    /// What became of the last attachment someone asked to save.
    saved_attachment: Option<Result<PathBuf, SaveError>>,
    /// The attachment being fetched, if any. Saving one is a deliberate act
    /// and a slow one, so a second press must not fetch and write the same
    /// file a second time under a name of its own.
    saving_attachment: Option<String>,
}

impl App {
    /// In demo mode the app opens a local fictional mailbox and never starts
    /// the Proton session resume.
    pub fn boot(demo: bool, settings: Settings) -> (Self, Effects) {
        let mut app = Self::new(settings);
        let task = if demo {
            app.open_mailbox(MailBackend::demo(), None)
        } else {
            Effects::perform(ProtonMailService::resume(), Message::SessionChecked)
        };

        (app, task)
    }

    fn new(settings: Settings) -> Self {
        Self {
            auth_state: AuthState::CheckingSession,
            login_form: LoginForm::default(),
            auth_error: None,
            pending_prompt: None,
            pending_link: None,
            backend: None,
            mailbox: None,
            last_request: 0,
            folders: Vec::new(),
            settings,
            settings_revision: 0,
            settings_written: 0,
            showing_settings: false,
            saved_attachment: None,
            saving_attachment: None,
        }
    }

    /// Where the last saved attachment landed, or why it did not.
    /// The attachment now being fetched, so its row can say so.
    pub fn saving_attachment(&self) -> Option<&str> {
        self.saving_attachment.as_deref()
    }

    pub fn saved_attachment(&self) -> Option<Result<&Path, SaveError>> {
        self.saved_attachment
            .as_ref()
            .map(|outcome| outcome.as_deref().map_err(|error| *error))
    }

    /// Fetches one attachment and writes it to the downloads folder. The
    /// download is the slow half, so the reader says what happened afterwards
    /// rather than blocking on it.
    fn save_attachment(&mut self, message_id: String, attachment_id: String) -> Effects {
        let Some(backend) = self.backend.clone() else {
            return Effects::none();
        };
        if self.saving_attachment.is_some() {
            return Effects::none();
        }
        self.saved_attachment = None;
        self.saving_attachment = Some(attachment_id.clone());

        Effects::perform(
            async move {
                match backend
                    .download_attachment(&message_id, &attachment_id)
                    .await
                {
                    // Writing happens off the interface thread, where a large
                    // file cannot stall a redraw.
                    Ok((name, contents)) => downloads::save(&name, &contents),
                    Err(_) => Err(SaveError::NotFetched),
                }
            },
            Message::AttachmentSaved,
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

    /// Writes the settings if they have changed since the last write. Saving
    /// is best effort, so a setting that cannot be stored still applies for
    /// this run.
    pub fn save_settings(&mut self) {
        if !self.settings_unsaved() {
            return;
        }
        self.settings.save();
        self.settings_written = self.settings_revision;
    }

    pub fn update(&mut self, message: Message) -> Effects {
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
                    if let Some(prompt) = self.pending_prompt.take() {
                        prompt.cancel();
                    }
                    self.login_form.clear_sensitive();
                    self.auth_error = None;
                    self.auth_state = AuthState::SignedOut;
                }
            }
            Message::SessionChecked(outcome) => return self.finish_session_check(outcome),
            Message::SignInFinished(outcome) => return self.finish_sign_in(outcome),
            Message::SignInPrompt(prompt) => return self.show_prompt(prompt),
            Message::OpenVerificationPage => {
                if let AuthState::NeedsHumanVerification { url } = &self.auth_state {
                    return open_in_browser(url.clone());
                }
            }
            Message::CopyVerificationLink => {
                if let AuthState::NeedsHumanVerification { url } = &self.auth_state {
                    return Effects::ui(UiEffect::CopyText(url.clone()));
                }
            }
            Message::Logout => return self.logout(),
            Message::LogoutFinished(result) => self.finish_logout(result),
            Message::SelectFolder(folder) => return self.select_folder(folder),
            Message::SelectConversation(id) => return self.select_conversation(id),
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
                self.handle_mailbox_error(error);
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
            Message::ToggleLabelsShown => {
                if let Some(mailbox) = self.active_mailbox() {
                    mailbox.toggle_labels();
                }
            }
            Message::ApplyAction(action) => return self.apply_action(action),
            Message::UndoMove => return self.undo_move(),
            Message::RefreshMailbox => return self.refresh_mailbox(),
            Message::LoadMoreConversations => return self.load_more_conversations(),
            Message::ConversationsLoaded(request, result) => {
                let error = self
                    .mailbox
                    .as_mut()
                    .and_then(|mailbox| mailbox.finish_page(request, result));
                self.handle_mailbox_error(error);
            }
            Message::ConversationLoaded(request, result) => {
                let error = self
                    .mailbox
                    .as_mut()
                    .and_then(|mailbox| mailbox.finish_conversation(&request, result));
                self.handle_mailbox_error(error);
                return self.mark_opened_read();
            }
            Message::FoldersLoaded(result) => {
                match result {
                    Ok(folders) => self.folders = folders,
                    // Without them the sidebar simply shows Proton's own.
                    Err(error) => self.handle_mailbox_error(Some(error)),
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
                self.handle_mailbox_error(error);
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
                    return open_in_browser(link.url);
                }
                self.pending_link = Some(link);
            }
            Message::OpenLink => {
                if let Some(link) = self.pending_link.take() {
                    return open_in_browser(link.url);
                }
            }
            Message::CopyLink => {
                if let Some(link) = self.pending_link.take() {
                    return Effects::ui(UiEffect::CopyText(link.url));
                }
            }
            Message::DismissLink => self.pending_link = None,
            Message::PanelsResized(panels) => {
                self.remember(|settings| settings.panels = panels);
            }
            Message::KeyPressed(press) => return self.handle_key(press),
            // A drag reports many sizes; only a real change is worth a write.
            Message::WindowResized(size) => {
                let moved = (size.width - self.settings.window.width).abs() >= RESIZE_STEP
                    || (size.height - self.settings.window.height).abs() >= RESIZE_STEP;
                if moved {
                    self.remember(|settings| {
                        settings.window = Window {
                            width: size.width,
                            height: size.height,
                        };
                    });
                }
            }
            Message::SaveAttachment(message_id, attachment_id) => {
                return self.save_attachment(message_id, attachment_id);
            }
            Message::AttachmentSaved(outcome) => {
                self.saving_attachment = None;
                // A save that landed after the session ended has nobody left
                // to tell, and its news must not surface in the next one.
                if self.mailbox.is_some() {
                    self.saved_attachment = Some(outcome);
                }
            }
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
            Message::SetStartFolder(start) => self.remember(|settings| settings.start = start),
        }

        Effects::none()
    }

    pub fn mailbox(&self) -> Option<&Mailbox> {
        self.mailbox.as_ref()
    }

    pub fn is_demo(&self) -> bool {
        matches!(self.backend, Some(MailBackend::Demo(_)))
    }

    pub fn mailbox_actions_available(&self) -> bool {
        self.backend.is_some()
    }

    pub fn panels(&self) -> Panels {
        self.settings.panels
    }

    pub fn pending_link(&self) -> Option<&PendingLink> {
        self.pending_link.as_ref()
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
        if !matches!(self.auth_state, AuthState::Authenticated { .. }) {
            return Effects::none();
        }
        self.pending_link = None;
        if let Some(MailBackend::Demo(service)) = self.backend.clone() {
            if !service.set_unread(&id, false) {
                return Effects::none();
            }
            self.apply_demo_snapshot(&service);
        }

        self.start_conversation_load(id)
    }

    fn apply_action(&mut self, action: MailAction) -> Effects {
        if !matches!(self.auth_state, AuthState::Authenticated { .. }) {
            return Effects::none();
        }
        match self.backend.clone() {
            Some(MailBackend::Demo(service)) => self.apply_demo_action(&service, action),
            Some(MailBackend::Proton(service)) => self.start_proton_action(service, action),
            None => Effects::none(),
        }
    }

    /// Demo actions change local data at once and reload the folder snapshot.
    fn apply_demo_action(&mut self, service: &DemoMailbox, action: MailAction) -> Effects {
        let Some(mailbox) = self.mailbox.as_ref() else {
            return Effects::none();
        };
        if mailbox.is_busy() {
            return Effects::none();
        }
        let Some((id, kind)) = mailbox
            .selected_summary()
            .map(|summary| (summary.id.clone(), summary.kind))
        else {
            return Effects::none();
        };

        let applied = match &action {
            MailAction::MoveTo(folder) => service.move_to(&id, folder),
            MailAction::SetUnread(unread) => service.set_unread(&id, *unread),
            MailAction::SetStarred(starred) => service.set_starred(&id, *starred),
            // The demo account owns no labels, so the reader offers none.
            MailAction::SetLabel { .. } => false,
        };
        if applied {
            // Demo actions never reach `finish_action`, so the offer to take
            // the move back, and the change itself, are recorded here instead.
            if let Some(mailbox) = self.active_mailbox() {
                mailbox.record_action(&id, &action);
                mailbox.offer_undo(&id, kind, action);
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
        let action = MailAction::MoveTo(undo.from.clone());

        match self.backend.clone() {
            Some(MailBackend::Demo(service)) => {
                if service.move_to(&undo.row_id, &undo.from) {
                    self.apply_demo_snapshot(&service);
                }

                Effects::none()
            }
            Some(MailBackend::Proton(service)) => {
                let request = self.next_request();
                let Some(request) = self.active_mailbox().and_then(|mailbox| {
                    mailbox.start_action_on(undo.row_id, undo.kind, action, request)
                }) else {
                    return Effects::none();
                };
                run_action(service, request, Message::ActionFinished)
            }
            None => Effects::none(),
        }
    }

    /// Proton actions run asynchronously; the list changes only once Proton
    /// confirms them.
    fn start_proton_action(
        &mut self,
        service: Arc<ProtonMailService>,
        action: MailAction,
    ) -> Effects {
        let request = self.next_request();
        let Some(request) = self
            .active_mailbox()
            .and_then(|mailbox| mailbox.start_action(action, request))
        else {
            return Effects::none();
        };
        run_action(service, request, Message::ActionFinished)
    }

    fn finish_mail_action(
        &mut self,
        request: ActionRequest,
        result: Result<(), MailboxError>,
    ) -> Effects {
        let Some(outcome) = self
            .mailbox
            .as_mut()
            .map(|mailbox| mailbox.finish_action(&request, result))
        else {
            return Effects::none();
        };
        let next = match outcome {
            Ok(next) => next,
            Err(error) => {
                self.handle_mailbox_error(Some(error));
                return Effects::none();
            }
        };

        // A row put back into the folder on screen is not in the loaded list,
        // and only a reload brings it into view. A row that is already listed
        // never left, so acting on it changes nothing to reload.
        let arrived = self.mailbox.as_ref().is_some_and(|mailbox| {
            request.action.destination() == Some(mailbox.folder())
                && !mailbox.has_row(&request.row_id)
        });
        if arrived {
            return self.refresh_mailbox();
        }

        let counts = self.reload_counts();
        let open_next = next.map_or_else(Effects::none, |id| {
            Effects::batch([self.reveal_in_list(&id), self.select_conversation(id)])
        });

        Effects::batch([counts, open_next])
    }

    /// Once an unread Proton row's content is shown, marks it read on Proton.
    /// Demo rows are already marked read when they are selected.
    fn mark_opened_read(&mut self) -> Effects {
        if !self.settings.mark_read_on_open {
            return Effects::none();
        }
        let Some(MailBackend::Proton(service)) = self.backend.clone() else {
            return Effects::none();
        };
        let request = self.next_request();
        let Some(request) = self
            .active_mailbox()
            .and_then(|mailbox| mailbox.start_mark_read(request))
        else {
            return Effects::none();
        };
        run_action(service, request, Message::MarkReadFinished)
    }

    fn finish_mark_read(
        &mut self,
        request: ActionRequest,
        result: Result<(), MailboxError>,
    ) -> Effects {
        match self
            .mailbox
            .as_mut()
            .map(|mailbox| mailbox.finish_mark_read(&request, result))
        {
            Some(Ok(true)) => self.reload_counts(),
            // Only an expired session matters; the row simply stays unread.
            Some(Err(error)) => {
                self.handle_mailbox_error(Some(error));
                Effects::none()
            }
            _ => Effects::none(),
        }
    }

    /// Proton owns the folder counts; reload them after a change.
    fn reload_counts(&mut self) -> Effects {
        let counts_request = self.next_request();
        let counts = self
            .active_mailbox()
            .and_then(|mailbox| mailbox.refresh_counts(counts_request));

        self.fetch_counts(counts)
    }

    fn start_conversation_load(&mut self, id: String) -> Effects {
        let request = self.next_request();
        let request = self
            .active_mailbox()
            .and_then(|mailbox| mailbox.start_conversation_load(id, request));
        if request.is_none() {
            return Effects::none();
        }

        // A new conversation starts at its top, not where the last one was
        // left; the reader pane keeps its scroll offset otherwise.
        Effects::batch([
            Effects::ui(UiEffect::ScrollReaderTop),
            self.fetch_conversation(request),
        ])
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

    /// Brings the open folder back in line with what the account has. It was
    /// opened from the settings file, which may name a folder or label that
    /// has since been renamed or removed, and a name nothing answers to would
    /// sit in the header with no matching button beside it.
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

        // The same place under a new name holds the same mail, so taking the
        // name up is all there is to do: nothing is reloaded and whatever is
        // being read stays open.
        if let Some(mailbox) = self.active_mailbox() {
            mailbox.rename_folder(current.clone());
        }
        self.remember_folder(&current);

        Effects::none()
    }

    /// Keeps the folder to open on the next run. Demo mail is fictional, so
    /// where it is read is not worth remembering, let alone worth overwriting
    /// the real answer with.
    fn remember_folder(&mut self, folder: &Folder) {
        if !self.is_demo() {
            self.remember(|settings| settings.folder = folder.clone());
        }
    }

    fn select_folder(&mut self, folder: Folder) -> Effects {
        self.pending_link = None;
        self.saved_attachment = None;
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

        self.fetch_search(request)
    }

    fn fetch_search(&self, request: SearchRequest) -> Effects {
        let Some(backend) = self.backend.clone() else {
            return Effects::none();
        };
        let query = request.query.clone();
        let limit = backend.page_size();

        Effects::perform(
            async move { backend.search(&query, limit).await },
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

        Effects::batch([self.fetch_page(page), self.fetch_counts(counts)])
    }

    fn load_more_conversations(&mut self) -> Effects {
        let request = self.next_request();
        let page = self
            .active_mailbox()
            .and_then(|mailbox| mailbox.load_more(request));

        self.fetch_page(page)
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

    /// Asks once for the folders the account made. They change rarely, so
    /// nothing re-fetches them until the mailbox is opened again.
    fn fetch_folders(&self) -> Effects {
        let Some(backend) = self.backend.clone() else {
            return Effects::none();
        };

        Effects::perform(
            async move { backend.list_folders().await },
            Message::FoldersLoaded,
        )
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

    fn fetch_conversation(&self, request: Option<ReaderRequest>) -> Effects {
        let (Some(request), Some(backend)) = (request, self.backend.clone()) else {
            return Effects::none();
        };
        let (kind, id) = (request.kind, request.conversation_id.clone());

        Effects::perform(
            async move { backend.conversation_detail(kind, &id).await },
            move |result| Message::ConversationLoaded(request.clone(), result),
        )
    }

    fn handle_mailbox_error(&mut self, error: Option<MailboxError>) {
        if error == Some(MailboxError::SessionExpired) {
            self.close_mailbox(Some(AuthError::SessionExpired));
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
            "mailto" => url.path().to_owned(),
            _ => return None,
        };

        Some(Self {
            url: url.into(),
            target,
        })
    }
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
        let _ = open_url(&url);
    })
}

/// Opens `url` in the default browser without going through a shell.
fn open_url(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = std::process::Command::new("rundll32");
        command.arg("url.dll,FileProtocolHandler");
        command
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut command = std::process::Command::new("xdg-open");

    command.arg(url).spawn().map(drop)
}

fn optional_trimmed(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

/// Same rule as `mail::demo::non_empty`, kept under the same name on purpose:
/// a mailbox password is taken exactly as typed, spaces included.
fn non_empty(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_owned())
}

#[cfg(test)]
mod tests;
