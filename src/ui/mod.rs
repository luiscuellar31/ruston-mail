mod auto_refresh;
mod close;
mod compose;
mod dialogs;
mod fonts;
mod graphics;
mod identity;
#[cfg(any(target_os = "linux", test))]
mod launcher;
mod login;
#[cfg(target_os = "macos")]
mod macos;
mod mailbox;
mod notifications;
mod reader;
mod settings;
#[cfg(target_os = "windows")]
mod taskbar;
#[cfg(test)]
mod test_support;
mod theme;
mod window_place;

use eframe::egui;

use crate::app::{App, AuthState, Effects, Key, KeyPress, Message, Notice, UiEffect, UndoMove};
use crate::mail::Folder;
use crate::runtime::Runtime;
use crate::settings::{ComposePlacement, MIN_WINDOW_SIZE, Settings, Window};
use auto_refresh::AutoRefresh;

/// Debounces settings writes while a window or divider is dragged.
const SETTINGS_QUIET: f32 = 0.75;
const LAYOUT_RESET_WAIT: f64 = 2.0;

#[derive(Default)]
pub(super) struct UiState {
    /// Password reveal is local to the current sign-in interaction.
    show_password: bool,
    focus_search: bool,
    scroll_reader_top: bool,
    /// Set when another conversation opens: once its messages show, the
    /// reader brings the first unread one, or the newest, into view.
    land_reader: bool,
    reveal_conversation: Option<String>,
    conversation_scroll: Option<mailbox::ConversationScroll>,
    compact_view: mailbox::CompactView,
    bottom_notice: Option<TimedNotice>,
    notification_status: notifications::Status,
}

/// What the notice at the foot of the mailbox says: the offer to take back
/// the last move, or a confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
enum BottomNotice {
    Undo(Box<UndoMove>),
    Confirmation(Notice),
}

/// The bottom notice on screen, and when it goes.
struct TimedNotice {
    notice: BottomNotice,
    expires_at: f64,
}

pub(crate) fn main_viewport(settings: &Settings) -> egui::ViewportBuilder {
    let viewport = identity::viewport()
        .with_inner_size([settings.window.width, settings.window.height])
        .with_min_inner_size(MIN_WINDOW_SIZE);

    #[cfg(target_os = "macos")]
    let viewport = viewport
        .with_fullsize_content_view(true)
        .with_titlebar_shown(false)
        .with_title_shown(false);

    viewport
}

pub fn run(demo: bool, activation: crate::launch::Instance) -> eframe::Result {
    #[cfg(target_os = "windows")]
    identity::set_process_app_id()?;

    // The app takes the activation once its window draws. A renderer that
    // fails before then leaves it for the next one.
    let activation = std::cell::Cell::new(Some(activation));
    let result = open(demo, &activation, graphics::preferred_renderer());
    #[cfg(target_os = "windows")]
    if let Err(eframe::Error::Wgpu(error)) = &result
        && let Some(instance) = activation.take()
    {
        eprintln!("Ruston Mail: DirectX 12 cannot draw the window ({error}); using OpenGL");
        activation.set(Some(instance));
        return open(demo, &activation, eframe::Renderer::Glow);
    }
    result
}

fn open(
    demo: bool,
    activation: &std::cell::Cell<Option<crate::launch::Instance>>,
    renderer: eframe::Renderer,
) -> eframe::Result {
    let settings = Settings::load(demo);
    let options = eframe::NativeOptions {
        viewport: main_viewport(&settings),
        renderer,
        #[cfg(target_os = "windows")]
        wgpu_options: graphics::wgpu_options(),
        ..Default::default()
    };

    eframe::run_native(
        "Ruston Mail",
        options,
        Box::new(move |creation| {
            let mut desktop = DesktopApp::new(creation, demo, settings);
            if let Some(activation) = activation.take() {
                activation.attach(&creation.egui_ctx);
                desktop.activation = Some(activation);
            }
            Ok(Box::new(desktop))
        }),
    )
}

struct DesktopApp {
    app: App,
    runtime: Runtime,
    ui: UiState,
    /// Whether the process started as the demo. It stays so after Exit demo,
    /// while `app.is_demo()` follows the backend that is open.
    demo_process: bool,
    title: String,
    unread_badge: Option<u32>,
    auto_refresh: AutoRefresh,
    /// Preferences being edited in the settings pane until Apply is pressed.
    settings_draft: Option<Settings>,
    /// Where to go after the user decides what to do with unsaved preferences.
    pending_settings_exit: Option<SettingsExit>,
    /// Set while the root window asks whether to discard a written message,
    /// and what discarding it makes way for.
    pending_discard: Option<close::Leaving>,
    window_close_approved: bool,
    /// Holds a close or a sign-out asked for while changes were still on
    /// their way to Proton, or while a sign-out was under way.
    deferred_leave: close::DeferredLeave,
    /// Revision and time used to debounce settings writes.
    settings_seen: u64,
    settings_seen_at: f64,
    /// Last window size dispatched, avoiding busy-loop repainting when window size is unchanged.
    last_window_size: Option<Window>,
    /// Waits for the requested window size before resetting egui's cached pane widths.
    layout_reset_at: Option<f64>,
    /// The main window: the parent of native dialogs, retained until each
    /// completes, including portal export, the window whose place is
    /// remembered, and on Windows the taskbar button with the unread count.
    main_window: Option<std::sync::Arc<winit::window::Window>>,
    /// Native dialog completion, including cancellation, releases this slot.
    file_dialog_open: bool,
    notifications: notifications::Notifications,
    activation: Option<crate::launch::Instance>,
    focus_after_activation: bool,
    /// The draft the editors last showed, so their memory can go with it.
    shown_draft: Option<crate::app::ComposeId>,
    /// Edit menu commands for the composer's own window, which only macOS
    /// sends. They reach its text as input on the window's next pass.
    composer_edits: Vec<egui::Event>,
    /// Keep each window's title bar in the app's theme on Windows.
    root_title_bar: theme::TitleBar,
    composer_title_bar: theme::TitleBar,
    #[cfg(target_os = "macos")]
    native_menu: Option<macos::Menu>,
}

enum SettingsExit {
    Navigate(Box<Message>),
    CloseWindow,
}

impl DesktopApp {
    fn new(creation: &eframe::CreationContext<'_>, demo: bool, settings: Settings) -> Self {
        let mut app = Self::with_context(&creation.egui_ctx, demo, settings);
        app.main_window = creation.winit_window().cloned();
        if let Some(window) = &app.main_window {
            window_place::restore(window, app.app.settings().window);
        }
        #[cfg(target_os = "macos")]
        {
            app.native_menu = macos::Menu::install(&creation.egui_ctx);
        }
        app
    }

    fn with_context(context: &egui::Context, demo: bool, settings: Settings) -> Self {
        theme::install(context);
        apply_appearance(context, &settings);
        fonts::apply(context, fonts::Typeface::chosen(settings.system_font));
        let runtime = Runtime::new().expect("failed to start background runtime");
        runtime.attach(context);
        let (app, effects) = App::boot(demo, settings);
        let mut desktop = Self {
            app,
            runtime,
            ui: UiState::default(),
            demo_process: demo,
            title: String::new(),
            unread_badge: None,
            auto_refresh: AutoRefresh::default(),
            settings_draft: None,
            pending_settings_exit: None,
            pending_discard: None,
            window_close_approved: false,
            deferred_leave: close::DeferredLeave::default(),
            settings_seen: 0,
            settings_seen_at: 0.0,
            last_window_size: None,
            layout_reset_at: None,
            main_window: None,
            file_dialog_open: false,
            notifications: notifications::Notifications::default(),
            activation: None,
            focus_after_activation: false,
            shown_draft: None,
            composer_edits: Vec::new(),
            root_title_bar: theme::TitleBar::default(),
            composer_title_bar: theme::TitleBar::default(),
            #[cfg(target_os = "macos")]
            native_menu: None,
        };
        desktop.execute(effects, context);
        desktop
    }

    /// Once a draft is sent, discarded or left behind by signing out, drops
    /// its editors' undo history, which still holds what was typed.
    fn forget_finished_draft(&mut self, context: &egui::Context) {
        let current = self.app.compose().map(crate::app::Compose::id);
        if self.shown_draft != current {
            if let Some(finished) = self.shown_draft {
                compose::forget_draft(context, finished);
            }
            self.shown_draft = current;
        }
    }

    fn dispatch(&mut self, message: Message, context: &egui::Context) {
        if matches!(
            &message,
            Message::AddComposeAttachments(_, _) | Message::AttachmentDestinationChosen(_, _)
        ) {
            self.file_dialog_open = false;
        }
        if self.file_dialog_open
            && matches!(
                &message,
                Message::PickComposeAttachments | Message::SaveAttachmentAs(_, _, _)
            )
        {
            return;
        }
        let message = if self.app.showing_settings() {
            match message {
                Message::KeyPressed(press)
                    if (press.key == Key::Escape && !press.command && !press.other_modifier)
                        || (press.key == Key::Character(',') && press.command) =>
                {
                    Message::ShowSettings(false)
                }
                other => other,
            }
        } else {
            message
        };
        if self.app.showing_settings()
            && matches!(
                message,
                Message::ShowSettings(false)
                    | Message::SelectFolder(_)
                    | Message::OpenCompose
                    | Message::Logout
            )
        {
            if self.pending_settings_exit.is_some() {
                return;
            }
            if self.has_unapplied_settings() {
                self.pending_settings_exit = Some(SettingsExit::Navigate(Box::new(message)));
                context.request_repaint();
                return;
            }
        }
        if matches!(message, Message::Logout) {
            let leaving = self.sign_out_leaving();
            // Signing out discards a written message only once the user says so.
            if self
                .app
                .compose()
                .is_some_and(|draft| !draft.in_flight() && !draft.is_untouched())
            {
                self.pending_discard = Some(leaving);
                context.request_repaint();
                return;
            }
            // Signing out drops what has not reached Proton yet, so it waits.
            if self
                .deferred_leave
                .defer(leaving, self.holding(leaving).is_some())
            {
                context.request_repaint();
                return;
            }
        }
        // The reset click was drawn with the previous pane widths.
        if self.layout_reset_at.is_some() && matches!(message, Message::PanelsResized(_)) {
            return;
        }

        let now = context.input(|input| input.time);
        let submitted_or_cancelled = matches!(&message, Message::Submit | Message::CancelChallenge);
        if matches!(&message, Message::RefreshMailbox) {
            self.auto_refresh.postpone(now);
        }
        let page = match &message {
            Message::ConversationsLoaded(request, result) if self.app.accepts_page(*request) => {
                Some((*request, result.is_ok()))
            }
            _ => None,
        };
        let compact_destination = match &message {
            Message::SelectFolder(_) => Some(mailbox::CompactView::Mail),
            Message::SelectConversation(_) => Some(mailbox::CompactView::Reader),
            Message::KeyPressed(press)
                if press.key == Key::Enter && !press.command && !press.other_modifier =>
            {
                Some(mailbox::CompactView::Reader)
            }
            _ => None,
        };
        let was_composing = self.app.compose().is_some();
        let effects = self.app.update(message);
        if let Some(view) = compact_destination {
            self.ui.compact_view = view;
        }
        if self.app.settings().compose_placement == ComposePlacement::ReadingPane
            && self.app.compose().is_some()
            && !was_composing
        {
            self.ui.compact_view = mailbox::CompactView::Reader;
        }
        if self.app.mailbox().is_none() {
            self.ui.conversation_scroll = None;
            self.ui.compact_view = mailbox::CompactView::default();
        }
        if submitted_or_cancelled
            || !matches!(
                self.app.auth_state(),
                AuthState::SignedOut | AuthState::NeedsMailboxPassword
            )
        {
            self.ui.show_password = false;
        }
        if submitted_or_cancelled || self.app.auth_state().is_authenticated() {
            login::forget_focus(context);
        }
        self.update_notifications(context);
        self.execute(effects, context);
        if let Some((request, succeeded)) = page {
            self.auto_refresh.page_finished(request, succeeded, now);
        }
        self.auto_refresh
            .cancel_superseded(|request| self.app.accepts_page(request), now);
    }

    fn resolve_settings_exit(&mut self, decision: settings::ExitDecision, context: &egui::Context) {
        let Some(destination) = self.pending_settings_exit.take() else {
            return;
        };
        context.request_repaint();
        match decision {
            settings::ExitDecision::KeepEditing => return,
            settings::ExitDecision::Discard => {}
            settings::ExitDecision::Apply => {
                if let Some(draft) = &self.settings_draft {
                    for change in settings::preference_changes(self.app.settings(), draft) {
                        self.dispatch(change, context);
                    }
                }
            }
        }
        self.settings_draft = None;
        match destination {
            SettingsExit::Navigate(message) => self.dispatch(*message, context),
            SettingsExit::CloseWindow => self.request_window_close(context),
        }
    }

    fn request_window_close(&mut self, context: &egui::Context) {
        if self.window_close_approved {
            return;
        }
        // A send, a native file dialog or a settings prompt already open keeps
        // the window open without asking anything more.
        let sending = self
            .app
            .compose()
            .is_some_and(crate::app::Compose::in_flight);
        if sending || self.file_dialog_open || self.pending_settings_exit.is_some() {
            Self::cancel_root_close(context);
            return;
        }
        if self.has_unapplied_settings() {
            Self::cancel_root_close(context);
            self.pending_settings_exit = Some(SettingsExit::CloseWindow);
            Self::bring_main_window_forward(context);
            return;
        }
        // A written draft is discarded only once asked, and so is one kept
        // for its account to sign in again.
        if self
            .app
            .compose()
            .is_some_and(|draft| !draft.is_untouched())
            || self.app.has_parked_draft()
        {
            Self::cancel_root_close(context);
            self.pending_discard = Some(close::Leaving::Window);
            Self::bring_main_window_forward(context);
            return;
        }
        if self.deferred_leave.defer(
            close::Leaving::Window,
            self.holding(close::Leaving::Window).is_some(),
        ) {
            // Changes still on their way to Proton would be lost, and a
            // sign-out cut short could leave the session saved; the window
            // closes once they are done.
            Self::cancel_root_close(context);
            context.request_repaint();
            return;
        }
        self.window_close_approved = true;
        context.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
    }

    /// Keeps the main window open after a close was asked for.
    fn cancel_root_close(context: &egui::Context) {
        context.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::CancelClose);
    }

    /// Shows the main window, so the question it now asks is seen.
    fn bring_main_window_forward(context: &egui::Context) {
        context.send_viewport_cmd_to(
            egui::ViewportId::ROOT,
            egui::ViewportCommand::Minimized(false),
        );
        context.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
        context.request_repaint();
    }

    /// Whether something owns the window until it is answered: a
    /// confirmation, or a native file dialog, whose result is what releases
    /// its slot. Shortcuts, menus, dropped files and activations wait for it.
    fn prompt_open(&self) -> bool {
        self.pending_discard.is_some()
            || self.pending_settings_exit.is_some()
            || self.file_dialog_open
    }

    /// Whether the open settings page has changes that are not applied yet.
    fn has_unapplied_settings(&self) -> bool {
        self.app.showing_settings()
            && self.settings_draft.as_ref().is_some_and(|draft| {
                !settings::preference_changes(self.app.settings(), draft).is_empty()
            })
    }

    /// What signing out leaves: the demo mailbox, or the account.
    fn sign_out_leaving(&self) -> close::Leaving {
        if self.app.is_demo() {
            close::Leaving::Demo
        } else {
            close::Leaving::SignOut
        }
    }

    fn check_window_close(&mut self, context: &egui::Context) {
        // eframe also calls logic while minimized, so cancellation belongs here.
        if context.input(|input| input.viewport().close_requested()) {
            self.request_window_close(context);
        }
        // A close or a sign-out that waited goes ahead once nothing holds it.
        if let Some(leaving) = self.deferred_leave.waiting()
            && self.holding(leaving).is_none()
        {
            self.deferred_leave.cancel();
            self.leave(leaving, context);
        }
    }

    /// What leaving now would cut short: a sign-out under way, for a close,
    /// whose local cleanup could stop half done and leave the session saved;
    /// or changes still on their way to Proton.
    fn holding(&self, leaving: close::Leaving) -> Option<close::Holding> {
        if leaving == close::Leaving::Window
            && matches!(self.app.auth_state(), AuthState::SigningOut)
        {
            return Some(close::Holding::SigningOut);
        }
        let unsent = self.app.unsent_changes();
        (unsent > 0).then_some(close::Holding::Changes(unsent))
    }

    /// Goes ahead with `leaving`, through the same checks as when it was
    /// first asked for.
    fn leave(&mut self, leaving: close::Leaving, context: &egui::Context) {
        match leaving {
            close::Leaving::Window => self.request_window_close(context),
            close::Leaving::SignOut | close::Leaving::Demo => {
                self.dispatch(Message::Logout, context);
            }
        }
    }

    fn resolve_discard(&mut self, decision: Message, context: &egui::Context) {
        let leaving = self.pending_discard.take();
        let discard = matches!(decision, Message::DiscardCompose);
        self.dispatch(decision, context);
        // App refuses to discard an in-flight send, so leaving checks again.
        if discard && let Some(leaving) = leaving {
            self.leave(leaving, context);
        }
        context.request_repaint();
    }

    #[cfg(target_os = "macos")]
    fn native_action(&mut self, action: macos::Action, context: &egui::Context) {
        match action {
            macos::Action::Quit => self.request_window_close(context),
            macos::Action::Settings | macos::Action::NewMessage
                if self.app.auth_state().is_authenticated() && !self.prompt_open() =>
            {
                self.dispatch(
                    match action {
                        macos::Action::Settings => Message::ShowSettings(true),
                        _ => Message::OpenCompose,
                    },
                    context,
                );
                context.send_viewport_cmd_to(
                    egui::ViewportId::ROOT,
                    egui::ViewportCommand::Minimized(false),
                );
                let destination = if matches!(action, macos::Action::NewMessage)
                    && self.app.compose().is_some()
                    && self.app.settings().compose_placement == ComposePlacement::Window
                    && self.pending_settings_exit.is_none()
                {
                    compose::viewport_id()
                } else {
                    egui::ViewportId::ROOT
                };
                context.send_viewport_cmd_to(destination, egui::ViewportCommand::Minimized(false));
                context.send_viewport_cmd_to(destination, egui::ViewportCommand::Focus);
            }
            macos::Action::Zoom(step) => self.step_zoom(step, context),
            macos::Action::Edit(edit) => {
                let events = edit.events();
                if self.composer_window_focused(context) {
                    self.composer_edits.extend(events);
                } else {
                    // This runs in the main window's pass, before its widgets
                    // read their input.
                    context.input_mut(|input| input.events.extend(events));
                }
            }
            _ => {}
        }
    }

    fn execute(&mut self, effects: Effects, context: &egui::Context) {
        let mut visual = false;
        for effect in self.runtime.execute(effects) {
            if effect.is_visual() {
                visual = true;
            }
            match effect {
                UiEffect::CopyText(text) => context.copy_text(text),
                UiEffect::FocusSearch => {
                    self.ui.focus_search = true;
                    self.ui.compact_view = mailbox::CompactView::Mail;
                }
                UiEffect::ScrollReaderTop => {
                    self.ui.scroll_reader_top = true;
                    self.ui.land_reader = true;
                    self.ui.compact_view = mailbox::CompactView::Reader;
                }
                UiEffect::RevealConversation(id) => self.ui.reveal_conversation = Some(id),
                UiEffect::ResetLayout => {
                    let window = self.app.settings().window;
                    let zoom = context.zoom_factor();
                    context.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                        window.width / zoom,
                        window.height / zoom,
                    )));
                    self.layout_reset_at = Some(context.input(|input| input.time));
                    context.request_repaint_after(std::time::Duration::from_secs_f64(
                        LAYOUT_RESET_WAIT,
                    ));
                }
                UiEffect::NotifyNewMail { sender, subject } => {
                    self.notifications
                        .show(&sender, &subject, &self.runtime, context);
                }
                UiEffect::RaiseComposer => {
                    let composer = compose::viewport_id();
                    context.send_viewport_cmd_to(composer, egui::ViewportCommand::Minimized(false));
                    context.send_viewport_cmd_to(composer, egui::ViewportCommand::Focus);
                }
                UiEffect::PickComposeAttachments(id) => {
                    self.file_dialog_open = true;
                    let parent = self.main_window.clone();
                    let task = dialogs::open(
                        parent.as_ref(),
                        self.app.settings().compose_placement == ComposePlacement::Window,
                        |dialog| dialog.set_title("Attach files").pick_files(),
                    );
                    self.runtime.spawn_message(async move {
                        let _keep_parent_alive = parent;
                        let files = task.await;
                        let paths: Vec<std::path::PathBuf> = files
                            .unwrap_or_default()
                            .into_iter()
                            .map(|f| f.path().to_path_buf())
                            .collect();
                        Some(Message::AddComposeAttachments(id, paths))
                    });
                }
                UiEffect::PickAttachmentDestination {
                    request,
                    suggested_name,
                } => {
                    self.file_dialog_open = true;
                    let parent = self.main_window.clone();
                    let task = dialogs::open(parent.as_ref(), false, |dialog| {
                        dialog
                            .set_title("Save attachment as (choose a new filename)")
                            .set_file_name(suggested_name)
                            .save_file()
                    });
                    self.runtime.spawn_message(async move {
                        let _keep_parent_alive = parent;
                        let destination = task.await.map(|file| file.path().to_path_buf());
                        Some(Message::AttachmentDestinationChosen(request, destination))
                    });
                }
            }
        }
        if visual {
            context.request_repaint();
        }
    }

    fn drain_runtime(&mut self, context: &egui::Context) {
        let messages: Vec<_> = self.runtime.drain().collect();
        for message in messages {
            self.dispatch(message, context);
        }
    }

    fn remember_window_size(&mut self, context: &egui::Context) {
        let (size, maximized, passing) = context.input(|input| {
            let viewport = input.viewport();
            (
                viewport.inner_rect.map(|rect| rect.size()),
                // Zoom only makes a Mac window larger, and that frame comes
                // back as it is, as it does for other Mac apps. winit does not
                // report the zoom it restores as maximized, either.
                viewport.maximized == Some(true) && !cfg!(target_os = "macos"),
                viewport.fullscreen == Some(true) || viewport.minimized == Some(true),
            )
        });
        // Full screen and the Dock or taskbar are passing states: the window
        // to come back to is the one already remembered, and a reset can only
        // run out of time.
        let Some(size) = size.filter(|_| !passing) else {
            self.settle_layout_reset(None, context);
            return;
        };
        let saved = self.app.settings().window;
        let current = if maximized {
            // Restoring it returns to the normal size and place, which stay.
            Window {
                maximized: true,
                ..saved
            }
        } else {
            // Store unscaled points so zoom does not change the remembered window.
            let unscaled = size * context.zoom_factor();
            Window {
                width: unscaled.x,
                height: unscaled.y,
                maximized: false,
                position: self
                    .main_window
                    .as_deref()
                    .and_then(window_place::position)
                    .or(saved.position),
            }
        };
        if self.settle_layout_reset(Some(&current), context) {
            return;
        }
        if let Some(last) = self.last_window_size
            && (last.width - current.width).abs() < f32::EPSILON
            && (last.height - current.height).abs() < f32::EPSILON
            && last.maximized == current.maximized
            && last.position == current.position
        {
            return;
        }
        self.last_window_size = Some(current);
        self.dispatch(Message::WindowResized(current), context);
    }

    /// Ends a layout reset once the window has its default size or the reset
    /// has waited long enough, and puts the panes back to their default sizes
    /// too. Returns whether the reset still waits. `current` is the window's
    /// size, or `None` while it is full screen or minimized.
    fn settle_layout_reset(&mut self, current: Option<&Window>, context: &egui::Context) -> bool {
        let Some(started_at) = self.layout_reset_at else {
            return false;
        };
        let target = Window::default();
        let reached = current.is_some_and(|current| {
            (current.width - target.width).abs() <= 1.0
                && (current.height - target.height).abs() <= 1.0
        });
        let timed_out = context.input(|input| input.time) - started_at >= LAYOUT_RESET_WAIT;
        if !reached && !timed_out {
            return true;
        }
        self.layout_reset_at = None;
        self.last_window_size = None;
        mailbox::reset_panel_sizes(context);
        false
    }

    /// Command with plus, equals, minus or zero steps the zoom on every
    /// screen, as Settings would. On macOS the View menu takes all but equals.
    fn zoom_shortcuts(&mut self, context: &egui::Context) {
        if let Some(step) = zoom_keys(context) {
            self.step_zoom(step, context);
        }
    }

    /// Files dropped on the window that shows the draft join it, unless a
    /// file dialog or a confirmation owns the window: a dialog's own result
    /// is what releases its slot.
    fn attach_dropped(&mut self, paths: Vec<std::path::PathBuf>, context: &egui::Context) {
        if paths.is_empty() || self.prompt_open() {
            return;
        }
        if let Some(draft) = self.app.compose().map(crate::app::Compose::id) {
            self.dispatch(Message::AddComposeAttachments(draft, paths), context);
        }
    }

    /// Moves the zoom one offered step, keeping an open Settings page, whose
    /// draft would otherwise put the old zoom back, in step with it.
    fn step_zoom(&mut self, step: settings::ZoomStep, context: &egui::Context) {
        let zoom = settings::zoom_step(self.app.settings().zoom, step);
        if let Some(draft) = &mut self.settings_draft {
            draft.zoom = zoom;
        }
        if (zoom - self.app.settings().zoom).abs() > f32::EPSILON {
            self.dispatch(Message::SetZoom(zoom), context);
        }
    }

    fn keyboard_shortcuts(&mut self, context: &egui::Context) {
        if !self.app.auth_state().is_authenticated()
            || self.prompt_open()
            || self.deferred_leave.waiting().is_some()
        {
            return;
        }

        let wants_text = context.egui_wants_keyboard_input();
        // Letters only matter to a text field: a focused button or row leaves
        // j and k to the list.
        let typing = context.text_edit_focused();
        // An open menu or list takes Escape for itself, closing later in this
        // pass, so what is behind it does not close as well.
        let menu_open = egui::Popup::is_any_open(context);
        let presses = context.input(|input| {
            let command = input.modifiers.command && !ctrl_alt_types(input.modifiers);
            let plain = !command && !input.modifiers.alt && !input.modifiers.shift;
            let mut presses = Vec::new();
            if command && input.key_pressed(egui::Key::R) {
                presses.push(KeyPress::with_command(Key::Character('r')));
            }
            if command && input.key_pressed(egui::Key::F) {
                presses.push(KeyPress::with_command(Key::Character('f')));
            }
            if command && input.key_pressed(egui::Key::Comma) {
                presses.push(KeyPress::with_command(Key::Character(',')));
            }
            if command && input.key_pressed(egui::Key::N) {
                presses.push(KeyPress::with_command(Key::Character('n')));
            }
            if command && input.key_pressed(egui::Key::Enter) {
                presses.push(KeyPress::with_command(Key::Enter));
            }
            if !menu_open && input.key_pressed(egui::Key::Escape) {
                presses.push(KeyPress::plain(Key::Escape));
            }
            // A focused widget keeps the arrows and Enter: egui moves focus
            // with the arrows and presses a focused button with Enter. Delete
            // waits too, as a focused row need not be the selected one.
            if plain && !wants_text && input.key_pressed(egui::Key::ArrowDown) {
                presses.push(KeyPress::plain(Key::ArrowDown));
            } else if plain && !wants_text && input.key_pressed(egui::Key::ArrowUp) {
                presses.push(KeyPress::plain(Key::ArrowUp));
            } else if plain && !wants_text && input.key_pressed(egui::Key::Enter) {
                presses.push(KeyPress::plain(Key::Enter));
            } else if plain && !wants_text && input.key_pressed(egui::Key::Delete) {
                presses.push(KeyPress::plain(Key::Delete));
            } else if plain && !typing {
                let letter = input.events.iter().find_map(|event| match event {
                    egui::Event::Text(text) => text
                        .chars()
                        .next()
                        .filter(|first| matches!(first, 'j' | 'k')),
                    _ => None,
                });
                presses.extend(letter.map(|letter| KeyPress::plain(Key::Character(letter))));
            } else if !wants_text && command && !input.modifiers.alt && !input.modifiers.shift {
                if input.key_pressed(egui::Key::Backspace) {
                    presses.push(KeyPress::with_command(Key::Backspace));
                } else if input.key_pressed(egui::Key::Delete) {
                    presses.push(KeyPress::with_command(Key::Delete));
                }
            }
            presses
        });

        // j or k past a focused row or button moves the list on, so that
        // widget lets focus go instead of keeping its ring behind.
        let letter = presses
            .iter()
            .any(|press| matches!(press.key, Key::Character('j' | 'k')));
        if wants_text && letter {
            context.memory_mut(|memory| {
                if let Some(focused) = memory.focused() {
                    memory.surrender_focus(focused);
                }
            });
        }
        for press in presses {
            self.dispatch(Message::KeyPressed(press), context);
        }
    }

    fn app_has_focus(&self, context: &egui::Context) -> bool {
        context.input_for(egui::ViewportId::ROOT, |input| input.focused)
            || self.composer_window_focused(context)
    }

    fn composer_window_focused(&self, context: &egui::Context) -> bool {
        self.app.settings().compose_placement == ComposePlacement::Window
            && self.app.compose().is_some()
            && context.input_for(compose::viewport_id(), |input| input.focused)
    }

    fn refresh_automatically(&mut self, context: &egui::Context) {
        let now = context.input(|input| input.time);
        let focused = self.app_has_focus(context);
        let active = self.app.auth_state().is_authenticated()
            && self.app.mailbox().is_some()
            && !self.app.is_demo();
        let available = self.app.auto_refresh_available();

        if self.auto_refresh.should_start(
            now,
            auto_refresh::Conditions {
                focused,
                active,
                available,
            },
        ) {
            self.dispatch(Message::AutoRefreshMailbox, context);
            let request = self
                .app
                .mailbox()
                .and_then(|mailbox| match mailbox.status() {
                    crate::app::ListStatus::Loading(request)
                    | crate::app::ListStatus::Refreshing(request) => Some(request),
                    _ => None,
                });
            if let Some(request) = request {
                self.auto_refresh.started(request);
            } else {
                self.auto_refresh.postpone(now);
            }
        }

        if let Some(wait) = self
            .auto_refresh
            .repaint_after(now, self.app.auto_refresh_available())
        {
            context.request_repaint_after(wait);
        }
    }

    /// Applies UI zoom without changing the remembered window size.
    fn apply_zoom(&self, context: &egui::Context) {
        let zoom = self.app.settings().zoom;
        if (context.zoom_factor() - zoom).abs() > f32::EPSILON {
            context.set_zoom_factor(zoom);
        }
    }

    /// Writes settled settings and schedules the frame that performs it.
    fn save_settled_settings(&mut self, context: &egui::Context) {
        let revision = self.app.settings_revision();
        let now = context.input(|input| input.time);
        if revision != self.settings_seen {
            self.settings_seen = revision;
            self.settings_seen_at = now;
        }
        if !self.app.settings_unsaved() {
            return;
        }
        if now - self.settings_seen_at >= f64::from(SETTINGS_QUIET) {
            let write = self.app.save_settings();
            self.execute(write, context);
        } else {
            context.request_repaint_after(std::time::Duration::from_secs_f32(SETTINGS_QUIET));
        }
    }

    fn update_title(&mut self, context: &egui::Context) {
        let title = window_title(&self.app, self.demo_process);
        if title != self.title {
            self.title.clone_from(&title);
            context.send_viewport_cmd(egui::ViewportCommand::Title(title));
        }
    }

    fn update_unread_badge(&mut self) {
        let badge = unread_badge_count(&self.app);
        if badge != self.unread_badge && set_unread_badge(self.main_window.as_deref(), badge) {
            self.unread_badge = badge;
        }
    }

    fn update_notifications(&mut self, context: &egui::Context) {
        let enabled = self.app.auth_state().is_authenticated()
            && self.app.settings().desktop_notifications
            && !self.app.is_demo();
        self.notifications.set_enabled(enabled, context);
        self.ui.notification_status = self.notifications.status();
    }

    fn receive_activation(&mut self, context: &egui::Context) {
        let events = self
            .activation
            .as_ref()
            .map(crate::launch::Instance::drain)
            .unwrap_or_default();
        if !events.is_empty() {
            self.focus_after_activation = true;
            // OS focus policy may decline activation (especially on Wayland).
            context.send_viewport_cmd_to(
                egui::ViewportId::ROOT,
                egui::ViewportCommand::Minimized(false),
            );
        }
        for event in events {
            match event {
                crate::launch::Event::Activate => {}
                crate::launch::Event::Mailto(request) => {
                    self.dispatch(Message::ReceiveMailto(request), context)
                }
                crate::launch::Event::Rejected => self.dispatch(Message::MailtoRejected, context),
            }
        }
        self.open_waiting_mailto(context);
    }

    fn open_waiting_mailto(&mut self, context: &egui::Context) {
        if self.activation_blocked() || !self.app.can_open_pending_mailto() {
            return;
        }
        self.dispatch(Message::OpenPendingMailto, context);
        if self.app.compose().is_some() {
            self.focus_after_activation = true;
            context.request_repaint();
        }
    }

    fn focus_activated_composer(&mut self, context: &egui::Context) {
        if !self.focus_after_activation || self.activation_blocked() {
            return;
        }
        let separate = self.app.compose().is_some()
            && !self.app.showing_settings()
            && self.app.settings().compose_placement == ComposePlacement::Window;
        let target = if separate {
            compose::viewport_id()
        } else {
            egui::ViewportId::ROOT
        };
        if separate && !context.input(|input| input.raw.viewports.contains_key(&target)) {
            return;
        }
        context.send_viewport_cmd_to(target, egui::ViewportCommand::Minimized(false));
        context.send_viewport_cmd_to(target, egui::ViewportCommand::Focus);
        #[cfg(target_os = "linux")]
        context.send_viewport_cmd_to(
            target,
            egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational),
        );
        self.focus_after_activation = false;
    }

    fn activation_blocked(&self) -> bool {
        self.window_close_approved || self.prompt_open()
    }
}

impl eframe::App for DesktopApp {
    fn logic(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.runtime.attach(context);
        apply_appearance(context, self.app.settings());
        theme::reduce_motion(context, system_reduces_motion());
        self.root_title_bar.keep(context);
        self.apply_zoom(context);
        fonts::apply(
            context,
            fonts::Typeface::chosen(self.app.settings().system_font),
        );
        self.drain_runtime(context);
        #[cfg(target_os = "macos")]
        {
            macos::update_window_chrome(context, _frame);
            let actions = self
                .native_menu
                .as_ref()
                .map(macos::Menu::drain)
                .unwrap_or_default();
            for action in actions {
                self.native_action(action, context);
            }
            if let Some(menu) = &self.native_menu {
                menu.adopt_windows();
                menu.set_enabled(self.app.auth_state().is_authenticated() && !self.prompt_open());
            }
        }
        self.check_window_close(context);
        self.receive_activation(context);
        self.focus_activated_composer(context);
        self.update_notifications(context);
        self.remember_window_size(context);
        self.zoom_shortcuts(context);
        self.keyboard_shortcuts(context);
        self.forget_finished_draft(context);
        self.refresh_automatically(context);
        self.save_settled_settings(context);
        self.update_title(context);
        self.update_unread_badge();
    }

    /// Closing is the one moment the settings must reach the file whether or
    /// not they have settled.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.notifications.stop();
        self.app.save_settings_before_quitting();
        set_unread_badge(self.main_window.as_deref(), None);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.window_close_approved {
            return;
        }
        let context = ui.ctx().clone();
        let mut messages = Vec::new();
        let mut composer_zoom = None;
        let mut dropped = Vec::new();
        email_link_notice(ui, &self.app, &mut messages);
        if !self.app.showing_settings() || !self.app.auth_state().is_authenticated() {
            self.settings_draft = None;
            self.pending_settings_exit = None;
        }
        match self.app.auth_state() {
            AuthState::CheckingSession => status_view(ui, "Opening Ruston Mail…"),
            AuthState::SignedOut
            | AuthState::SigningIn(_)
            | AuthState::NeedsTotp
            | AuthState::NeedsMailboxPassword
            | AuthState::NeedsHumanVerification { .. } => {
                login::show(ui, &mut self.app, &mut self.ui.show_password, &mut messages);
            }
            AuthState::Authenticated { .. } => {
                if self.app.mailbox().is_some() {
                    let draft = if self.app.showing_settings() {
                        Some(
                            self.settings_draft
                                .get_or_insert_with(|| self.app.settings().clone()),
                        )
                    } else {
                        None
                    };
                    mailbox::show(ui, &self.app, draft, &mut self.ui, &mut messages);
                } else {
                    status_view(ui, "Opening mailbox…");
                }

                if self.app.settings().compose_placement == ComposePlacement::Window
                    && let Some(writing) = self.app.compose()
                {
                    let window = compose::show_window(
                        &context,
                        writing,
                        self.pending_discard.is_some() || self.pending_settings_exit.is_some(),
                        std::mem::take(&mut self.composer_edits),
                        &mut self.composer_title_bar,
                    );
                    messages.extend(window.messages);
                    composer_zoom = window.zoom;
                    dropped = window.dropped;
                } else if self.app.compose().is_some() && !self.app.showing_settings() {
                    // The draft is in this window's reading pane.
                    dropped = dropped_paths(&context);
                }
            }
            AuthState::SigningOut => {
                if self.app.mailbox().is_some() {
                    mailbox::show(ui, &self.app, None, &mut self.ui, &mut messages);
                } else {
                    status_view(ui, "Signing out…");
                }
            }
        }

        if let Some(link) = self.app.pending_link() {
            messages.extend(reader::link_dialog(&context, link));
        }
        // The discard confirmation owns the decision even if input for the
        // same frame was already queued in a pane or the child composer.
        if self.pending_discard.is_some() {
            messages.clear();
        }
        let exit_decision = self
            .pending_settings_exit
            .as_ref()
            .and_then(|_| settings::confirm_exit(&context));
        let discard_decision = self
            .pending_discard
            .and_then(|leaving| close::confirm_discard(&context, leaving));
        let wait_decision = self.deferred_leave.waiting().and_then(|leaving| {
            let holding = self.holding(leaving)?;
            close::confirm_wait(&context, leaving, holding)
        });
        theme::paint_focus_ring(&context);
        if let Some(step) = composer_zoom {
            self.step_zoom(step, &context);
        }
        self.attach_dropped(dropped, &context);
        let has_messages = !messages.is_empty();
        for message in messages {
            self.dispatch(message, &context);
        }
        if let Some(decision) = exit_decision {
            self.resolve_settings_exit(decision, &context);
        }
        if let Some(decision) = discard_decision {
            self.resolve_discard(decision, &context);
        }
        match wait_decision {
            Some(close::WaitDecision::Stay) => self.deferred_leave.cancel(),
            // Asking again while it waits goes ahead at once.
            Some(close::WaitDecision::GoNow) => {
                if let Some(leaving) = self.deferred_leave.waiting() {
                    self.leave(leaving, &context);
                }
            }
            None => {}
        }
        if has_messages {
            context.request_repaint();
        }
    }
}

/// The local files dropped on the window being drawn in this pass. Wayland
/// delivers none: winit does not support dropping files there.
fn dropped_paths(context: &egui::Context) -> Vec<std::path::PathBuf> {
    context.input(|input| {
        input
            .raw
            .dropped_files
            .iter()
            .map(|file| file.path().to_path_buf())
            .collect()
    })
}

/// Takes the zoom keys pressed in the window being drawn, the mailbox's or
/// the composer's, and returns the step they ask for.
fn zoom_keys(context: &egui::Context) -> Option<settings::ZoomStep> {
    use egui::gui_zoom::kb_shortcuts;
    context.input_mut(|input| {
        if ctrl_alt_types(input.modifiers) {
            None
        } else if input.consume_shortcut(&kb_shortcuts::ZOOM_RESET) {
            Some(settings::ZoomStep::Reset)
        } else if input.consume_shortcut(&kb_shortcuts::ZOOM_IN)
            || input.consume_shortcut(&kb_shortcuts::ZOOM_IN_SECONDARY)
        {
            Some(settings::ZoomStep::In)
        } else if input.consume_shortcut(&kb_shortcuts::ZOOM_OUT) {
            Some(settings::ZoomStep::Out)
        } else {
            None
        }
    })
}

/// On Windows Ctrl+Alt types what AltGr types, as "ń" with N on a Polish
/// layout, so it takes no Ctrl shortcut there.
fn ctrl_alt_types(modifiers: egui::Modifiers) -> bool {
    cfg!(target_os = "windows") && modifiers.command && modifiers.alt
}

fn apply_appearance(context: &egui::Context, settings: &Settings) {
    if settings.follow_system_appearance {
        theme::follow_system(context, settings.appearance);
    } else {
        theme::apply(context, settings.appearance);
    }
}

/// Whether Windows' "Animation effects" are off, under Accessibility ›
/// Visual effects. Read on every pass, which costs one call, so a change
/// shows on the next frame. Other systems' settings are not read yet.
#[cfg(target_os = "windows")]
fn system_reduces_motion() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SPI_GETCLIENTAREAANIMATION, SystemParametersInfoW,
    };
    let mut animated: windows_sys::core::BOOL = 1;
    // SAFETY: this action writes one BOOL through the pointer.
    let read = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            (&mut animated as *mut windows_sys::core::BOOL).cast(),
            0,
        )
    };
    read != 0 && animated == 0
}

#[cfg(not(target_os = "windows"))]
fn system_reduces_motion() -> bool {
    false
}

fn status_view(root: &mut egui::Ui, message: &str) {
    egui::CentralPanel::default().show(root, |ui| {
        theme::centered_group(ui, |ui| {
            ui.heading(egui::RichText::new("Ruston Mail").size(36.0).strong());
            ui.add_space(16.0);
            ui.label(message);
            ui.add_space(12.0);
            theme::spinner(ui);
        });
    });
}

fn email_link_notice(root: &mut egui::Ui, app: &App, messages: &mut Vec<Message>) {
    let count = app.pending_mailto_count();
    if count == 0 && app.mailto_error().is_none() {
        return;
    }
    let reason = if !app.auth_state().is_authenticated() {
        "Sign in to write the message."
    } else if app.compose().is_some() {
        "Finish or close your current draft to open it."
    } else {
        "Close Settings or the open dialog to continue."
    };
    let notice = if count == 1 {
        format!("An email link is waiting. {reason}")
    } else {
        format!("{count} email links are waiting. {reason}")
    };
    egui::Panel::top("pending-email-links")
        .frame(theme::top_panel_frame(
            theme::colors(root).panel,
            theme::titlebar_inset(root.ctx()),
        ))
        .show(root, |ui| {
            if let Some(error) = app.mailto_error() {
                ui.horizontal_wrapped(|ui| {
                    ui.label(error);
                    if ui.button("Dismiss").clicked() {
                        messages.push(Message::DismissMailtoError);
                    }
                });
            }
            if count > 0 {
                ui.horizontal_wrapped(|ui| {
                    ui.label(notice);
                    if ui.button("Dismiss link").clicked() {
                        messages.push(Message::DismissPendingMailto);
                    }
                });
            }
        });
}

fn version_label(ui: &mut egui::Ui) -> egui::Response {
    ui.label(
        egui::RichText::new(concat!("v", env!("CARGO_PKG_VERSION")))
            .size(theme::FOOTNOTE_SIZE)
            .color(theme::colors(ui).muted),
    )
}

fn window_title(app: &App, demo: bool) -> String {
    title_label(inbox_unread(app), demo)
}

/// The Inbox's unread count, once the mailbox has its counts.
fn inbox_unread(app: &App) -> Option<u32> {
    app.mailbox()
        .and_then(|mailbox| mailbox.counts()?.unread(&Folder::INBOX))
}

fn title_label(unread: Option<u32>, demo: bool) -> String {
    let name = if demo {
        "Ruston Mail (demo)"
    } else {
        "Ruston Mail"
    };
    match unread.filter(|unread| *unread > 0) {
        Some(unread) => format!("{name} ({unread})"),
        None => name.to_owned(),
    }
}

pub(crate) fn unread_badge_count(app: &App) -> Option<u32> {
    if !app.settings().show_unread_badge {
        return None;
    }
    inbox_unread(app).filter(|&count| count > 0)
}

/// Shows the unread count on the Dock icon, on Windows the taskbar button,
/// and on Linux the dock or panel icon. Returns false while the window cannot
/// show it yet, so the next pass tries again.
#[cfg(target_os = "macos")]
fn set_unread_badge(_window: Option<&winit::window::Window>, count: Option<u32>) -> bool {
    use objc2_app_kit::NSApplication;
    use objc2_foundation::{MainThreadMarker, NSString};

    if let Some(mtm) = MainThreadMarker::new() {
        let app = NSApplication::sharedApplication(mtm);
        let dock_tile = app.dockTile();
        let label = count.map(|c| NSString::from_str(&c.to_string()));
        dock_tile.setBadgeLabel(label.as_deref());
    }
    true
}

#[cfg(target_os = "windows")]
fn set_unread_badge(window: Option<&winit::window::Window>, count: Option<u32>) -> bool {
    window.is_none_or(|window| taskbar::show_unread(window, count))
}

#[cfg(target_os = "linux")]
fn set_unread_badge(_window: Option<&winit::window::Window>, count: Option<u32>) -> bool {
    launcher::show_unread(count);
    true
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn set_unread_badge(_window: Option<&winit::window::Window>, _count: Option<u32>) -> bool {
    true
}

#[cfg(test)]
mod tests;
