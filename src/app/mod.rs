mod layout;
mod mailbox;
mod reader;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use iced::keyboard::{self, Key, Modifiers};
use iced::widget::operation::{self, RelativeOffset};
use iced::widget::{pane_grid, text_editor};
use iced::{Size, Subscription, Task, window};

use crate::downloads::{self, SaveError};
use crate::settings::{self, Settings};

/// How far the window must change before its new size is worth storing.
const RESIZE_STEP: f32 = 8.0;

use crate::mail::{
    AuthError, ConversationDetail, ConversationPage, Folder, LoginRequest, MailAction, MailBackend,
    MailboxCounts, MailboxError, ProtonMailService, ResumeOutcome, SignInEvent, SignInOutcome,
    SignInPrompt, demo::DemoMailbox,
};

pub use layout::{
    CONVERSATION_LIST, DIVIDER_GRAB, DIVIDER_WIDTH, MIN_PANEL_WIDTH, Panel, READER_BODY,
    SEARCH_INPUT,
};
pub use mailbox::{
    ActionRequest, ListStatus, Mailbox, ReaderRequest, SearchRequest, Step, UndoMove,
};
use mailbox::{PageRequest, RequestId};
pub use reader::{ConversationReader, ReaderState};

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
    /// Shows one message as selectable text, or goes back to the formatted body.
    ToggleTextSelection(String),
    /// A click, drag or keyboard interaction inside one selectable body.
    SelectText(String, text_editor::Action),
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
    PanelResized(pane_grid::ResizeEvent),
    /// A key the focused widget did not take.
    Keyboard(keyboard::Event),
    /// The window changed size, which is worth remembering for next time.
    WindowResized(Size),
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
}

/// What a key press means to the mailbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shortcut {
    /// Open the conversation one step away in the list.
    Move(Step),
    /// Open the selected conversation, which also retries a failed one.
    Open,
    /// Back out of the topmost thing on screen.
    Dismiss,
    Refresh,
    Search,
}

/// The meaning of a key press, or `None` when it is not a shortcut. Plain
/// keys carry no modifiers, so typing never moves the mailbox underneath.
fn shortcut(key: Key<&str>, modifiers: Modifiers) -> Option<Shortcut> {
    if modifiers.command() {
        return match key {
            Key::Character("r") => Some(Shortcut::Refresh),
            Key::Character("f") => Some(Shortcut::Search),
            _ => None,
        };
    }
    if !modifiers.is_empty() {
        return None;
    }

    match key {
        Key::Named(keyboard::key::Named::ArrowDown) | Key::Character("j") => {
            Some(Shortcut::Move(Step::Next))
        }
        Key::Named(keyboard::key::Named::ArrowUp) | Key::Character("k") => {
            Some(Shortcut::Move(Step::Previous))
        }
        Key::Named(keyboard::key::Named::Enter) => Some(Shortcut::Open),
        Key::Named(keyboard::key::Named::Escape) => Some(Shortcut::Dismiss),
        _ => None,
    }
}

#[derive(Default)]
struct LoginForm {
    username: String,
    password: String,
    totp: String,
    mailbox_password: String,
}

impl LoginForm {
    fn clear_sensitive(&mut self) {
        self.password.clear();
        self.totp.clear();
        self.mailbox_password.clear();
    }

    fn clear_all(&mut self) {
        self.username.clear();
        self.clear_sensitive();
    }
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
    /// Mailbox panel sizes. Pure layout state: kept for the life of the app
    /// and never touched by mailbox or authentication changes.
    panels: pane_grid::State<Panel>,
    /// The folders the account made, which the sidebar shows below Proton's
    /// own. Empty until they arrive, and in demo mode for good.
    folders: Vec<Folder>,
    /// What the app remembers between runs.
    settings: Settings,
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
    pub fn boot(demo: bool, settings: Settings) -> (Self, Task<Message>) {
        let mut app = Self::new(settings);
        let task = if demo {
            app.open_mailbox(MailBackend::demo(), None)
        } else {
            Task::perform(ProtonMailService::resume(), Message::SessionChecked)
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
            panels: layout::panels(settings.panels),
            folders: Vec::new(),
            settings,
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
    fn save_attachment(&mut self, message_id: String, attachment_id: String) -> Task<Message> {
        let Some(backend) = self.backend.clone() else {
            return Task::none();
        };
        if self.saving_attachment.is_some() {
            return Task::none();
        }
        self.saved_attachment = None;
        self.saving_attachment = Some(attachment_id.clone());

        Task::perform(
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

    /// Keeps a changed setting, and writes it out. Saving is best effort, so
    /// a setting that cannot be stored still applies for this run.
    fn remember(&mut self, change: impl FnOnce(&mut Settings)) {
        let before = self.settings.clone();
        change(&mut self.settings);
        if self.settings != before {
            self.settings.save();
        }
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
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
                    return iced::clipboard::write(url.clone());
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
            Message::ToggleTextSelection(id) => {
                if let Some(mailbox) = self.active_mailbox() {
                    mailbox.toggle_selection(&id);
                }
            }
            Message::SelectText(id, action) => {
                if let Some(mailbox) = self.active_mailbox() {
                    mailbox.select_text(&id, action);
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
                return Task::batch([self.reconcile_open_folder(), self.reload_counts()]);
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
                    return Task::none();
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
                    return iced::clipboard::write(link.url);
                }
            }
            Message::DismissLink => self.pending_link = None,
            Message::PanelResized(event) => {
                self.panels.resize(event.split, event.ratio);
                let panels = layout::ratios(&self.panels);
                self.remember(|settings| settings.panels = panels);
            }
            Message::Keyboard(event) => return self.handle_key(event),
            // A drag reports many sizes; only a real change is worth a write.
            Message::WindowResized(size) => {
                let moved = (size.width - self.settings.window.width).abs() >= RESIZE_STEP
                    || (size.height - self.settings.window.height).abs() >= RESIZE_STEP;
                if moved {
                    self.remember(|settings| {
                        settings.window = settings::Window {
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
                self.saved_attachment = Some(outcome);
            }
            Message::ShowSettings(showing) => self.showing_settings = showing,
            Message::SetMarkReadOnOpen(on) => {
                self.remember(|settings| settings.mark_read_on_open = on);
            }
            Message::SetConfirmLinks(on) => self.remember(|settings| settings.confirm_links = on),
        }

        Task::none()
    }

    /// Keyboard shortcuts, and only once a mailbox is open. `listen` reports
    /// just the keys no focused widget took, so typing in the search field
    /// never reaches the list.
    pub fn subscription(&self) -> Subscription<Message> {
        // The window is worth following whatever the app is showing.
        let resized = window::resize_events().map(|(_, size)| Message::WindowResized(size));

        match self.auth_state {
            AuthState::Authenticated { .. } => {
                Subscription::batch([keyboard::listen().map(Message::Keyboard), resized])
            }
            _ => resized,
        }
    }

    fn handle_key(&mut self, event: keyboard::Event) -> Task<Message> {
        let keyboard::Event::KeyPressed { key, modifiers, .. } = event else {
            return Task::none();
        };
        let Some(shortcut) = shortcut(key.as_ref(), modifiers) else {
            return Task::none();
        };
        // Only the topmost thing on screen takes a key. The mailbox sits
        // under the settings page and under the link prompt, so while either
        // is up the one shortcut still worth anything backs out of it.
        let covered = self.showing_settings || self.pending_link.is_some();
        if covered && shortcut != Shortcut::Dismiss {
            return Task::none();
        }

        match shortcut {
            Shortcut::Move(step) => {
                let Some(next) = self
                    .active_mailbox()
                    .and_then(|mailbox| mailbox.neighbour(step))
                else {
                    return Task::none();
                };

                Task::batch([self.reveal_in_list(&next), self.select_conversation(next)])
            }
            Shortcut::Open => {
                let Some(selected) = self
                    .mailbox
                    .as_ref()
                    .and_then(|mailbox| mailbox.selected_conversation())
                    .map(str::to_owned)
                else {
                    return Task::none();
                };

                self.select_conversation(selected)
            }
            // One layer at a time, starting with the most recent.
            Shortcut::Dismiss => {
                if self.showing_settings {
                    self.showing_settings = false;
                    return Task::none();
                }
                if self.pending_link.take().is_some() {
                    return Task::none();
                }
                if let Some(mailbox) = self.active_mailbox() {
                    if mailbox.is_searching() {
                        mailbox.set_search_query(String::new());
                    } else {
                        mailbox.close_reader();
                    }
                }

                Task::none()
            }
            Shortcut::Refresh => self.refresh_mailbox(),
            Shortcut::Search => operation::focus(SEARCH_INPUT),
        }
    }

    pub fn auth_state(&self) -> &AuthState {
        &self.auth_state
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

    pub fn panels(&self) -> &pane_grid::State<Panel> {
        &self.panels
    }

    pub fn pending_link(&self) -> Option<&PendingLink> {
        self.pending_link.as_ref()
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

    fn sign_in(&mut self) -> Task<Message> {
        if let Some(prompt) = self.pending_prompt.take() {
            return self.answer_prompt(prompt);
        }

        let step = match self.auth_state {
            AuthState::SignedOut => SignInStep::Credentials,
            AuthState::NeedsTotp => SignInStep::Totp,
            AuthState::NeedsMailboxPassword => SignInStep::MailboxPassword,
            _ => return Task::none(),
        };

        if self.login_form.username.trim().is_empty() {
            self.auth_error = Some(AuthError::UsernameRequired);
            return Task::none();
        }
        if self.login_form.password.is_empty() {
            self.auth_error = Some(AuthError::PasswordRequired);
            return Task::none();
        }
        if step == SignInStep::Totp && self.login_form.totp.trim().is_empty() {
            self.auth_error = Some(AuthError::TotpRequired);
            return Task::none();
        }
        if step == SignInStep::MailboxPassword && self.login_form.mailbox_password.is_empty() {
            self.auth_error = Some(AuthError::MailboxPasswordRequired);
            return Task::none();
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
    fn show_prompt(&mut self, prompt: SignInPrompt) -> Task<Message> {
        if !matches!(self.auth_state, AuthState::SigningIn(_)) {
            prompt.cancel();
            return Task::none();
        }

        self.auth_error = None;
        let task = match &prompt {
            SignInPrompt::Totp(_) => {
                self.login_form.totp.clear();
                self.auth_state = AuthState::NeedsTotp;
                Task::none()
            }
            SignInPrompt::HumanVerification { url, .. } => {
                self.auth_state = AuthState::NeedsHumanVerification { url: url.clone() };
                open_in_browser(url.clone())
            }
        };
        self.pending_prompt = Some(prompt);
        task
    }

    fn answer_prompt(&mut self, prompt: SignInPrompt) -> Task<Message> {
        match prompt {
            SignInPrompt::Totp(reply) => {
                let code = self.login_form.totp.trim().to_owned();
                if code.is_empty() {
                    self.auth_error = Some(AuthError::TotpRequired);
                    self.pending_prompt = Some(SignInPrompt::Totp(reply));
                    return Task::none();
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
        Task::none()
    }

    fn finish_session_check(&mut self, outcome: ResumeOutcome) -> Task<Message> {
        match outcome {
            ResumeOutcome::Authenticated(service) => return self.set_authenticated(service),
            ResumeOutcome::SignedOut => self.auth_state = AuthState::SignedOut,
            ResumeOutcome::Failed(error) => {
                self.auth_error = Some(error);
                self.auth_state = AuthState::SignedOut;
            }
        }

        Task::none()
    }

    fn finish_sign_in(&mut self, outcome: SignInOutcome) -> Task<Message> {
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

        Task::none()
    }

    fn set_authenticated(&mut self, service: Arc<ProtonMailService>) -> Task<Message> {
        let email = service.email().map(str::to_owned);
        self.open_mailbox(MailBackend::Proton(service), email)
    }

    fn open_mailbox(&mut self, backend: MailBackend, email: Option<String>) -> Task<Message> {
        self.login_form.clear_all();
        self.auth_error = None;
        let page_size = backend.page_size();
        self.backend = Some(backend);
        self.auth_state = AuthState::Authenticated { email };

        let page_request = self.next_request();
        let counts_request = self.next_request();
        // Someone comes back to the folder they were last reading. The demo
        // account made no folders of its own, so a remembered one would open
        // a place with none of its fictional mail in it.
        let remembered = self.settings.folder.clone();
        let folder = if self.is_demo() && remembered.system().is_none() {
            Folder::INBOX
        } else {
            remembered
        };
        let (mailbox, page) = Mailbox::open(folder, page_size, page_request, counts_request);
        self.mailbox = Some(mailbox);

        Task::batch([
            self.fetch_page(Some(page)),
            self.fetch_counts(Some(counts_request)),
            self.fetch_folders(),
        ])
    }

    fn logout(&mut self) -> Task<Message> {
        if !matches!(self.auth_state, AuthState::Authenticated { .. }) {
            return Task::none();
        }
        let service = match &self.backend {
            Some(MailBackend::Proton(service)) => service.clone(),
            // Leaving demo mode has no Proton session to revoke.
            Some(MailBackend::Demo(_)) => {
                self.close_mailbox(None);
                return Task::none();
            }
            None => {
                self.auth_state = AuthState::SignedOut;
                return Task::none();
            }
        };

        self.auth_error = None;
        self.auth_state = AuthState::SigningOut;
        Task::perform(
            async move { service.logout().await },
            Message::LogoutFinished,
        )
    }

    fn finish_logout(&mut self, result: Result<(), AuthError>) {
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
    fn close_mailbox(&mut self, error: Option<AuthError>) {
        self.backend = None;
        self.mailbox = None;
        self.pending_link = None;
        self.showing_settings = false;
        self.login_form.clear_all();
        self.auth_error = error;
        self.auth_state = AuthState::SignedOut;
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

    fn select_conversation(&mut self, id: String) -> Task<Message> {
        if !matches!(self.auth_state, AuthState::Authenticated { .. }) {
            return Task::none();
        }
        self.pending_link = None;
        if let Some(MailBackend::Demo(service)) = self.backend.clone() {
            if !service.set_unread(&id, false) {
                return Task::none();
            }
            self.apply_demo_snapshot(&service);
        }

        self.start_conversation_load(id)
    }

    fn apply_action(&mut self, action: MailAction) -> Task<Message> {
        if !matches!(self.auth_state, AuthState::Authenticated { .. }) {
            return Task::none();
        }
        match self.backend.clone() {
            Some(MailBackend::Demo(service)) => self.apply_demo_action(&service, action),
            Some(MailBackend::Proton(service)) => self.start_proton_action(service, action),
            None => Task::none(),
        }
    }

    /// Demo actions change local data at once and reload the folder snapshot.
    fn apply_demo_action(&mut self, service: &DemoMailbox, action: MailAction) -> Task<Message> {
        let Some(mailbox) = self.mailbox.as_ref() else {
            return Task::none();
        };
        if mailbox.is_busy() {
            return Task::none();
        }
        let Some((id, kind)) = mailbox
            .selected_summary()
            .map(|summary| (summary.id.clone(), summary.kind))
        else {
            return Task::none();
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

        Task::none()
    }

    /// Puts the last moved conversation back. The row has left the list, so
    /// the action names it directly instead of going through the selection.
    fn undo_move(&mut self) -> Task<Message> {
        let Some(undo) = self.active_mailbox().and_then(Mailbox::take_undo) else {
            return Task::none();
        };
        let action = MailAction::MoveTo(undo.from.clone());

        match self.backend.clone() {
            Some(MailBackend::Demo(service)) => {
                if service.move_to(&undo.row_id, &undo.from) {
                    self.apply_demo_snapshot(&service);
                }

                Task::none()
            }
            Some(MailBackend::Proton(service)) => {
                let request = self.next_request();
                let Some(request) = self.active_mailbox().and_then(|mailbox| {
                    mailbox.start_action_on(undo.row_id, undo.kind, action, request)
                }) else {
                    return Task::none();
                };
                run_action(service, request, Message::ActionFinished)
            }
            None => Task::none(),
        }
    }

    /// Proton actions run asynchronously; the list changes only once Proton
    /// confirms them.
    fn start_proton_action(
        &mut self,
        service: Arc<ProtonMailService>,
        action: MailAction,
    ) -> Task<Message> {
        let request = self.next_request();
        let Some(request) = self
            .active_mailbox()
            .and_then(|mailbox| mailbox.start_action(action, request))
        else {
            return Task::none();
        };
        run_action(service, request, Message::ActionFinished)
    }

    fn finish_mail_action(
        &mut self,
        request: ActionRequest,
        result: Result<(), MailboxError>,
    ) -> Task<Message> {
        let Some(outcome) = self
            .mailbox
            .as_mut()
            .map(|mailbox| mailbox.finish_action(&request, result))
        else {
            return Task::none();
        };
        let next = match outcome {
            Ok(next) => next,
            Err(error) => {
                self.handle_mailbox_error(Some(error));
                return Task::none();
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
        let open_next = next.map_or_else(Task::none, |id| {
            Task::batch([self.reveal_in_list(&id), self.select_conversation(id)])
        });

        Task::batch([counts, open_next])
    }

    /// Once an unread Proton row's content is shown, marks it read on Proton.
    /// Demo rows are already marked read when they are selected.
    fn mark_opened_read(&mut self) -> Task<Message> {
        if !self.settings.mark_read_on_open {
            return Task::none();
        }
        let Some(MailBackend::Proton(service)) = self.backend.clone() else {
            return Task::none();
        };
        let request = self.next_request();
        let Some(request) = self
            .active_mailbox()
            .and_then(|mailbox| mailbox.start_mark_read(request))
        else {
            return Task::none();
        };
        run_action(service, request, Message::MarkReadFinished)
    }

    fn finish_mark_read(
        &mut self,
        request: ActionRequest,
        result: Result<(), MailboxError>,
    ) -> Task<Message> {
        match self
            .mailbox
            .as_mut()
            .map(|mailbox| mailbox.finish_mark_read(&request, result))
        {
            Some(Ok(true)) => self.reload_counts(),
            // Only an expired session matters; the row simply stays unread.
            Some(Err(error)) => {
                self.handle_mailbox_error(Some(error));
                Task::none()
            }
            _ => Task::none(),
        }
    }

    /// Proton owns the folder counts; reload them after a change.
    fn reload_counts(&mut self) -> Task<Message> {
        let counts_request = self.next_request();
        let counts = self
            .active_mailbox()
            .and_then(|mailbox| mailbox.refresh_counts(counts_request));

        self.fetch_counts(counts)
    }

    fn start_conversation_load(&mut self, id: String) -> Task<Message> {
        let request = self.next_request();
        let request = self
            .active_mailbox()
            .and_then(|mailbox| mailbox.start_conversation_load(id, request));
        if request.is_none() {
            return Task::none();
        }

        // A new conversation starts at its top, not where the last one was
        // left; the reader pane keeps its scroll offset otherwise.
        Task::batch([
            operation::snap_to(READER_BODY, RelativeOffset::START),
            self.fetch_conversation(request),
        ])
    }

    /// Brings a row the app opened on its own into view. A row the user
    /// clicked is already on screen, so only these are worth scrolling to.
    fn reveal_in_list(&self, row_id: &str) -> Task<Message> {
        let Some(y) = self
            .mailbox
            .as_ref()
            .and_then(|mailbox| mailbox.visible_position(row_id))
        else {
            return Task::none();
        };

        operation::snap_to(CONVERSATION_LIST, RelativeOffset { x: 0.0, y })
    }

    fn retry_conversation(&mut self) -> Task<Message> {
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
    fn reconcile_open_folder(&mut self) -> Task<Message> {
        let Some(open) = self
            .mailbox
            .as_ref()
            .map(|mailbox| mailbox.folder().clone())
        else {
            return Task::none();
        };
        // Proton's own folders are always there, so only the account's own
        // can have gone stale.
        let Some(id) = open.custom_id() else {
            return Task::none();
        };
        // Gone from the account means gone from the sidebar, and the Inbox is
        // the one place that is always there to fall back to.
        let folder = self
            .folders
            .iter()
            .find(|place| place.custom_id() == Some(id))
            .cloned()
            .unwrap_or(Folder::INBOX);
        if folder == open {
            return Task::none();
        }

        self.select_folder(folder)
    }

    fn select_folder(&mut self, folder: Folder) -> Task<Message> {
        self.pending_link = None;
        self.saved_attachment = None;
        // The folder someone left off in is the one they want on the next
        // run. Demo mail is fictional, so where it is read is not worth
        // remembering, let alone worth overwriting the real answer with.
        if !self.is_demo() {
            self.remember(|settings| settings.folder = folder.clone());
        }
        let request = self.next_request();
        let page = self
            .active_mailbox()
            .and_then(|mailbox| mailbox.select_folder(folder, request));

        self.fetch_page(page)
    }

    /// Runs what is typed in the search field against the server, which sees
    /// every folder and every conversation, not just the loaded ones.
    fn start_search(&mut self) -> Task<Message> {
        let request = self.next_request();
        let Some(request) = self
            .active_mailbox()
            .and_then(|mailbox| mailbox.start_search(request))
        else {
            return Task::none();
        };

        self.fetch_search(request)
    }

    fn fetch_search(&self, request: SearchRequest) -> Task<Message> {
        let Some(backend) = self.backend.clone() else {
            return Task::none();
        };
        let query = request.query.clone();
        let limit = backend.page_size();

        Task::perform(
            async move { backend.search(&query, limit).await },
            move |result| Message::SearchLoaded(request.clone(), result),
        )
    }

    fn refresh_mailbox(&mut self) -> Task<Message> {
        let page_request = self.next_request();
        let counts_request = self.next_request();
        let Some(mailbox) = self.active_mailbox() else {
            return Task::none();
        };
        let page = mailbox.refresh(page_request);
        let counts = mailbox.refresh_counts(counts_request);

        Task::batch([self.fetch_page(page), self.fetch_counts(counts)])
    }

    fn load_more_conversations(&mut self) -> Task<Message> {
        let request = self.next_request();
        let page = self
            .active_mailbox()
            .and_then(|mailbox| mailbox.load_more(request));

        self.fetch_page(page)
    }

    fn fetch_page(&self, request: Option<PageRequest>) -> Task<Message> {
        let (Some(request), Some(backend)) = (request, self.backend.clone()) else {
            return Task::none();
        };

        let (id, folder, page, page_size) =
            (request.id, request.folder, request.page, request.page_size);

        Task::perform(
            async move { backend.list_conversations(&folder, page, page_size).await },
            move |result| Message::ConversationsLoaded(id, result),
        )
    }

    /// Asks once for the folders the account made. They change rarely, so
    /// nothing re-fetches them until the mailbox is opened again.
    fn fetch_folders(&self) -> Task<Message> {
        let Some(backend) = self.backend.clone() else {
            return Task::none();
        };

        Task::perform(
            async move { backend.list_folders().await },
            Message::FoldersLoaded,
        )
    }

    fn fetch_counts(&self, request: Option<RequestId>) -> Task<Message> {
        let (Some(request), Some(backend)) = (request, self.backend.clone()) else {
            return Task::none();
        };

        let folders = self.folders.clone();

        Task::perform(
            async move { backend.conversation_counts(&folders).await },
            move |result| Message::CountsLoaded(request, result),
        )
    }

    fn fetch_conversation(&self, request: Option<ReaderRequest>) -> Task<Message> {
        let (Some(request), Some(backend)) = (request, self.backend.clone()) else {
            return Task::none();
        };
        let (kind, id) = (request.kind, request.conversation_id.clone());

        Task::perform(
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
) -> Task<Message> {
    let pending = request.clone();

    Task::perform(
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

/// Runs one sign-in, forwarding its prompts and final outcome as messages.
fn sign_in_task(request: LoginRequest) -> Task<Message> {
    Task::run(
        iced::stream::channel(1, async move |events| {
            ProtonMailService::sign_in(request, events).await;
        }),
        |event| match event {
            SignInEvent::Prompt(prompt) => Message::SignInPrompt(prompt),
            SignInEvent::Finished(outcome) => Message::SignInFinished(outcome),
        },
    )
}

// ponytail: launch failures are ignored; the page offers "Copy link" instead.
fn open_in_browser(url: String) -> Task<Message> {
    Task::future(async move {
        let _ = open_url(&url);
    })
    .discard()
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
mod tests {
    use super::*;
    use crate::mail::demo;

    use crate::mail::MailFolder;

    /// One of Proton's own folders, as a place to list from.
    fn sys(folder: MailFolder) -> Folder {
        Folder::System(folder)
    }

    const NOW: i64 = 1_789_000_000;

    fn authenticated_app() -> App {
        let mut app = App::new(Settings::default());
        app.auth_state = AuthState::Authenticated { email: None };
        app.mailbox = Some(Mailbox::open(Folder::INBOX, 50, 1, 2).0);
        app.last_request = 2;
        app
    }

    /// Delivers the demo page a request would produce, as the async task would.
    fn deliver_demo_page(app: &mut App, request: RequestId, page: u32) {
        let folder = app.mailbox().unwrap().folder();
        let result = demo_service(app).list_conversations(folder, page, demo::PAGE_SIZE, NOW);
        let _ = app.update(Message::ConversationsLoaded(request, result));
    }

    fn deliver_latest_demo_page(app: &mut App, page: u32) {
        let request = app.last_request;
        deliver_demo_page(app, request, page);
    }

    fn demo_service(app: &App) -> demo::DemoMailbox {
        match app.backend.as_ref() {
            Some(MailBackend::Demo(service)) => service.clone(),
            _ => panic!("expected demo backend"),
        }
    }

    fn pending_reader_request(app: &App) -> ReaderRequest {
        match app.mailbox().unwrap().reader_state() {
            ReaderState::Loading {
                conversation_id,
                request,
            } => ReaderRequest {
                id: *request,
                conversation_id: conversation_id.clone(),
                kind: crate::mail::SummaryKind::Conversation,
            },
            _ => panic!("expected pending reader request"),
        }
    }

    fn deliver_selected_demo_detail(app: &mut App) {
        let request = pending_reader_request(app);
        let result = demo_service(app).conversation_detail(&request.conversation_id, NOW);
        let _ = app.update(Message::ConversationLoaded(request, result));
    }

    /// Runs what is typed against the demo backend, the way pressing Enter
    /// asks the server for everything matching it.
    fn deliver_demo_search(app: &mut App, query: &str) {
        let _ = app.update(Message::SearchChanged(query.to_owned()));
        let _ = app.update(Message::SearchSubmitted);
        let request = SearchRequest {
            id: app.last_request,
            query: query.to_owned(),
        };
        let result = demo_service(app).search(query, demo::PAGE_SIZE, NOW);
        let _ = app.update(Message::SearchLoaded(request, result));
    }

    #[test]
    fn settings_changes_reach_the_app() {
        let mut app = loaded_demo_app();

        // Both start the way the app behaved before it could be configured.
        assert!(app.settings().mark_read_on_open);
        assert!(app.settings().confirm_links);

        let _ = app.update(Message::SetMarkReadOnOpen(false));
        let _ = app.update(Message::SetConfirmLinks(false));

        assert!(!app.settings().mark_read_on_open);
        assert!(!app.settings().confirm_links);
    }

    #[test]
    fn a_link_skips_the_prompt_only_when_confirmation_is_off() {
        let mut app = loaded_demo_app();

        let _ = app.update(Message::LinkClicked("https://example.com/a".into()));
        assert!(
            app.pending_link().is_some(),
            "confirmation is on by default"
        );
        let _ = app.update(Message::DismissLink);

        let _ = app.update(Message::SetConfirmLinks(false));
        let _ = app.update(Message::LinkClicked("https://example.com/a".into()));
        assert!(
            app.pending_link().is_none(),
            "the link opens without asking"
        );

        // Refusing what is not a web or mail link does not depend on the
        // setting: it is a rule, not a preference.
        let _ = app.update(Message::LinkClicked("javascript:alert(1)".into()));
        assert!(app.pending_link().is_none());
    }

    #[test]
    fn the_settings_page_covers_the_mailbox_and_steps_back() {
        let mut app = loaded_demo_app();
        assert!(!app.showing_settings());

        let _ = app.update(Message::ShowSettings(true));
        assert!(app.showing_settings());

        // Escape backs out of the page before anything underneath it.
        let _ = app.update(Message::Keyboard(keyboard::Event::KeyPressed {
            key: Key::Named(keyboard::key::Named::Escape),
            modified_key: Key::Named(keyboard::key::Named::Escape),
            physical_key: keyboard::key::Physical::Unidentified(
                keyboard::key::NativeCode::Unidentified,
            ),
            location: keyboard::Location::Standard,
            modifiers: Modifiers::default(),
            text: None,
            repeat: false,
        }));

        assert!(!app.showing_settings());
    }

    #[test]
    fn keys_map_to_mailbox_shortcuts() {
        let plain = Modifiers::default();
        let command = Modifiers::COMMAND;
        let down = || Key::Named(keyboard::key::Named::ArrowDown);

        assert_eq!(
            shortcut(Key::Character("j"), plain),
            Some(Shortcut::Move(Step::Next))
        );
        assert_eq!(shortcut(down(), plain), Some(Shortcut::Move(Step::Next)));
        assert_eq!(
            shortcut(Key::Character("k"), plain),
            Some(Shortcut::Move(Step::Previous))
        );
        assert_eq!(
            shortcut(Key::Named(keyboard::key::Named::Enter), plain),
            Some(Shortcut::Open)
        );
        assert_eq!(
            shortcut(Key::Named(keyboard::key::Named::Escape), plain),
            Some(Shortcut::Dismiss)
        );
        assert_eq!(
            shortcut(Key::Character("r"), command),
            Some(Shortcut::Refresh)
        );
        assert_eq!(
            shortcut(Key::Character("f"), command),
            Some(Shortcut::Search)
        );

        // A modifier turns a plain shortcut into somebody else's business.
        assert_eq!(shortcut(Key::Character("j"), command), None);
        assert_eq!(shortcut(down(), command), None);
        assert_eq!(shortcut(Key::Character("r"), plain), None);
        assert_eq!(shortcut(Key::Character("z"), plain), None);
    }

    /// Sends a plain key press, as the window would when no field took it.
    fn press(app: &mut App, key: Key<&str>) {
        let named = match key {
            Key::Character(c) => Key::Character(c.into()),
            Key::Named(named) => Key::Named(named),
            Key::Unidentified => Key::Unidentified,
        };
        let _ = app.update(Message::Keyboard(keyboard::Event::KeyPressed {
            key: named.clone(),
            modified_key: named,
            physical_key: keyboard::key::Physical::Unidentified(
                keyboard::key::NativeCode::Unidentified,
            ),
            location: keyboard::Location::Standard,
            modifiers: Modifiers::default(),
            text: None,
            repeat: false,
        }));
    }

    #[test]
    fn signing_out_from_the_settings_page_comes_back_to_the_mailbox() {
        let mut app = loaded_demo_app();
        let _ = app.update(Message::ShowSettings(true));

        let _ = app.update(Message::Logout);

        // Otherwise signing back in lands on the settings page again, with
        // no sign that it was ever left open.
        assert!(!app.showing_settings());
    }

    #[test]
    fn keys_do_not_reach_a_covered_mailbox() {
        let mut app = loaded_demo_app();
        press(&mut app, Key::Character("j"));
        let opened = app
            .mailbox()
            .unwrap()
            .selected_conversation()
            .map(str::to_owned);
        assert!(opened.is_some());

        let _ = app.update(Message::ShowSettings(true));
        // Stepping the list underneath the settings page would change the
        // selection out of sight, and ask Proton for a conversation nobody
        // can see.
        press(&mut app, Key::Character("j"));
        assert_eq!(
            app.mailbox().unwrap().selected_conversation(),
            opened.as_deref()
        );

        // Escape still backs out of the page, and the list answers again.
        press(&mut app, Key::Named(keyboard::key::Named::Escape));
        assert!(!app.showing_settings());
        press(&mut app, Key::Character("j"));
        assert_ne!(
            app.mailbox().unwrap().selected_conversation(),
            opened.as_deref()
        );
    }

    #[test]
    fn keys_do_not_reach_the_mailbox_under_a_link_prompt() {
        let mut app = loaded_demo_app();
        press(&mut app, Key::Character("j"));
        let opened = app
            .mailbox()
            .unwrap()
            .selected_conversation()
            .map(str::to_owned);

        let _ = app.update(Message::LinkClicked("https://example.com/".to_owned()));
        assert!(app.pending_link().is_some());

        press(&mut app, Key::Character("j"));
        assert_eq!(
            app.mailbox().unwrap().selected_conversation(),
            opened.as_deref()
        );

        press(&mut app, Key::Named(keyboard::key::Named::Escape));
        assert!(app.pending_link().is_none());
    }

    #[test]
    fn the_keyboard_walks_the_conversation_list() {
        let mut app = loaded_demo_app();

        // Nothing is open, so the first key opens the first conversation.
        press(&mut app, Key::Character("j"));
        let selected = |app: &App| {
            app.mailbox()
                .unwrap()
                .selected_conversation()
                .map(str::to_owned)
        };
        assert_eq!(selected(&app).as_deref(), Some("demo-0"));

        press(&mut app, Key::Character("j"));
        assert_eq!(selected(&app).as_deref(), Some("demo-1"));

        press(&mut app, Key::Character("k"));
        assert_eq!(selected(&app).as_deref(), Some("demo-0"));

        // The list has an end, and Escape closes the reader.
        press(&mut app, Key::Character("k"));
        assert_eq!(selected(&app).as_deref(), Some("demo-0"));

        press(&mut app, Key::Named(keyboard::key::Named::Escape));
        assert_eq!(selected(&app), None);
    }

    #[test]
    fn the_account_folders_reach_the_sidebar() {
        let mut app = loaded_demo_app();
        assert!(app.folders().is_empty(), "the demo account has none");

        let invoices = Folder::custom("kZ9", "Invoices");
        let receipts = Folder::label("wN2", "Receipts");
        let _ = app.update(Message::FoldersLoaded(Ok(vec![
            invoices.clone(),
            receipts.clone(),
        ])));

        assert_eq!(app.folders(), [invoices, receipts]);
    }

    #[test]
    fn the_mailbox_opens_where_it_was_left() {
        let (app, _) = App::boot(true, Settings::opening(sys(MailFolder::Archive)));

        assert_eq!(app.mailbox().unwrap().folder(), &sys(MailFolder::Archive));
    }

    #[test]
    fn the_demo_opens_its_own_mailbox_and_leaves_the_setting_alone() {
        let invoices = Folder::custom("kZ9", "Invoices");
        let (mut app, _) = App::boot(true, Settings::opening(invoices.clone()));

        // None of the demo's fictional mail is in a folder the real account
        // made, so opening one would show an empty list under a name the
        // demo sidebar does not even carry.
        assert_eq!(app.mailbox().unwrap().folder(), &Folder::INBOX);

        let _ = app.update(Message::SelectFolder(sys(MailFolder::Archive)));
        // Reading demo mail says nothing about where the real mailbox was
        // left, so the remembered folder is untouched.
        assert_eq!(app.settings.folder, invoices);
    }

    #[test]
    fn a_renamed_folder_is_taken_up_and_a_removed_one_falls_back() {
        let mut app = loaded_demo_app();
        let _ = app.update(Message::SelectFolder(Folder::custom("kZ9", "Invoices")));

        // The account renamed it since the settings file was written, so the
        // sidebar and the header agree on the name it has now.
        let renamed = Folder::custom("kZ9", "Facturas");
        let _ = app.update(Message::FoldersLoaded(Ok(vec![renamed.clone()])));
        assert_eq!(app.mailbox().unwrap().folder(), &renamed);

        // Once it is gone from the account there is nothing to show under it,
        // so reading falls back to the one folder that is always there.
        let _ = app.update(Message::FoldersLoaded(Ok(Vec::new())));
        assert_eq!(app.mailbox().unwrap().folder(), &Folder::INBOX);
    }

    #[test]
    fn only_web_and_mail_links_are_offered() {
        let web = PendingLink::parse(" https://Example.com/path?x=1 ").unwrap();
        assert_eq!(web.target, "example.com");
        assert_eq!(web.url, "https://example.com/path?x=1");
        assert_eq!(
            PendingLink::parse("mailto:team@example.org")
                .unwrap()
                .target,
            "team@example.org"
        );
        // The real host shows even when the link hides it behind a user name.
        assert_eq!(
            PendingLink::parse("https://bank.example@evil.example/login")
                .unwrap()
                .target,
            "evil.example"
        );

        for rejected in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "data:text/html,x",
            "/relative",
            "not a link",
        ] {
            assert_eq!(PendingLink::parse(rejected), None, "{rejected}");
        }
    }

    #[test]
    fn clicked_links_wait_for_confirmation_and_clear() {
        let mut app = loaded_demo_app();

        let _ = app.update(Message::LinkClicked("https://example.com/a".into()));
        assert_eq!(
            app.pending_link().map(|link| link.target.as_str()),
            Some("example.com")
        );
        let _ = app.update(Message::DismissLink);
        assert!(app.pending_link().is_none());

        let _ = app.update(Message::LinkClicked("javascript:alert(1)".into()));
        assert!(app.pending_link().is_none());

        let _ = app.update(Message::LinkClicked("https://example.com/a".into()));
        let _ = app.update(Message::OpenLink);
        assert!(app.pending_link().is_none());

        let _ = app.update(Message::LinkClicked("https://example.com/a".into()));
        let _ = app.update(Message::SelectFolder(sys(MailFolder::Sent)));
        assert!(app.pending_link().is_none());
    }

    #[test]
    fn rerender_inputs_issue_no_mailbox_requests() {
        let mut app = loaded_demo_app();
        let requests = app.last_request;
        let split = *app.panels().layout().splits().next().unwrap();

        for message in [
            Message::SearchChanged("alex".into()),
            Message::PanelResized(pane_grid::ResizeEvent { split, ratio: 0.3 }),
            Message::SearchChanged(String::new()),
        ] {
            assert_eq!(app.update(message).units(), 0);
        }

        assert_eq!(app.last_request, requests);
    }

    fn loaded_demo_app() -> App {
        let (mut app, _) = App::boot(true, Settings::default());
        deliver_demo_page(&mut app, 1, 0);
        let counts = demo_service(&app).counts();
        let _ = app.update(Message::CountsLoaded(2, Ok(counts)));
        app
    }

    #[test]
    fn initial_state_checks_saved_session() {
        let app = App::new(Settings::default());

        assert_eq!(app.auth_state, AuthState::CheckingSession);
    }

    #[test]
    fn normal_startup_checks_the_proton_session() {
        let (app, _) = App::boot(false, Settings::default());

        assert_eq!(app.auth_state, AuthState::CheckingSession);
        assert!(!app.is_demo());
        assert!(app.backend.is_none());
        assert!(app.mailbox.is_none());
    }

    #[test]
    fn demo_startup_opens_mailbox_without_session_resume() {
        let (app, _) = App::boot(true, Settings::default());

        assert!(app.is_demo());
        assert_eq!(app.auth_state, AuthState::Authenticated { email: None });
        let mailbox = app.mailbox().unwrap();
        assert_eq!(mailbox.folder(), &sys(MailFolder::Inbox));
        assert_eq!(mailbox.status(), ListStatus::Loading(1));
    }

    #[test]
    fn demo_mailbox_uses_production_pagination() {
        let (mut app, _) = App::boot(true, Settings::default());
        deliver_demo_page(&mut app, 1, 0);
        let counts = demo_service(&app).counts();
        let _ = app.update(Message::CountsLoaded(2, Ok(counts)));

        let mailbox = app.mailbox().unwrap();
        assert_eq!(mailbox.conversations().len(), 10);
        assert!(mailbox.has_more());
        assert_eq!(
            mailbox.counts().unwrap().unread(&sys(MailFolder::Inbox)),
            Some(6)
        );

        for page in 1..=3 {
            let _ = app.update(Message::LoadMoreConversations);
            deliver_latest_demo_page(&mut app, page);
        }

        let mailbox = app.mailbox().unwrap();
        assert_eq!(mailbox.conversations().len(), 34);
        assert!(!mailbox.has_more());
        assert_eq!(mailbox.status(), ListStatus::Loaded);
    }

    #[test]
    fn demo_empty_and_spam_folders_load() {
        let (mut app, _) = App::boot(true, Settings::default());

        let _ = app.update(Message::SelectFolder(sys(MailFolder::Trash)));
        deliver_latest_demo_page(&mut app, 0);
        let mailbox = app.mailbox().unwrap();
        assert_eq!(mailbox.status(), ListStatus::Loaded);
        assert!(mailbox.conversations().is_empty());

        let _ = app.update(Message::SelectFolder(sys(MailFolder::Spam)));
        deliver_latest_demo_page(&mut app, 0);
        let mailbox = app.mailbox().unwrap();
        assert_eq!(mailbox.status(), ListStatus::Loaded);
        assert_eq!(mailbox.conversations().len(), 2);
        assert!(app.is_demo());
    }

    #[test]
    fn demo_selection_is_local() {
        let (mut app, _) = App::boot(true, Settings::default());
        deliver_demo_page(&mut app, 1, 0);

        let _ = app.update(Message::SelectConversation("demo-3".into()));

        assert_eq!(
            app.mailbox().unwrap().selected_conversation(),
            Some("demo-3")
        );
    }

    #[test]
    fn selecting_only_queues_a_detail_request() {
        let mut app = loaded_demo_app();
        let status = app.mailbox().unwrap().status();
        let counts = app.mailbox().unwrap().counts().cloned();
        let conversation_ids: Vec<_> = app
            .mailbox()
            .unwrap()
            .conversations()
            .iter()
            .map(|conversation| conversation.id.clone())
            .collect();

        let task = app.update(Message::SelectConversation("demo-3".into()));

        // The detail request and the scroll back to the top of the reader.
        // `last_request` is what proves only one of them reaches the backend.
        assert_eq!(task.units(), 2);
        assert_eq!(app.last_request, 3);
        let mailbox = app.mailbox().unwrap();
        assert_eq!(mailbox.status(), status);
        assert_eq!(mailbox.counts().cloned(), counts);
        assert_eq!(
            mailbox
                .conversations()
                .iter()
                .map(|conversation| conversation.id.clone())
                .collect::<Vec<_>>(),
            conversation_ids
        );
        assert!(matches!(
            mailbox.reader_state(),
            ReaderState::Loading {
                conversation_id,
                request: 3,
            } if conversation_id == "demo-3"
        ));
    }

    #[test]
    fn no_selection_has_no_reader() {
        let (mut app, _) = App::boot(true, Settings::default());
        deliver_demo_page(&mut app, 1, 0);

        assert_eq!(app.mailbox().unwrap().reader_state(), &ReaderState::Empty);
    }

    #[test]
    fn opened_plain_bodies_are_selectable_without_asking() {
        let mut app = loaded_demo_app();
        let _ = app.update(Message::SelectConversation("demo-0".into()));
        deliver_selected_demo_detail(&mut app);

        // Demo bodies are plain text, so they arrive ready to select.
        let content = app
            .mailbox()
            .unwrap()
            .selectable_body("demo-0-2")
            .expect("a plain body is selectable");
        assert!(!content.text().trim().is_empty());

        // The switch still works both ways through the message path.
        let _ = app.update(Message::ToggleTextSelection("demo-0-2".into()));
        assert!(app.mailbox().unwrap().selectable_body("demo-0-2").is_none());

        let _ = app.update(Message::ToggleTextSelection("demo-0-2".into()));
        assert!(app.mailbox().unwrap().selectable_body("demo-0-2").is_some());
    }

    #[test]
    fn demo_selection_opens_reader_with_newest_message_expanded() {
        let (mut app, _) = App::boot(true, Settings::default());
        deliver_demo_page(&mut app, 1, 0);

        let _ = app.update(Message::SelectConversation("demo-0".into()));
        deliver_selected_demo_detail(&mut app);

        let reader = app.mailbox().unwrap().reader().unwrap();
        assert_eq!(reader.conversation_id(), "demo-0");
        assert_eq!(reader.detail().id, reader.conversation_id());
        assert_eq!(reader.detail().messages.len(), 3);
        assert!(reader.is_expanded("demo-0-2"));
        assert!(!reader.is_expanded("demo-0-0"));
    }

    #[test]
    fn toggling_messages_and_switching_conversations() {
        let (mut app, _) = App::boot(true, Settings::default());
        deliver_demo_page(&mut app, 1, 0);
        let _ = app.update(Message::SelectConversation("demo-0".into()));
        deliver_selected_demo_detail(&mut app);

        let _ = app.update(Message::ToggleMessageExpanded("demo-0-0".into()));
        let reader = app.mailbox().unwrap().reader().unwrap();
        assert!(reader.is_expanded("demo-0-0"));
        assert!(reader.is_expanded("demo-0-2"));

        let _ = app.update(Message::SelectConversation("demo-2".into()));
        deliver_selected_demo_detail(&mut app);
        let _ = app.update(Message::SelectConversation("demo-0".into()));
        deliver_selected_demo_detail(&mut app);
        assert!(
            !app.mailbox()
                .unwrap()
                .reader()
                .unwrap()
                .is_expanded("demo-0-0")
        );
    }

    #[test]
    fn folder_switch_closes_reader() {
        let (mut app, _) = App::boot(true, Settings::default());
        deliver_demo_page(&mut app, 1, 0);
        let _ = app.update(Message::SelectConversation("demo-0".into()));

        let _ = app.update(Message::SelectFolder(sys(MailFolder::Sent)));

        assert!(app.mailbox().unwrap().reader().is_none());
    }

    #[test]
    fn detail_failure_and_retry_use_a_new_request() {
        let mut app = loaded_demo_app();
        let _ = app.update(Message::SelectConversation("demo-3".into()));
        let first = pending_reader_request(&app);
        let _ = app.update(Message::ConversationLoaded(
            first.clone(),
            Err(MailboxError::Connection),
        ));
        assert!(matches!(
            app.mailbox().unwrap().reader_state(),
            ReaderState::Failed {
                conversation_id,
                error: MailboxError::Connection,
            } if conversation_id == "demo-3"
        ));

        let task = app.update(Message::RetryConversation);
        let retry = pending_reader_request(&app);

        assert_eq!(task.units(), 1);
        assert!(retry.id > first.id);
        assert_eq!(retry.conversation_id, first.conversation_id);
    }

    #[test]
    fn demo_read_actions_update_row_and_count_idempotently() {
        let mut app = loaded_demo_app();
        let _ = app.update(Message::SearchChanged("design review".into()));
        let _ = app.update(Message::SelectConversation("demo-0".into()));
        deliver_selected_demo_detail(&mut app);
        let detail = app.mailbox().unwrap().reader().unwrap().detail().clone();

        assert!(!app.mailbox().unwrap().conversations()[0].unread);
        assert_eq!(
            app.mailbox()
                .unwrap()
                .counts()
                .unwrap()
                .unread(&sys(MailFolder::Inbox)),
            Some(5)
        );

        let _ = app.update(Message::ApplyAction(MailAction::SetUnread(true)));
        let _ = app.update(Message::ApplyAction(MailAction::SetUnread(true)));
        assert_eq!(app.mailbox().unwrap().search_query(), "design review");
        assert_eq!(app.mailbox().unwrap().visible_conversations().count(), 1);
        assert!(app.mailbox().unwrap().conversations()[0].unread);
        assert_eq!(
            app.mailbox()
                .unwrap()
                .counts()
                .unwrap()
                .unread(&sys(MailFolder::Inbox)),
            Some(6)
        );

        let _ = app.update(Message::ApplyAction(MailAction::SetUnread(false)));
        let _ = app.update(Message::ApplyAction(MailAction::SetUnread(false)));
        let _ = app.update(Message::ApplyAction(MailAction::SetStarred(true)));
        assert_eq!(app.mailbox().unwrap().reader().unwrap().detail(), &detail);
        assert!(!app.mailbox().unwrap().conversations()[0].unread);
        assert_eq!(
            app.mailbox()
                .unwrap()
                .counts()
                .unwrap()
                .unread(&sys(MailFolder::Inbox)),
            Some(5)
        );
    }

    #[test]
    fn demo_star_actions_update_starred_folder_and_selection() {
        let mut app = loaded_demo_app();
        let _ = app.update(Message::SelectConversation("demo-1".into()));
        deliver_selected_demo_detail(&mut app);
        let _ = app.update(Message::ApplyAction(MailAction::SetStarred(true)));
        let _ = app.update(Message::ApplyAction(MailAction::SetStarred(true)));
        assert!(app.mailbox().unwrap().conversations()[1].starred);

        let _ = app.update(Message::SelectFolder(sys(MailFolder::Starred)));
        deliver_latest_demo_page(&mut app, 0);
        assert!(
            app.mailbox()
                .unwrap()
                .conversations()
                .iter()
                .any(|conversation| conversation.id == "demo-1")
        );

        let _ = app.update(Message::SelectConversation("demo-1".into()));
        deliver_selected_demo_detail(&mut app);
        let _ = app.update(Message::ApplyAction(MailAction::SetStarred(false)));
        let mailbox = app.mailbox().unwrap();
        assert!(
            mailbox
                .conversations()
                .iter()
                .all(|conversation| conversation.id != "demo-1")
        );
        assert!(
            mailbox
                .selected_conversation()
                .is_some_and(|id| id != "demo-1")
        );
    }

    #[test]
    fn starred_search_reflects_unstar_immediately() {
        let mut app = loaded_demo_app();
        let _ = app.update(Message::SelectFolder(sys(MailFolder::Starred)));
        deliver_latest_demo_page(&mut app, 0);
        let _ = app.update(Message::SearchChanged("offsite".into()));
        let _ = app.update(Message::SelectConversation("demo-2".into()));

        let _ = app.update(Message::ApplyAction(MailAction::SetStarred(false)));

        let mailbox = app.mailbox().unwrap();
        assert_eq!(mailbox.search_query(), "offsite");
        assert_eq!(mailbox.visible_conversations().count(), 0);
        assert_eq!(mailbox.selected_conversation(), None);
    }

    #[test]
    fn one_attachment_is_saved_at_a_time() {
        let mut app = loaded_demo_app();

        // The demo carries no files, so the fetch itself fails; what matters
        // is that the second press never starts a fetch of its own, which is
        // how the same file ended up written twice under two names.
        let _ = app.update(Message::SaveAttachment("demo-0".into(), "file".into()));
        assert_eq!(app.saving_attachment(), Some("file"));

        let _ = app.update(Message::SaveAttachment("demo-0".into(), "other".into()));
        assert_eq!(app.saving_attachment(), Some("file"));

        let _ = app.update(Message::AttachmentSaved(Err(SaveError::NotFetched)));
        assert_eq!(app.saving_attachment(), None);
        // Once the first one is done, the next press is free to go.
        let _ = app.update(Message::SaveAttachment("demo-0".into(), "other".into()));
        assert_eq!(app.saving_attachment(), Some("other"));
    }

    #[test]
    fn a_conversation_the_server_found_can_still_be_acted_on() {
        let mut app = loaded_demo_app();
        deliver_demo_search(&mut app, "offsite");
        let found = app
            .mailbox()
            .unwrap()
            .visible_conversations()
            .next()
            .expect("the search found something")
            .id
            .clone();
        let _ = app.update(Message::SelectConversation(found.clone()));
        deliver_selected_demo_detail(&mut app);

        // The row is not in the open folder's listing, which used to leave
        // the reader with no actions at all.
        assert_eq!(
            app.mailbox().unwrap().selected_summary().map(|row| &row.id),
            Some(&found)
        );

        let _ = app.update(Message::ApplyAction(MailAction::SetStarred(true)));

        // The results are what is on screen, so they carry the change.
        let mailbox = app.mailbox().unwrap();
        assert_eq!(mailbox.search_results(), Some("offsite"));
        assert!(
            mailbox
                .visible_conversations()
                .find(|row| row.id == found)
                .expect("the row is still listed")
                .starred
        );
    }

    #[test]
    fn archiving_from_the_results_takes_the_row_out_of_them() {
        let mut app = loaded_demo_app();
        deliver_demo_search(&mut app, "the");
        let rows: Vec<String> = app
            .mailbox()
            .unwrap()
            .visible_conversations()
            .map(|row| row.id.clone())
            .collect();
        assert!(rows.len() > 1, "the search found more than one row");
        let _ = app.update(Message::SelectConversation(rows[0].clone()));
        deliver_selected_demo_detail(&mut app);

        let _ = app.update(Message::ApplyAction(MailAction::MoveTo(sys(
            MailFolder::Archive,
        ))));

        // The demo has no server to re-read, so nothing else would have taken
        // the row out of the results, and archiving would look like a no-op.
        let mailbox = app.mailbox().unwrap();
        assert!(
            !mailbox.visible_conversations().any(|row| row.id == rows[0]),
            "the archived row is still listed among the results"
        );
        assert_eq!(mailbox.selected_conversation(), Some(rows[1].as_str()));
    }

    #[test]
    fn demo_archive_removes_selected_and_opens_next() {
        let mut app = loaded_demo_app();
        let _ = app.update(Message::SelectConversation("demo-0".into()));
        deliver_selected_demo_detail(&mut app);

        let _ = app.update(Message::ApplyAction(MailAction::MoveTo(sys(
            MailFolder::Archive,
        ))));

        let mailbox = app.mailbox().unwrap();
        assert!(
            mailbox
                .conversations()
                .iter()
                .all(|conversation| conversation.id != "demo-0")
        );
        assert_eq!(mailbox.selected_conversation(), Some("demo-1"));

        let _ = app.update(Message::SelectFolder(sys(MailFolder::Archive)));
        deliver_latest_demo_page(&mut app, 0);
        assert!(
            app.mailbox()
                .unwrap()
                .conversations()
                .iter()
                .any(|conversation| conversation.id == "demo-0")
        );
    }

    #[test]
    fn moving_selected_conversation_invalidates_its_pending_detail() {
        let mut app = loaded_demo_app();
        let _ = app.update(Message::SelectConversation("demo-0".into()));
        let stale = pending_reader_request(&app);

        let task = app.update(Message::ApplyAction(MailAction::MoveTo(sys(
            MailFolder::Archive,
        ))));
        let current = pending_reader_request(&app);
        let stale_detail = demo_service(&app).conversation_detail("demo-0", NOW);
        let _ = app.update(Message::ConversationLoaded(stale, stale_detail));

        // Opening the next conversation: its detail request and the scroll
        // back to the top of the reader.
        assert_eq!(task.units(), 2);
        assert_eq!(
            app.mailbox().unwrap().selected_conversation(),
            Some("demo-1")
        );
        assert_eq!(pending_reader_request(&app), current);
    }

    #[test]
    fn undoing_a_demo_move_puts_the_conversation_back() {
        let mut app = loaded_demo_app();
        let _ = app.update(Message::SelectConversation("demo-3".into()));
        deliver_selected_demo_detail(&mut app);

        let _ = app.update(Message::ApplyAction(MailAction::MoveTo(sys(
            MailFolder::Archive,
        ))));
        let has_row = |app: &App| {
            app.mailbox()
                .unwrap()
                .conversations()
                .iter()
                .any(|conversation| conversation.id == "demo-3")
        };
        assert!(!has_row(&app));
        assert!(app.mailbox().unwrap().undo().is_some());

        let _ = app.update(Message::UndoMove);

        assert!(has_row(&app));
        assert!(app.mailbox().unwrap().undo().is_none());
    }

    #[test]
    fn demo_trash_and_spam_moves_update_folders() {
        for (message, folder) in [
            (
                Message::ApplyAction(MailAction::MoveTo(sys(MailFolder::Trash))),
                sys(MailFolder::Trash),
            ),
            (
                Message::ApplyAction(MailAction::MoveTo(sys(MailFolder::Spam))),
                sys(MailFolder::Spam),
            ),
        ] {
            let mut app = loaded_demo_app();
            let _ = app.update(Message::SelectConversation("demo-3".into()));
            deliver_selected_demo_detail(&mut app);
            let _ = app.update(message);
            assert!(
                app.mailbox()
                    .unwrap()
                    .conversations()
                    .iter()
                    .all(|conversation| conversation.id != "demo-3")
            );

            let _ = app.update(Message::SelectFolder(folder.clone()));
            deliver_latest_demo_page(&mut app, 0);
            assert!(
                app.mailbox()
                    .unwrap()
                    .conversations()
                    .iter()
                    .any(|conversation| conversation.id == "demo-3")
            );
        }
    }

    #[test]
    fn inbox_search_reflects_folder_moves_immediately() {
        for message in [
            Message::ApplyAction(MailAction::MoveTo(sys(MailFolder::Archive))),
            Message::ApplyAction(MailAction::MoveTo(sys(MailFolder::Trash))),
            Message::ApplyAction(MailAction::MoveTo(sys(MailFolder::Spam))),
        ] {
            let mut app = loaded_demo_app();
            let _ = app.update(Message::SearchChanged("blue notebook".into()));
            assert_eq!(app.mailbox().unwrap().visible_conversations().count(), 1);
            let _ = app.update(Message::SelectConversation("demo-3".into()));
            deliver_selected_demo_detail(&mut app);

            let _ = app.update(message);

            let mailbox = app.mailbox().unwrap();
            assert_eq!(mailbox.search_query(), "blue notebook");
            assert_eq!(mailbox.visible_conversations().count(), 0);
            assert_eq!(mailbox.selected_conversation(), None);
        }
    }

    #[test]
    fn moving_only_conversation_closes_reader_cleanly() {
        let mut app = loaded_demo_app();
        let _ = app.update(Message::SelectConversation("demo-3".into()));
        let _ = app.update(Message::ApplyAction(MailAction::MoveTo(sys(
            MailFolder::Trash,
        ))));
        let _ = app.update(Message::SelectFolder(sys(MailFolder::Trash)));
        deliver_latest_demo_page(&mut app, 0);
        let _ = app.update(Message::SelectConversation("demo-3".into()));

        let _ = app.update(Message::ApplyAction(MailAction::MoveTo(sys(
            MailFolder::Archive,
        ))));

        let mailbox = app.mailbox().unwrap();
        assert!(mailbox.conversations().is_empty());
        assert!(mailbox.reader().is_none());
        assert_eq!(mailbox.selected_conversation(), None);
    }

    #[test]
    fn resizing_panels_only_changes_layout() {
        let (mut app, _) = App::boot(true, Settings::default());
        deliver_demo_page(&mut app, 1, 0);
        let _ = app.update(Message::SelectConversation("demo-0".into()));
        deliver_selected_demo_detail(&mut app);
        let _ = app.update(Message::ToggleMessageExpanded("demo-0-0".into()));
        let requests = app.last_request;
        let window = iced::Size::new(1_100.0, 700.0);
        let widths = |panels: &pane_grid::State<Panel>| {
            panels
                .layout()
                .pane_regions(DIVIDER_WIDTH, MIN_PANEL_WIDTH, window)
        };
        let default_widths = widths(app.panels());
        let splits: Vec<_> = app.panels().layout().splits().copied().collect();

        for (split, ratio) in splits.into_iter().zip([0.35, 0.6]) {
            let task = app.update(Message::PanelResized(pane_grid::ResizeEvent {
                split,
                ratio,
            }));
            assert_eq!(task.units(), 0);
        }

        assert_ne!(widths(app.panels()), default_widths);
        assert_eq!(app.last_request, requests);
        assert!(app.is_demo());
        let mailbox = app.mailbox().unwrap();
        assert_eq!(mailbox.folder(), &sys(MailFolder::Inbox));
        assert_eq!(mailbox.status(), ListStatus::Loaded);
        assert_eq!(mailbox.conversations().len(), 10);
        let reader = mailbox.reader().unwrap();
        assert_eq!(reader.conversation_id(), "demo-0");
        assert!(reader.is_expanded("demo-0-0"));
    }

    #[test]
    fn exiting_demo_clears_mailbox_and_opens_login() {
        let (mut app, _) = App::boot(true, Settings::default());
        deliver_demo_page(&mut app, 1, 0);

        let _ = app.update(Message::Logout);

        assert_eq!(app.auth_state, AuthState::SignedOut);
        assert_eq!(app.auth_error, None);
        assert!(app.backend.is_none());
        assert!(app.mailbox.is_none());
        assert!(!app.is_demo());
    }

    #[test]
    fn absent_saved_session_opens_login() {
        let mut app = App::new(Settings::default());

        let _ = app.update(Message::SessionChecked(ResumeOutcome::SignedOut));

        assert_eq!(app.auth_state, AuthState::SignedOut);
        assert_eq!(app.auth_error, None);
    }

    fn signing_in_app() -> App {
        let mut app = App::new(Settings::default());
        app.auth_state = AuthState::SigningIn(SignInStep::Credentials);
        app.login_form.username = "username sentinel".into();
        app.login_form.password = "password sentinel".into();
        app
    }

    #[test]
    fn totp_prompt_is_answered_within_the_same_sign_in() {
        let mut app = signing_in_app();
        let (reply, mut answer) = crate::mail::Reply::channel();

        let _ = app.update(Message::SignInPrompt(SignInPrompt::Totp(reply)));
        assert_eq!(app.auth_state, AuthState::NeedsTotp);
        assert_eq!(app.login_form.username, "username sentinel");
        assert_eq!(app.login_form.password, "password sentinel");

        let _ = app.update(Message::Submit);
        assert_eq!(app.auth_error, Some(AuthError::TotpRequired));
        assert_eq!(answer.try_recv(), Ok(None));

        let _ = app.update(Message::TotpChanged(" 123456 ".into()));
        let _ = app.update(Message::Submit);
        assert_eq!(answer.try_recv(), Ok(Some("123456".to_owned())));
        assert_eq!(app.auth_state, AuthState::SigningIn(SignInStep::Totp));
        assert!(app.login_form.totp.is_empty());
    }

    #[test]
    fn human_verification_waits_for_confirmation() {
        let mut app = signing_in_app();
        let (done, mut answer) = crate::mail::Reply::channel();
        let url = "https://verify.proton.me/?methods=captcha&token=t".to_owned();

        let _ = app.update(Message::SignInPrompt(SignInPrompt::HumanVerification {
            url: url.clone(),
            done,
        }));
        assert_eq!(app.auth_state, AuthState::NeedsHumanVerification { url });
        assert_eq!(answer.try_recv(), Ok(None));

        let _ = app.update(Message::Submit);
        assert_eq!(answer.try_recv(), Ok(Some(())));
        assert!(matches!(app.auth_state, AuthState::SigningIn(_)));
    }

    #[test]
    fn going_back_cancels_the_pending_prompt() {
        let mut app = signing_in_app();
        let (reply, mut answer) = crate::mail::Reply::<String>::channel();
        let _ = app.update(Message::SignInPrompt(SignInPrompt::Totp(reply)));

        let _ = app.update(Message::CancelChallenge);
        assert_eq!(app.auth_state, AuthState::SignedOut);
        assert!(answer.try_recv().is_err());

        let _ = app.update(Message::SignInFinished(SignInOutcome::Cancelled));
        assert_eq!(app.auth_state, AuthState::SignedOut);
        assert_eq!(app.auth_error, None);
    }

    #[test]
    fn prompts_without_a_running_sign_in_are_cancelled() {
        let mut app = App::new(Settings::default());
        app.auth_state = AuthState::SignedOut;
        let (reply, mut answer) = crate::mail::Reply::<String>::channel();

        let _ = app.update(Message::SignInPrompt(SignInPrompt::Totp(reply)));

        assert_eq!(app.auth_state, AuthState::SignedOut);
        assert!(answer.try_recv().is_err());
    }

    #[test]
    fn terminal_sign_in_failure_clears_secrets() {
        let mut app = App::new(Settings::default());
        app.login_form.password = "password sentinel".into();
        app.login_form.totp = "totp sentinel".into();
        app.login_form.mailbox_password = "mailbox password sentinel".into();

        let _ = app.update(Message::SignInFinished(SignInOutcome::Failed(
            AuthError::Connection,
        )));

        assert_eq!(app.auth_state, AuthState::SignedOut);
        assert!(app.login_form.password.is_empty());
        assert!(app.login_form.totp.is_empty());
        assert!(app.login_form.mailbox_password.is_empty());
    }

    #[test]
    fn failed_logout_restores_authenticated_shell() {
        let mut app = App::new(Settings::default());
        app.auth_state = AuthState::SigningOut;

        let _ = app.update(Message::LogoutFinished(Err(AuthError::SessionUnavailable)));

        assert_eq!(app.auth_state, AuthState::Authenticated { email: None });
        assert_eq!(app.auth_error, Some(AuthError::SessionUnavailable));
    }

    #[test]
    fn successful_logout_opens_login_and_clears_mailbox() {
        let mut app = authenticated_app();
        app.auth_state = AuthState::SigningOut;

        let _ = app.update(Message::LogoutFinished(Ok(())));

        assert_eq!(app.auth_state, AuthState::SignedOut);
        assert!(app.mailbox.is_none());
    }

    #[test]
    fn expired_session_signs_out_and_clears_mailbox() {
        let mut app = authenticated_app();

        let _ = app.update(Message::ConversationsLoaded(
            1,
            Err(MailboxError::SessionExpired),
        ));

        assert_eq!(app.auth_state, AuthState::SignedOut);
        assert_eq!(app.auth_error, Some(AuthError::SessionExpired));
        assert!(app.mailbox.is_none());
    }

    #[test]
    fn stale_expired_session_response_is_ignored() {
        let mut app = authenticated_app();

        let _ = app.update(Message::ConversationsLoaded(
            99,
            Err(MailboxError::SessionExpired),
        ));

        assert_eq!(app.auth_state, AuthState::Authenticated { email: None });
        assert!(app.mailbox.is_some());
    }

    #[test]
    fn logout_result_after_session_expiry_is_ignored() {
        let mut app = authenticated_app();
        app.auth_state = AuthState::SigningOut;
        let _ = app.update(Message::CountsLoaded(2, Err(MailboxError::SessionExpired)));

        let _ = app.update(Message::LogoutFinished(Err(AuthError::Connection)));

        assert_eq!(app.auth_state, AuthState::SignedOut);
        assert_eq!(app.auth_error, Some(AuthError::SessionExpired));
    }

    #[test]
    fn mailbox_actions_are_ignored_while_signing_out() {
        let mut app = authenticated_app();
        app.auth_state = AuthState::SigningOut;

        let _ = app.update(Message::SelectFolder(sys(MailFolder::Trash)));

        assert_eq!(app.mailbox().unwrap().folder(), &sys(MailFolder::Inbox));
    }
}
