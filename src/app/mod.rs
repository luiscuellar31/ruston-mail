mod layout;
mod mailbox;
mod reader;

use std::sync::Arc;

use iced::Task;
use iced::widget::pane_grid;

use crate::mail::{
    AuthError, ConversationDetail, ConversationPage, LoginRequest, MailBackend, MailFolder,
    MailboxCounts, MailboxError, ProtonMailService, ResumeOutcome, SignInOutcome,
    demo::DemoMailbox,
};

pub use layout::{DIVIDER_GRAB, DIVIDER_WIDTH, MIN_PANEL_WIDTH, Panel};
pub use mailbox::{ListStatus, Mailbox, ReaderRequest};
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
    Authenticated { email: Option<String> },
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
    Logout,
    LogoutFinished(Result<(), AuthError>),
    SelectFolder(MailFolder),
    SelectConversation(String),
    RetryConversation,
    SearchChanged(String),
    ToggleMessageExpanded(String),
    ArchiveSelected,
    MoveSelectedToSpam,
    MoveSelectedToTrash,
    MarkSelectedRead,
    MarkSelectedUnread,
    StarSelected,
    UnstarSelected,
    RefreshMailbox,
    LoadMoreConversations,
    ConversationsLoaded(RequestId, Result<ConversationPage, MailboxError>),
    ConversationLoaded(ReaderRequest, Result<ConversationDetail, MailboxError>),
    CountsLoaded(RequestId, Result<MailboxCounts, MailboxError>),
    PanelResized(pane_grid::ResizeEvent),
}

#[derive(Clone, Copy)]
enum DemoAction {
    Archive,
    MoveToSpam,
    MoveToTrash,
    SetUnread(bool),
    SetStarred(bool),
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
    backend: Option<MailBackend>,
    mailbox: Option<Mailbox>,
    last_request: RequestId,
    /// Mailbox panel sizes. Pure layout state: kept for the life of the app
    /// and never touched by mailbox or authentication changes.
    panels: pane_grid::State<Panel>,
}

impl App {
    /// In demo mode the app opens a local fictional mailbox and never starts
    /// the Proton session resume.
    pub fn boot(demo: bool) -> (Self, Task<Message>) {
        let mut app = Self::new();
        let task = if demo {
            app.open_mailbox(MailBackend::demo(), None)
        } else {
            Task::perform(ProtonMailService::resume(), Message::SessionChecked)
        };

        (app, task)
    }

    fn new() -> Self {
        Self {
            auth_state: AuthState::CheckingSession,
            login_form: LoginForm::default(),
            auth_error: None,
            backend: None,
            mailbox: None,
            last_request: 0,
            panels: layout::default_panels(),
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
                    self.login_form.clear_sensitive();
                    self.auth_error = None;
                    self.auth_state = AuthState::SignedOut;
                }
            }
            Message::SessionChecked(outcome) => return self.finish_session_check(outcome),
            Message::SignInFinished(outcome) => return self.finish_sign_in(outcome),
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
            Message::ToggleMessageExpanded(id) => {
                if let Some(mailbox) = self.active_mailbox() {
                    mailbox.toggle_message(&id);
                }
            }
            Message::ArchiveSelected => return self.apply_demo_action(DemoAction::Archive),
            Message::MoveSelectedToSpam => {
                return self.apply_demo_action(DemoAction::MoveToSpam);
            }
            Message::MoveSelectedToTrash => {
                return self.apply_demo_action(DemoAction::MoveToTrash);
            }
            Message::MarkSelectedRead => {
                return self.apply_demo_action(DemoAction::SetUnread(false));
            }
            Message::MarkSelectedUnread => {
                return self.apply_demo_action(DemoAction::SetUnread(true));
            }
            Message::StarSelected => {
                return self.apply_demo_action(DemoAction::SetStarred(true));
            }
            Message::UnstarSelected => {
                return self.apply_demo_action(DemoAction::SetStarred(false));
            }
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
            }
            Message::CountsLoaded(request, result) => {
                let error = self
                    .mailbox
                    .as_mut()
                    .and_then(|mailbox| mailbox.finish_counts(request, result));
                self.handle_mailbox_error(error);
            }
            Message::PanelResized(event) => self.panels.resize(event.split, event.ratio),
        }

        Task::none()
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
        self.is_demo()
    }

    pub fn panels(&self) -> &pane_grid::State<Panel> {
        &self.panels
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
            optional_unmodified(&self.login_form.mailbox_password),
        );
        self.auth_error = None;
        self.auth_state = AuthState::SigningIn(step);

        Task::perform(ProtonMailService::sign_in(request), Message::SignInFinished)
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
            SignInOutcome::NeedsTotp => {
                self.login_form.totp.clear();
                self.auth_error = None;
                self.auth_state = AuthState::NeedsTotp;
            }
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
        let (mailbox, page) = Mailbox::open(page_size, page_request, counts_request);
        self.mailbox = Some(mailbox);

        Task::batch([
            self.fetch_page(Some(page)),
            self.fetch_counts(Some(counts_request)),
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
    fn close_mailbox(&mut self, error: Option<AuthError>) {
        self.backend = None;
        self.mailbox = None;
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
        if let Some(MailBackend::Demo(service)) = self.backend.clone() {
            if !service.set_unread(&id, false) {
                return Task::none();
            }
            self.apply_demo_snapshot(&service);
        }

        self.start_conversation_load(id)
    }

    fn apply_demo_action(&mut self, action: DemoAction) -> Task<Message> {
        let Some(MailBackend::Demo(service)) = self.backend.clone() else {
            return Task::none();
        };
        let Some(mailbox) = self.mailbox.as_ref() else {
            return Task::none();
        };
        if !matches!(self.auth_state, AuthState::Authenticated { .. }) || mailbox.is_busy() {
            return Task::none();
        }
        let Some(id) = mailbox.selected_conversation().map(str::to_owned) else {
            return Task::none();
        };

        let applied = match action {
            DemoAction::Archive => service.archive(&id),
            DemoAction::MoveToSpam => service.move_to_spam(&id),
            DemoAction::MoveToTrash => service.move_to_trash(&id),
            DemoAction::SetUnread(unread) => service.set_unread(&id, unread),
            DemoAction::SetStarred(starred) => service.set_starred(&id, starred),
        };
        if applied && let Some(next) = self.apply_demo_snapshot(&service) {
            return self.select_conversation(next);
        }

        Task::none()
    }

    fn start_conversation_load(&mut self, id: String) -> Task<Message> {
        let request = self.next_request();
        let request = self
            .active_mailbox()
            .and_then(|mailbox| mailbox.start_conversation_load(id, request));

        self.fetch_conversation(request)
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
        let folder = mailbox.folder();
        let page_size = mailbox.loaded_conversation_limit();
        let (page, counts) = service.snapshot(folder, page_size, crate::mail::demo::now());

        self.active_mailbox()?.apply_action_snapshot(page, counts)
    }

    fn select_folder(&mut self, folder: MailFolder) -> Task<Message> {
        let request = self.next_request();
        let page = self
            .active_mailbox()
            .and_then(|mailbox| mailbox.select_folder(folder, request));

        self.fetch_page(page)
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

        Task::perform(
            async move {
                backend
                    .list_conversations(request.folder, request.page, request.page_size)
                    .await
            },
            move |result| Message::ConversationsLoaded(request.id, result),
        )
    }

    fn fetch_counts(&self, request: Option<RequestId>) -> Task<Message> {
        let (Some(request), Some(backend)) = (request, self.backend.clone()) else {
            return Task::none();
        };

        Task::perform(
            async move { backend.conversation_counts().await },
            move |result| Message::CountsLoaded(request, result),
        )
    }

    fn fetch_conversation(&self, request: Option<ReaderRequest>) -> Task<Message> {
        let (Some(request), Some(backend)) = (request, self.backend.clone()) else {
            return Task::none();
        };
        let conversation_id = request.conversation_id.clone();

        Task::perform(
            async move { backend.conversation_detail(&conversation_id).await },
            move |result| Message::ConversationLoaded(request.clone(), result),
        )
    }

    fn handle_mailbox_error(&mut self, error: Option<MailboxError>) {
        if error == Some(MailboxError::SessionExpired) {
            self.close_mailbox(Some(AuthError::SessionExpired));
        }
    }
}

fn optional_trimmed(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn optional_unmodified(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail::demo;

    const NOW: i64 = 1_789_000_000;

    fn authenticated_app() -> App {
        let mut app = App::new();
        app.auth_state = AuthState::Authenticated { email: None };
        app.mailbox = Some(Mailbox::open(50, 1, 2).0);
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
            },
            _ => panic!("expected pending reader request"),
        }
    }

    fn deliver_selected_demo_detail(app: &mut App) {
        let request = pending_reader_request(app);
        let result = demo_service(app).conversation_detail(&request.conversation_id, NOW);
        let _ = app.update(Message::ConversationLoaded(request, result));
    }

    fn loaded_demo_app() -> App {
        let (mut app, _) = App::boot(true);
        deliver_demo_page(&mut app, 1, 0);
        let counts = demo_service(&app).counts();
        let _ = app.update(Message::CountsLoaded(2, Ok(counts)));
        app
    }

    #[test]
    fn initial_state_checks_saved_session() {
        let app = App::new();

        assert_eq!(app.auth_state, AuthState::CheckingSession);
    }

    #[test]
    fn normal_startup_checks_the_proton_session() {
        let (app, _) = App::boot(false);

        assert_eq!(app.auth_state, AuthState::CheckingSession);
        assert!(!app.is_demo());
        assert!(app.backend.is_none());
        assert!(app.mailbox.is_none());
    }

    #[test]
    fn demo_startup_opens_mailbox_without_session_resume() {
        let (app, _) = App::boot(true);

        assert!(app.is_demo());
        assert_eq!(app.auth_state, AuthState::Authenticated { email: None });
        let mailbox = app.mailbox().unwrap();
        assert_eq!(mailbox.folder(), MailFolder::Inbox);
        assert_eq!(mailbox.status(), ListStatus::Loading(1));
    }

    #[test]
    fn demo_mailbox_uses_production_pagination() {
        let (mut app, _) = App::boot(true);
        deliver_demo_page(&mut app, 1, 0);
        let counts = demo_service(&app).counts();
        let _ = app.update(Message::CountsLoaded(2, Ok(counts)));

        let mailbox = app.mailbox().unwrap();
        assert_eq!(mailbox.conversations().len(), 10);
        assert!(mailbox.has_more());
        assert_eq!(mailbox.counts().unwrap().unread(MailFolder::Inbox), Some(6));

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
        let (mut app, _) = App::boot(true);

        let _ = app.update(Message::SelectFolder(MailFolder::Trash));
        deliver_latest_demo_page(&mut app, 0);
        let mailbox = app.mailbox().unwrap();
        assert_eq!(mailbox.status(), ListStatus::Loaded);
        assert!(mailbox.conversations().is_empty());

        let _ = app.update(Message::SelectFolder(MailFolder::Spam));
        deliver_latest_demo_page(&mut app, 0);
        let mailbox = app.mailbox().unwrap();
        assert_eq!(mailbox.status(), ListStatus::Loaded);
        assert_eq!(mailbox.conversations().len(), 2);
        assert!(app.is_demo());
    }

    #[test]
    fn demo_selection_is_local() {
        let (mut app, _) = App::boot(true);
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

        assert_eq!(task.units(), 1);
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
        let (mut app, _) = App::boot(true);
        deliver_demo_page(&mut app, 1, 0);

        assert_eq!(app.mailbox().unwrap().reader_state(), &ReaderState::Empty);
    }

    #[test]
    fn demo_selection_opens_reader_with_newest_message_expanded() {
        let (mut app, _) = App::boot(true);
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
        let (mut app, _) = App::boot(true);
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
        let (mut app, _) = App::boot(true);
        deliver_demo_page(&mut app, 1, 0);
        let _ = app.update(Message::SelectConversation("demo-0".into()));

        let _ = app.update(Message::SelectFolder(MailFolder::Sent));

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
                .unread(MailFolder::Inbox),
            Some(5)
        );

        let _ = app.update(Message::MarkSelectedUnread);
        let _ = app.update(Message::MarkSelectedUnread);
        assert_eq!(app.mailbox().unwrap().search_query(), "design review");
        assert_eq!(app.mailbox().unwrap().visible_conversations().count(), 1);
        assert!(app.mailbox().unwrap().conversations()[0].unread);
        assert_eq!(
            app.mailbox()
                .unwrap()
                .counts()
                .unwrap()
                .unread(MailFolder::Inbox),
            Some(6)
        );

        let _ = app.update(Message::MarkSelectedRead);
        let _ = app.update(Message::MarkSelectedRead);
        let _ = app.update(Message::StarSelected);
        assert_eq!(app.mailbox().unwrap().reader().unwrap().detail(), &detail);
        assert!(!app.mailbox().unwrap().conversations()[0].unread);
        assert_eq!(
            app.mailbox()
                .unwrap()
                .counts()
                .unwrap()
                .unread(MailFolder::Inbox),
            Some(5)
        );
    }

    #[test]
    fn demo_star_actions_update_starred_folder_and_selection() {
        let mut app = loaded_demo_app();
        let _ = app.update(Message::SelectConversation("demo-1".into()));
        deliver_selected_demo_detail(&mut app);
        let _ = app.update(Message::StarSelected);
        let _ = app.update(Message::StarSelected);
        assert!(app.mailbox().unwrap().conversations()[1].starred);

        let _ = app.update(Message::SelectFolder(MailFolder::Starred));
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
        let _ = app.update(Message::UnstarSelected);
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
        let _ = app.update(Message::SelectFolder(MailFolder::Starred));
        deliver_latest_demo_page(&mut app, 0);
        let _ = app.update(Message::SearchChanged("offsite".into()));
        let _ = app.update(Message::SelectConversation("demo-2".into()));

        let _ = app.update(Message::UnstarSelected);

        let mailbox = app.mailbox().unwrap();
        assert_eq!(mailbox.search_query(), "offsite");
        assert_eq!(mailbox.visible_conversations().count(), 0);
        assert_eq!(mailbox.selected_conversation(), None);
    }

    #[test]
    fn demo_archive_removes_selected_and_opens_next() {
        let mut app = loaded_demo_app();
        let _ = app.update(Message::SelectConversation("demo-0".into()));
        deliver_selected_demo_detail(&mut app);

        let _ = app.update(Message::ArchiveSelected);

        let mailbox = app.mailbox().unwrap();
        assert!(
            mailbox
                .conversations()
                .iter()
                .all(|conversation| conversation.id != "demo-0")
        );
        assert_eq!(mailbox.selected_conversation(), Some("demo-1"));

        let _ = app.update(Message::SelectFolder(MailFolder::Archive));
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

        let task = app.update(Message::ArchiveSelected);
        let current = pending_reader_request(&app);
        let stale_detail = demo_service(&app).conversation_detail("demo-0", NOW);
        let _ = app.update(Message::ConversationLoaded(stale, stale_detail));

        assert_eq!(task.units(), 1);
        assert_eq!(
            app.mailbox().unwrap().selected_conversation(),
            Some("demo-1")
        );
        assert_eq!(pending_reader_request(&app), current);
    }

    #[test]
    fn demo_trash_and_spam_moves_update_folders() {
        for (message, folder) in [
            (Message::MoveSelectedToTrash, MailFolder::Trash),
            (Message::MoveSelectedToSpam, MailFolder::Spam),
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

            let _ = app.update(Message::SelectFolder(folder));
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
            Message::ArchiveSelected,
            Message::MoveSelectedToTrash,
            Message::MoveSelectedToSpam,
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
        let _ = app.update(Message::MoveSelectedToTrash);
        let _ = app.update(Message::SelectFolder(MailFolder::Trash));
        deliver_latest_demo_page(&mut app, 0);
        let _ = app.update(Message::SelectConversation("demo-3".into()));

        let _ = app.update(Message::ArchiveSelected);

        let mailbox = app.mailbox().unwrap();
        assert!(mailbox.conversations().is_empty());
        assert!(mailbox.reader().is_none());
        assert_eq!(mailbox.selected_conversation(), None);
    }

    #[test]
    fn resizing_panels_only_changes_layout() {
        let (mut app, _) = App::boot(true);
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
        assert_eq!(mailbox.folder(), MailFolder::Inbox);
        assert_eq!(mailbox.status(), ListStatus::Loaded);
        assert_eq!(mailbox.conversations().len(), 10);
        let reader = mailbox.reader().unwrap();
        assert_eq!(reader.conversation_id(), "demo-0");
        assert!(reader.is_expanded("demo-0-0"));
    }

    #[test]
    fn exiting_demo_clears_mailbox_and_opens_login() {
        let (mut app, _) = App::boot(true);
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
        let mut app = App::new();

        let _ = app.update(Message::SessionChecked(ResumeOutcome::SignedOut));

        assert_eq!(app.auth_state, AuthState::SignedOut);
        assert_eq!(app.auth_error, None);
    }

    #[test]
    fn totp_requirement_opens_totp_step() {
        let mut app = App::new();
        app.login_form.username = "username sentinel".into();
        app.login_form.password = "password sentinel".into();
        app.login_form.totp = "totp sentinel".into();

        let _ = app.update(Message::SignInFinished(SignInOutcome::NeedsTotp));

        assert_eq!(app.auth_state, AuthState::NeedsTotp);
        assert_eq!(app.login_form.username, "username sentinel");
        assert_eq!(app.login_form.password, "password sentinel");
        assert!(app.login_form.totp.is_empty());
    }

    #[test]
    fn terminal_sign_in_failure_clears_secrets() {
        let mut app = App::new();
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
        let mut app = App::new();
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

        let _ = app.update(Message::SelectFolder(MailFolder::Trash));

        assert_eq!(app.mailbox().unwrap().folder(), MailFolder::Inbox);
    }
}
