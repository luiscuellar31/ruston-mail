mod auth;
mod compose;
mod effect;
mod keys;
mod layout;
mod mailbox;
mod reader;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::downloads::{self, SaveError};
use crate::settings::{ComposePlacement, Panels, Settings, StartFolder, Window};

/// How far the window must change before its new size is worth storing.
const RESIZE_STEP: f32 = 8.0;

use crate::mail::{
    AuthError, BodyFormat, ConversationDetail, ConversationPage, ConversationSummary, Folder,
    MailAction, MailBackend, MailboxCounts, MailboxError, ProtonMailService, ResumeOutcome,
    SendError, SignInOutcome, SignInPrompt, demo::DemoMailbox,
};

use auth::LoginForm;
pub use auth::{AuthState, SignInStep};
pub use compose::{Answering, Compose, ComposeField, ComposeId, Sending};
pub use effect::{Effect, Effects, UiEffect};
pub use keys::{Key, KeyPress};
pub use layout::{ratios as panel_ratios, widths as panel_widths};
pub use mailbox::{
    ActionRequest, ListStatus, Mailbox, ReaderRequest, SearchRequest, Step, UndoMove,
};
use mailbox::{PageRequest, RequestId};
pub use reader::{ConversationReader, ReaderState};

type SessionEpoch = u64;
pub(crate) type AuthAttempt = u64;

#[derive(Clone)]
pub enum Message {
    Submit,
    CancelChallenge,
    SessionChecked(ResumeOutcome),
    SignInFinished(AuthAttempt, SignInOutcome),
    SignInPrompt(AuthAttempt, SignInPrompt),
    OpenVerificationPage,
    CopyVerificationLink,
    LinkClicked(String),
    OpenLink,
    CopyLink,
    DismissLink,
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
    /// Puts the last moved conversation back where it came from.
    UndoMove,
    /// Lets this temporary undo offer expire if it is still current.
    DismissUndo(UndoMove),
    RefreshMailbox,
    /// Periodic foreground refresh. Unlike a manual refresh, this also marks
    /// the other folder caches stale so they catch up when opened.
    AutoRefreshMailbox,
    LoadMoreConversations,
    ConversationsLoaded(RequestId, Result<ConversationPage, MailboxError>),
    ThreadInspected {
        epoch: SessionEpoch,
        folder: Folder,
        conversation_id: String,
        result: Result<Option<Vec<ConversationSummary>>, MailboxError>,
    },
    ConversationLoaded(ReaderRequest, Result<ConversationDetail, MailboxError>),
    CountsLoaded(RequestId, Result<MailboxCounts, MailboxError>),
    NewMailConversationLoaded(Result<ConversationPage, MailboxError>),
    /// The folders the account made, fetched once when the mailbox opens.
    FoldersLoaded(SessionEpoch, Result<Vec<Folder>, MailboxError>),
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
    AttachmentSaved(SessionEpoch, Result<PathBuf, SaveError>),
    /// Reveals a downloaded file in the system file manager.
    RevealAttachment(PathBuf),
    /// Opens or closes the settings window.
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
    /// Starts a new message.
    OpenCompose,
    /// Answers the open message, or passes it on. `everyone` is only read by
    /// a reply.
    Answer {
        message_id: String,
        forward: bool,
        everyone: bool,
    },
    /// Puts an unsent message away. Untouched messages close; written ones ask first.
    CloseCompose,
    /// Discards an unsent message after confirmation.
    DiscardCompose,
    /// Cancels discard confirmation and returns to editing.
    CancelDiscard,
    /// Shows or hides the copy fields.
    ToggleComposeCopies,
    ComposeChanged(ComposeField, String),
    /// Tells the desktop to open the native file dialog for selecting attachments.
    PickComposeAttachments,
    /// Adds picked local files as attachments to the active draft.
    AddComposeAttachments(ComposeId, Vec<PathBuf>),
    /// Removes an attachment by index from the active draft.
    RemoveComposeAttachment(usize),
    /// Hands the message over to be sent.
    Send,
    Sent(SessionEpoch, Result<(), SendError>),
}

pub struct App {
    auth_state: AuthState,
    login_form: LoginForm,
    auth_error: Option<AuthError>,
    /// Changes for every authentication attempt so queued results from an
    /// aborted task cannot affect its replacement.
    auth_attempt: AuthAttempt,
    /// The question a running sign-in is waiting on, if any.
    pending_prompt: Option<SignInPrompt>,
    /// A link clicked in a message, waiting for the user to confirm it.
    pending_link: Option<PendingLink>,
    backend: Option<MailBackend>,
    mailbox: Option<Mailbox>,
    /// Changes whenever an account opens or closes so late async results can
    /// never mutate another session.
    session_epoch: SessionEpoch,
    last_request: RequestId,
    /// The folders the account made, which the sidebar shows below Proton's
    /// own. Empty until they arrive, and in demo mode for good.
    folders: Vec<Folder>,
    /// What the app remembers between runs.
    settings: Settings,
    /// Settings changes and the last revision written to disk.
    /// The UI waits for resizing to settle before calling [`Self::save_settings`].
    settings_revision: u64,
    settings_written: u64,
    /// Whether the settings window is open.
    showing_settings: bool,
    /// The message being written, if there is one.
    compose: Option<Compose>,
    /// Changes for each opened draft so asynchronous attachment pickers from a
    /// closed or discarded draft cannot inject files into a subsequent draft.
    last_compose_id: ComposeId,
    /// What became of the last attachment someone asked to save.
    saved_attachment: Option<Result<PathBuf, SaveError>>,
    /// The attachment in flight, preventing duplicate downloads.
    saving_attachment: Option<String>,
    /// Whether the current refresh was started by background polling.
    is_polling: bool,
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
            auth_attempt: 0,
            pending_prompt: None,
            pending_link: None,
            backend: None,
            mailbox: None,
            session_epoch: 0,
            last_request: 0,
            folders: Vec::new(),
            settings,
            settings_revision: 0,
            settings_written: 0,
            showing_settings: false,
            compose: None,
            last_compose_id: 0,
            saved_attachment: None,
            saving_attachment: None,
            is_polling: false,
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

    /// Fetches an attachment and writes it off the UI thread.
    fn save_attachment(&mut self, message_id: String, attachment_id: String) -> Effects {
        let Some(backend) = self.backend.clone() else {
            return Effects::none();
        };
        if self.saving_attachment.is_some() {
            return Effects::none();
        }
        self.saved_attachment = None;
        self.saving_attachment = Some(attachment_id.clone());
        let epoch = self.session_epoch;

        Effects::perform(
            async move {
                match backend
                    .download_attachment(&message_id, &attachment_id)
                    .await
                {
                    // File I/O is blocking, so keep it off both the interface
                    // thread and Tokio's asynchronous workers.
                    Ok((name, contents)) => {
                        tokio::task::spawn_blocking(move || downloads::save(&name, &contents))
                            .await
                            .unwrap_or(Err(SaveError::Failed))
                    }
                    Err(_) => Err(SaveError::NotFetched),
                }
            },
            move |outcome| Message::AttachmentSaved(epoch, outcome),
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

    /// Writes changed settings best-effort; in-memory changes always remain.
    pub fn save_settings(&mut self) {
        if !self.settings_unsaved() {
            return;
        }
        self.settings.save();
        self.settings_written = self.settings_revision;
    }

    pub fn update(&mut self, message: Message) -> Effects {
        match message {
            Message::Submit => return self.sign_in(),
            Message::CancelChallenge => return self.cancel_sign_in(),
            Message::SessionChecked(outcome) => return self.finish_session_check(outcome),
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
            Message::CopyVerificationLink => {
                if let AuthState::NeedsHumanVerification { url } = &self.auth_state {
                    return Effects::ui(UiEffect::CopyText(url.clone()));
                }
            }
            Message::Logout => return self.logout(),
            Message::LogoutFinished(epoch, result) => self.finish_logout(epoch, result),
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
            Message::ApplyAction(action) => return self.apply_action(action),
            Message::UndoMove => return self.undo_move(),
            Message::DismissUndo(offer) => {
                if let Some(mailbox) = self.active_mailbox() {
                    mailbox.dismiss_undo(&offer);
                }
            }
            Message::RefreshMailbox => {
                self.is_polling = false;
                return self.refresh_mailbox();
            }
            Message::AutoRefreshMailbox => return self.auto_refresh_mailbox(),
            Message::LoadMoreConversations => return self.load_more_conversations(),
            Message::ConversationsLoaded(request, result) => {
                let candidates = match &result {
                    Ok(page) => page.inspect_candidates.clone(),
                    Err(_) => Vec::new(),
                };
                let error = self
                    .mailbox
                    .as_mut()
                    .and_then(|mailbox| mailbox.finish_page(request, result));
                self.handle_mailbox_error(error);
                if error.is_none() {
                    let revalidating = self
                        .mailbox
                        .as_ref()
                        .is_some_and(Mailbox::needs_revalidation);
                    if revalidating {
                        return self.refresh_mailbox();
                    }
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
                folder,
                conversation_id,
                result,
            } => {
                if !self.is_current_session(epoch) {
                    return Effects::none();
                }
                match result {
                    Ok(Some(split_rows)) => {
                        if let Some(mailbox) =
                            self.mailbox.as_mut().filter(|m| m.folder() == &folder)
                        {
                            mailbox.split_conversation(&conversation_id, split_rows);
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        self.handle_mailbox_error(Some(error));
                    }
                }
            }
            Message::ConversationLoaded(request, result) => {
                let error = self
                    .mailbox
                    .as_mut()
                    .and_then(|mailbox| mailbox.finish_conversation(&request, result));
                self.handle_mailbox_error(error);
                return self.mark_opened_read();
            }
            Message::FoldersLoaded(epoch, result) => {
                if !self.is_current_session(epoch) {
                    return Effects::none();
                }
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
                let old_inbox_unread = self
                    .mailbox
                    .as_ref()
                    .and_then(|mailbox| mailbox.counts())
                    .and_then(|counts| counts.unread(&Folder::INBOX));
                let was_polling = self.is_polling;
                self.is_polling = false;

                let error = self
                    .mailbox
                    .as_mut()
                    .and_then(|mailbox| mailbox.finish_counts(request, result));
                self.handle_mailbox_error(error);

                let new_inbox_unread = self
                    .mailbox
                    .as_ref()
                    .and_then(|mailbox| mailbox.counts())
                    .and_then(|counts| counts.unread(&Folder::INBOX));

                if was_polling
                    && self.settings.desktop_notifications
                    && let (Some(old), Some(new)) = (old_inbox_unread, new_inbox_unread)
                    && new > old
                {
                    return self.fetch_new_mail_notification();
                }
            }
            Message::NewMailConversationLoaded(result) => {
                if let Ok(page) = result
                    && let Some(conversation) = page.conversations.first()
                {
                    let sender = conversation
                        .correspondents
                        .as_deref()
                        .filter(|c| !c.is_empty())
                        .or_else(|| {
                            conversation
                                .participants
                                .first()
                                .and_then(|addr| addr.display_name())
                        })
                        .unwrap_or("Proton Mail")
                        .to_owned();
                    let subject = conversation
                        .subject
                        .as_deref()
                        .filter(|s| !s.trim().is_empty())
                        .unwrap_or("(No subject)")
                        .to_owned();
                    return Effects::ui(UiEffect::NotifyNewMail { sender, subject });
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
            Message::AttachmentSaved(epoch, outcome) => {
                if !self.is_current_session(epoch) {
                    return Effects::none();
                }
                self.saving_attachment = None;
                // A save that landed after the session ended has nobody left
                // to tell, and its news must not surface in the next one.
                if self.mailbox.is_some() {
                    self.saved_attachment = Some(outcome);
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
            Message::SetShowUnreadBadge(on) => {
                self.remember(|settings| settings.show_unread_badge = on);
            }
            Message::SetDesktopNotifications(on) => {
                self.remember(|settings| settings.desktop_notifications = on);
            }
            Message::SetStartFolder(start) => self.remember(|settings| settings.start = start),
            Message::SetComposeFormat(format) => {
                self.remember(|settings| settings.compose_format = format);
            }
            Message::SetComposePlacement(placement) => {
                self.remember(|settings| settings.compose_placement = placement);
            }
            Message::OpenCompose => self.open_compose(),
            Message::Answer {
                message_id,
                forward,
                everyone,
            } => self.answer(&message_id, forward, everyone),
            Message::CloseCompose => self.close_compose(),
            Message::DiscardCompose => self.discard_compose(),
            Message::CancelDiscard => self.cancel_discard(),
            Message::ToggleComposeCopies => self.toggle_compose_copies(),
            Message::ComposeChanged(field, value) => self.change_compose(field, value),
            Message::PickComposeAttachments => return self.pick_compose_attachments(),
            Message::AddComposeAttachments(id, paths) => self.add_compose_attachments(id, paths),
            Message::RemoveComposeAttachment(index) => self.remove_compose_attachment(index),
            Message::Send => return self.send_compose(),
            Message::Sent(epoch, result) => return self.finish_send(epoch, result),
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

    /// Whether a background-triggered list request can start without racing
    /// another list-changing operation.
    pub fn auto_refresh_available(&self) -> bool {
        matches!(self.auth_state, AuthState::Authenticated { .. })
            && self.backend.is_some()
            && self
                .mailbox
                .as_ref()
                .is_some_and(Mailbox::auto_refresh_available)
            && !self.compose.as_ref().is_some_and(Compose::in_flight)
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

        // Reload only when undo returns a missing row to the visible folder.
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

        Effects::batch([self.fetch_page(page), self.fetch_counts(counts)])
    }

    fn auto_refresh_mailbox(&mut self) -> Effects {
        if !self.auto_refresh_available() {
            return Effects::none();
        }
        self.is_polling = true;
        if let Some(mailbox) = &mut self.mailbox {
            mailbox.invalidate_cached_listings();
        }

        self.refresh_mailbox()
    }

    fn load_more_conversations(&mut self) -> Effects {
        let request = self.next_request();
        let Some(mailbox) = self.active_mailbox() else {
            return Effects::none();
        };
        if mailbox.search_results().is_some() {
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

    fn inspect_candidates(&self, candidates: Vec<String>) -> Effects {
        let (Some(backend), Some(mailbox)) = (self.backend.clone(), self.mailbox.as_ref()) else {
            return Effects::none();
        };
        let epoch = self.session_epoch;
        let folder = mailbox.folder().clone();

        Effects::batch(candidates.into_iter().map(|id| {
            let backend = backend.clone();
            let target_folder = folder.clone();
            let callback_folder = folder.clone();
            let conversation_id = id.clone();
            Effects::perform(
                async move {
                    backend
                        .inspect_conversation(&conversation_id, &target_folder)
                        .await
                },
                move |result| Message::ThreadInspected {
                    epoch,
                    folder: callback_folder,
                    conversation_id: id,
                    result,
                },
            )
        }))
    }

    /// Asks once for the folders the account made. They change rarely, so
    /// nothing re-fetches them until the mailbox is opened again.
    fn fetch_folders(&self) -> Effects {
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

    fn fetch_new_mail_notification(&self) -> Effects {
        let Some(backend) = self.backend.clone() else {
            return Effects::none();
        };

        Effects::perform(
            async move { backend.list_conversations(&Folder::INBOX, 0, 1).await },
            Message::NewMailConversationLoaded,
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

fn reveal_in_file_manager(path: PathBuf) -> Effects {
    Effects::background(async move {
        let _ = reveal_file(&path);
    })
}

/// Reveals `path` in the system file manager without going through a shell.
fn reveal_file(path: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = std::process::Command::new("open");
        command.arg("-R").arg(path);
        command
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = std::process::Command::new("explorer");
        command.arg(format!("/select,{}", path.display()));
        command
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut command = {
        let parent = path.parent().unwrap_or(path);
        let mut command = std::process::Command::new("xdg-open");
        command.arg(parent);
        command
    };

    command.spawn().map(drop)
}

#[cfg(test)]
mod tests;
