mod auto_refresh;
mod close;
mod compose;
mod dialogs;
mod identity;
mod login;
#[cfg(target_os = "macos")]
mod macos;
mod mailbox;
mod notifications;
mod reader;
mod settings;
mod theme;

use eframe::egui;

use crate::app::{App, AuthState, Effects, Key, KeyPress, Message, UiEffect, UndoMove};
use crate::mail::Folder;
use crate::runtime::Runtime;
use crate::settings::{ComposePlacement, Settings, Window};
use auto_refresh::AutoRefresh;

const MIN_WINDOW_SIZE: [f32; 2] = [820.0, 480.0];
/// Debounces settings writes while a window or divider is dragged.
const SETTINGS_QUIET: f32 = 0.75;
const LAYOUT_RESET_WAIT: f64 = 2.0;

#[derive(Default)]
pub(super) struct UiState {
    /// Password reveal is local to the current sign-in interaction.
    show_password: bool,
    focus_search: bool,
    scroll_reader_top: bool,
    reveal_conversation: Option<String>,
    conversation_scroll: Option<mailbox::ConversationScroll>,
    compact_view: mailbox::CompactView,
    undo_notice: Option<UndoNotice>,
    notification_status: notifications::Status,
}

struct UndoNotice {
    offer: UndoMove,
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

    let settings = Settings::load();
    let options = eframe::NativeOptions {
        viewport: main_viewport(&settings),
        ..Default::default()
    };

    eframe::run_native(
        "Ruston Mail",
        options,
        Box::new(move |creation| {
            let mut desktop = DesktopApp::new(creation, demo, settings);
            activation.attach(&creation.egui_ctx);
            desktop.activation = Some(activation);
            Ok(Box::new(desktop))
        }),
    )
}

struct DesktopApp {
    app: App,
    runtime: Runtime,
    ui: UiState,
    demo: bool,
    title: String,
    dock_badge: Option<u32>,
    auto_refresh: AutoRefresh,
    /// Preferences being edited in the settings pane until Apply is pressed.
    settings_draft: Option<Settings>,
    /// Where to go after the user decides what to do with unsaved preferences.
    pending_settings_exit: Option<SettingsExit>,
    pending_window_close: bool,
    window_close_approved: bool,
    /// Revision and time used to debounce settings writes.
    settings_seen: u64,
    settings_seen_at: f64,
    /// Last window size dispatched, avoiding busy-loop repainting when window size is unchanged.
    last_window_size: Option<Window>,
    /// Waits for the requested window size before resetting egui's cached pane widths.
    layout_reset_at: Option<f64>,
    /// Retained until each native dialog completes, including portal export.
    dialog_parent: Option<std::sync::Arc<winit::window::Window>>,
    /// Native dialog completion, including cancellation, releases this slot.
    file_dialog_open: bool,
    notifications: notifications::Notifications,
    activation: Option<crate::launch::Instance>,
    focus_after_activation: bool,
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
        app.dialog_parent = creation.winit_window().cloned();
        #[cfg(target_os = "macos")]
        {
            app.native_menu = macos::Menu::install(&creation.egui_ctx);
        }
        app
    }

    fn with_context(context: &egui::Context, demo: bool, settings: Settings) -> Self {
        theme::install(context);
        theme::apply(context, settings.appearance);
        let runtime = Runtime::new().expect("failed to start background runtime");
        runtime.attach(context);
        let (app, effects) = App::boot(demo, settings);
        let mut desktop = Self {
            app,
            runtime,
            ui: UiState::default(),
            demo,
            title: String::new(),
            dock_badge: None,
            auto_refresh: AutoRefresh::default(),
            settings_draft: None,
            pending_settings_exit: None,
            pending_window_close: false,
            window_close_approved: false,
            settings_seen: 0,
            settings_seen_at: 0.0,
            last_window_size: None,
            layout_reset_at: None,
            dialog_parent: None,
            file_dialog_open: false,
            notifications: notifications::Notifications::default(),
            activation: None,
            focus_after_activation: false,
            #[cfg(target_os = "macos")]
            native_menu: None,
        };
        desktop.execute(effects, context);
        desktop
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
            if self.settings_draft.as_ref().is_some_and(|draft| {
                !settings::preference_changes(self.app.settings(), draft).is_empty()
            }) {
                self.pending_settings_exit = Some(SettingsExit::Navigate(Box::new(message)));
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
        let sending = self
            .app
            .compose()
            .is_some_and(crate::app::Compose::in_flight);
        let unsaved_message = self
            .app
            .compose()
            .is_some_and(|draft| !draft.is_untouched());
        let unsaved_settings = self.app.showing_settings()
            && self.settings_draft.as_ref().is_some_and(|draft| {
                !settings::preference_changes(self.app.settings(), draft).is_empty()
            });
        if sending
            || self.file_dialog_open
            || unsaved_message
            || unsaved_settings
            || self.pending_settings_exit.is_some()
        {
            context
                .send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::CancelClose);
            if sending || self.file_dialog_open {
                return;
            }
            if self.pending_settings_exit.is_some() {
                return;
            }
            if unsaved_settings {
                self.pending_settings_exit = Some(SettingsExit::CloseWindow);
            } else {
                self.pending_window_close = true;
            }
            context.send_viewport_cmd_to(
                egui::ViewportId::ROOT,
                egui::ViewportCommand::Minimized(false),
            );
            context.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
            context.request_repaint();
        } else {
            self.window_close_approved = true;
            context.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
        }
    }

    fn check_window_close(&mut self, context: &egui::Context) {
        // eframe also calls logic while minimized, so cancellation belongs here.
        if context.input(|input| input.viewport().close_requested()) {
            self.request_window_close(context);
        }
    }

    fn resolve_window_close(&mut self, decision: Message, context: &egui::Context) {
        self.pending_window_close = false;
        let discard = matches!(decision, Message::DiscardCompose);
        self.dispatch(decision, context);
        if discard {
            // App refuses to discard an in-flight send; recheck before closing.
            self.request_window_close(context);
        }
        context.request_repaint();
    }

    #[cfg(target_os = "macos")]
    fn native_action(&mut self, action: macos::Action, context: &egui::Context) {
        match action {
            macos::Action::Quit => self.request_window_close(context),
            macos::Action::Settings | macos::Action::NewMessage
                if matches!(self.app.auth_state(), AuthState::Authenticated { .. })
                    && !self.pending_window_close
                    && self.pending_settings_exit.is_none()
                    && !self.file_dialog_open =>
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
                UiEffect::PickComposeAttachments(id) => {
                    self.file_dialog_open = true;
                    let parent = self.dialog_parent.clone();
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
                    let parent = self.dialog_parent.clone();
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
        let Some(size) = context.input(|input| input.viewport().inner_rect.map(|rect| rect.size()))
        else {
            return;
        };
        // Store unscaled points so zoom does not change the remembered window.
        let unscaled = size * context.zoom_factor();
        let current = Window {
            width: unscaled.x,
            height: unscaled.y,
        };
        if let Some(started_at) = self.layout_reset_at {
            let target = Window::default();
            let reached = (current.width - target.width).abs() <= 1.0
                && (current.height - target.height).abs() <= 1.0;
            let timed_out = context.input(|input| input.time) - started_at >= LAYOUT_RESET_WAIT;
            if !reached && !timed_out {
                return;
            }
            self.layout_reset_at = None;
            self.last_window_size = None;
            mailbox::reset_panel_sizes(context);
        }
        if let Some(last) = self.last_window_size
            && (last.width - current.width).abs() < f32::EPSILON
            && (last.height - current.height).abs() < f32::EPSILON
        {
            return;
        }
        self.last_window_size = Some(current);
        self.dispatch(Message::WindowResized(current), context);
    }

    fn keyboard_shortcuts(&mut self, context: &egui::Context) {
        if !matches!(self.app.auth_state(), AuthState::Authenticated { .. })
            || self.pending_window_close
            || self.pending_settings_exit.is_some()
        {
            return;
        }

        let wants_text = context.egui_wants_keyboard_input();
        let presses = context.input(|input| {
            let command = input.modifiers.command;
            let mut presses = Vec::new();
            if command && input.key_pressed(egui::Key::R) {
                presses.push(key(Key::Character('r'), true));
            }
            if command && input.key_pressed(egui::Key::F) {
                presses.push(key(Key::Character('f'), true));
            }
            if command && input.key_pressed(egui::Key::Comma) {
                presses.push(key(Key::Character(','), true));
            }
            if command && input.key_pressed(egui::Key::N) {
                presses.push(key(Key::Character('n'), true));
            }
            if command && input.key_pressed(egui::Key::Enter) {
                presses.push(key(Key::Enter, true));
            }
            if input.key_pressed(egui::Key::Escape) {
                presses.push(key(Key::Escape, false));
            }
            if !wants_text && !command && !input.modifiers.alt && !input.modifiers.shift {
                if input.key_pressed(egui::Key::ArrowDown) {
                    presses.push(key(Key::ArrowDown, false));
                } else if input.key_pressed(egui::Key::ArrowUp) {
                    presses.push(key(Key::ArrowUp, false));
                } else if input.key_pressed(egui::Key::Enter) {
                    presses.push(key(Key::Enter, false));
                } else if input.key_pressed(egui::Key::Delete) {
                    presses.push(key(Key::Delete, false));
                } else {
                    for event in &input.events {
                        if let egui::Event::Text(text) = event
                            && let Some(character @ ('j' | 'k')) = text.chars().next()
                        {
                            presses.push(key(Key::Character(character), false));
                            break;
                        }
                    }
                }
            } else if !wants_text && command && !input.modifiers.alt && !input.modifiers.shift {
                if input.key_pressed(egui::Key::Backspace) {
                    presses.push(key(Key::Backspace, true));
                } else if input.key_pressed(egui::Key::Delete) {
                    presses.push(key(Key::Delete, true));
                }
            }
            presses
        });

        for press in presses {
            self.dispatch(Message::KeyPressed(press), context);
        }
    }

    fn app_has_focus(&self, context: &egui::Context) -> bool {
        context.input_for(egui::ViewportId::ROOT, |input| input.focused)
            || self.app.settings().compose_placement == ComposePlacement::Window
                && self.app.compose().is_some()
                && context.input_for(compose::viewport_id(), |input| input.focused)
    }

    fn refresh_automatically(&mut self, context: &egui::Context) {
        let now = context.input(|input| input.time);
        let focused = self.app_has_focus(context);
        let active = matches!(self.app.auth_state(), AuthState::Authenticated { .. })
            && self.app.mailbox().is_some()
            && !self.app.is_demo();
        let available = self.app.auto_refresh_available();

        if self
            .auto_refresh
            .should_start(now, focused, active, available)
        {
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
            self.app.save_settings();
        } else {
            context.request_repaint_after(std::time::Duration::from_secs_f32(SETTINGS_QUIET));
        }
    }

    fn update_title(&mut self, context: &egui::Context) {
        let title = window_title(&self.app, self.demo);
        if title != self.title {
            self.title.clone_from(&title);
            context.send_viewport_cmd(egui::ViewportCommand::Title(title));
        }
    }

    fn update_dock_badge(&mut self) {
        let badge = dock_badge_count(&self.app);
        if badge != self.dock_badge {
            self.dock_badge = badge;
            set_dock_badge(badge);
        }
    }

    fn update_notifications(&mut self, context: &egui::Context) {
        let enabled = matches!(self.app.auth_state(), AuthState::Authenticated { .. })
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
        self.window_close_approved
            || self.pending_window_close
            || self.pending_settings_exit.is_some()
            || self.file_dialog_open
    }
}

impl eframe::App for DesktopApp {
    fn logic(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.runtime.attach(context);
        theme::apply(context, self.app.settings().appearance);
        self.apply_zoom(context);
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
                menu.set_enabled(
                    matches!(self.app.auth_state(), AuthState::Authenticated { .. })
                        && !self.pending_window_close
                        && self.pending_settings_exit.is_none()
                        && !self.file_dialog_open,
                );
            }
        }
        self.check_window_close(context);
        self.receive_activation(context);
        self.focus_activated_composer(context);
        self.update_notifications(context);
        self.remember_window_size(context);
        self.keyboard_shortcuts(context);
        self.refresh_automatically(context);
        self.save_settled_settings(context);
        self.update_title(context);
        self.update_dock_badge();
    }

    /// Closing is the one moment the settings must reach the file whether or
    /// not they have settled.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.notifications.stop();
        self.app.save_settings();
        set_dock_badge(None);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.window_close_approved {
            return;
        }
        let context = ui.ctx().clone();
        let mut messages = Vec::new();
        email_link_notice(ui, &self.app, &mut messages);
        if !self.app.showing_settings()
            || !matches!(self.app.auth_state(), AuthState::Authenticated { .. })
        {
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
            AuthState::Authenticated { email } => {
                if self.app.mailbox().is_some() {
                    let draft = if self.app.showing_settings() {
                        Some(
                            self.settings_draft
                                .get_or_insert_with(|| self.app.settings().clone()),
                        )
                    } else {
                        None
                    };
                    mailbox::show(
                        ui,
                        &self.app,
                        email.as_deref(),
                        false,
                        draft,
                        &mut self.ui,
                        &mut messages,
                    );
                } else {
                    status_view(ui, "Opening mailbox…");
                }

                if self.app.settings().compose_placement == ComposePlacement::Window
                    && let Some(writing) = self.app.compose()
                {
                    messages.extend(compose::show_window(
                        &context,
                        writing,
                        self.pending_window_close || self.pending_settings_exit.is_some(),
                    ));
                }
            }
            AuthState::SigningOut => {
                if self.app.mailbox().is_some() {
                    mailbox::show(ui, &self.app, None, true, None, &mut self.ui, &mut messages);
                } else {
                    status_view(ui, "Signing out…");
                }
            }
        }

        // Closing confirmation owns the decision even if input for the same
        // frame was already queued in a pane or the child composer.
        if self.pending_window_close {
            messages.clear();
        }
        let exit_decision = self
            .pending_settings_exit
            .as_ref()
            .and_then(|_| settings::confirm_exit(&context));
        let close_decision = self
            .pending_window_close
            .then(|| close::confirm(&context))
            .flatten();
        let has_messages = !messages.is_empty();
        for message in messages {
            self.dispatch(message, &context);
        }
        if let Some(decision) = exit_decision {
            self.resolve_settings_exit(decision, &context);
        }
        if let Some(decision) = close_decision {
            self.resolve_window_close(decision, &context);
        }
        if has_messages {
            context.request_repaint();
        }
    }
}

fn key(key: Key, command: bool) -> KeyPress {
    KeyPress {
        key,
        command,
        other_modifier: false,
    }
}

fn status_view(root: &mut egui::Ui, message: &str) {
    egui::CentralPanel::default().show(root, |ui| {
        theme::centered_group(ui, |ui| {
            ui.heading(egui::RichText::new("Ruston Mail").size(36.0));
            ui.add_space(16.0);
            ui.label(message);
            ui.add_space(12.0);
            ui.spinner();
        });
    });
}

fn email_link_notice(root: &mut egui::Ui, app: &App, messages: &mut Vec<Message>) {
    let count = app.pending_mailto_count();
    if count == 0 && app.mailto_error().is_none() {
        return;
    }
    let reason = if !matches!(app.auth_state(), AuthState::Authenticated { .. }) {
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
    let unread = app
        .mailbox()
        .and_then(|mailbox| mailbox.counts()?.unread(&Folder::INBOX));
    title_label(unread, demo)
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

pub(crate) fn dock_badge_count(app: &App) -> Option<u32> {
    if !app.settings().show_unread_badge {
        return None;
    }
    app.mailbox()
        .and_then(|mailbox| mailbox.counts()?.unread(&Folder::INBOX))
        .filter(|&count| count > 0)
}

#[cfg(target_os = "macos")]
fn set_dock_badge(count: Option<u32>) {
    use objc2_app_kit::NSApplication;
    use objc2_foundation::{MainThreadMarker, NSString};

    if let Some(mtm) = MainThreadMarker::new() {
        let app = NSApplication::sharedApplication(mtm);
        let dock_tile = app.dockTile();
        let label = count.map(|c| NSString::from_str(&c.to_string()));
        dock_tile.setBadgeLabel(label.as_deref());
    }
}

#[cfg(not(target_os = "macos"))]
fn set_dock_badge(_count: Option<u32>) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_link_waits_for_file_settings_and_close_guards_without_losing_the_request() {
        let context = egui::Context::default();
        let mut desktop = DesktopApp::with_context(&context, true, Settings::default());
        desktop.dispatch(
            Message::ReceiveMailto(
                crate::mailto::Request::parse("mailto:team@example.org?body=Hello").unwrap(),
            ),
            &context,
        );
        desktop.file_dialog_open = true;
        desktop.focus_after_activation = true;
        desktop.focus_activated_composer(&context);
        assert!(desktop.focus_after_activation);
        desktop.open_waiting_mailto(&context);
        assert!(desktop.app.compose().is_none());
        desktop.file_dialog_open = false;
        desktop.pending_window_close = true;
        desktop.open_waiting_mailto(&context);
        assert!(desktop.app.compose().is_none());
        desktop.pending_window_close = false;
        desktop.pending_settings_exit = Some(SettingsExit::CloseWindow);
        desktop.open_waiting_mailto(&context);
        assert!(desktop.app.compose().is_none());
        desktop.pending_settings_exit = None;
        desktop.dispatch(Message::ShowSettings(true), &context);
        desktop.open_waiting_mailto(&context);
        assert!(desktop.app.compose().is_none());
        desktop.dispatch(Message::ShowSettings(false), &context);
        desktop.open_waiting_mailto(&context);
        assert_eq!(
            desktop
                .app
                .compose()
                .unwrap()
                .field(crate::app::ComposeField::To),
            "team@example.org"
        );
        assert_eq!(desktop.ui.compact_view, mailbox::CompactView::Reader);
        assert_eq!(desktop.app.pending_mailto_count(), 0);
        desktop.focus_activated_composer(&context);
        assert!(!desktop.focus_after_activation);
    }

    fn native_close(
        desktop: &mut DesktopApp,
        context: &egui::Context,
    ) -> Vec<egui::ViewportCommand> {
        let mut raw = egui::RawInput::default();
        let viewport = raw.viewports.get_mut(&egui::ViewportId::ROOT).unwrap();
        viewport.minimized = Some(true);
        viewport.events.push(egui::ViewportEvent::Close);
        let mut output = context.run_logic(&raw, |context| desktop.check_window_close(context));
        output
            .viewport_commands
            .remove(&egui::ViewportId::ROOT)
            .unwrap_or_default()
    }

    #[test]
    fn main_window_close_requires_explicit_discard_in_both_composer_placements() {
        for placement in [ComposePlacement::ReadingPane, ComposePlacement::Window] {
            let context = egui::Context::default();
            let mut desktop = DesktopApp::with_context(&context, true, Settings::default());
            desktop.dispatch(Message::SetComposePlacement(placement), &context);
            desktop.dispatch(Message::OpenCompose, &context);
            desktop.dispatch(
                Message::ComposeChanged(crate::app::ComposeField::Body, "Keep this draft".into()),
                &context,
            );
            for _ in 0..2 {
                assert!(
                    native_close(&mut desktop, &context)
                        .contains(&egui::ViewportCommand::CancelClose)
                );
                assert!(desktop.pending_window_close);
                assert!(!desktop.window_close_approved);
                assert_eq!(
                    desktop
                        .app
                        .compose()
                        .unwrap()
                        .field(crate::app::ComposeField::Body),
                    "Keep this draft"
                );
            }
            desktop.resolve_window_close(Message::CancelDiscard, &context);
            assert!(!desktop.pending_window_close);
            assert!(desktop.app.compose().is_some());
            native_close(&mut desktop, &context);
            desktop.resolve_window_close(Message::DiscardCompose, &context);
            assert!(desktop.app.compose().is_none());
            assert!(desktop.window_close_approved);
        }
    }

    #[test]
    fn untouched_main_window_closes_but_native_dialogs_block_it() {
        let context = egui::Context::default();
        let mut desktop = DesktopApp::with_context(&context, true, Settings::default());
        desktop.dispatch(Message::OpenCompose, &context);
        desktop.file_dialog_open = true;
        assert!(native_close(&mut desktop, &context).contains(&egui::ViewportCommand::CancelClose));
        assert!(!desktop.window_close_approved);
        assert!(!desktop.pending_window_close);
        desktop.file_dialog_open = false;
        assert!(native_close(&mut desktop, &context).contains(&egui::ViewportCommand::Close));
        assert!(desktop.window_close_approved);
    }

    #[test]
    fn pending_send_cannot_be_discarded_by_a_window_close_decision() {
        let context = egui::Context::default();
        let mut desktop = DesktopApp::with_context(&context, true, Settings::default());
        desktop.dispatch(Message::OpenCompose, &context);
        desktop.dispatch(
            Message::ComposeChanged(crate::app::ComposeField::To, "alex@example.com".into()),
            &context,
        );
        native_close(&mut desktop, &context);
        desktop.dispatch(Message::Send, &context);
        assert!(desktop.app.compose().unwrap().in_flight());
        desktop.resolve_window_close(Message::DiscardCompose, &context);
        assert!(desktop.app.compose().unwrap().in_flight());
        assert!(!desktop.window_close_approved);
        assert!(native_close(&mut desktop, &context).contains(&egui::ViewportCommand::CancelClose));
    }

    #[test]
    fn main_close_confirms_settings_then_preserves_or_discards_the_draft() {
        let (mut desktop, context) = demo_with_settings();
        desktop.dispatch(Message::OpenCompose, &context);
        desktop.dispatch(
            Message::ComposeChanged(crate::app::ComposeField::Body, "Draft".into()),
            &context,
        );
        desktop.dispatch(Message::ShowSettings(true), &context);
        desktop.settings_draft = Some(desktop.app.settings().clone());
        desktop.settings_draft.as_mut().unwrap().confirm_links = false;
        assert!(native_close(&mut desktop, &context).contains(&egui::ViewportCommand::CancelClose));
        assert!(matches!(
            desktop.pending_settings_exit,
            Some(SettingsExit::CloseWindow)
        ));
        assert!(!desktop.pending_window_close);
        desktop.resolve_settings_exit(settings::ExitDecision::KeepEditing, &context);
        assert!(!desktop.window_close_approved);
        assert!(desktop.settings_draft.is_some());
        assert!(desktop.app.compose().is_some());
        native_close(&mut desktop, &context);
        desktop.resolve_settings_exit(settings::ExitDecision::Apply, &context);
        assert!(!desktop.app.settings().confirm_links);
        assert!(desktop.pending_window_close);
        assert!(!desktop.window_close_approved);
        desktop.resolve_window_close(Message::CancelDiscard, &context);
        assert!(desktop.app.compose().is_some());
        native_close(&mut desktop, &context);
        desktop.resolve_window_close(Message::DiscardCompose, &context);
        assert!(desktop.window_close_approved);
    }

    #[test]
    fn main_close_can_discard_unapplied_settings_without_changing_preferences() {
        let (mut desktop, context) = demo_with_settings();
        desktop.settings_draft.as_mut().unwrap().confirm_links = false;
        native_close(&mut desktop, &context);
        desktop.resolve_settings_exit(settings::ExitDecision::Discard, &context);
        assert!(desktop.app.settings().confirm_links);
        assert!(desktop.window_close_approved);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_commands_reuse_settings_guards_and_never_replace_an_existing_draft() {
        let (mut desktop, context) = demo_with_settings();
        desktop.settings_draft.as_mut().unwrap().confirm_links = false;
        desktop.native_action(macos::Action::NewMessage, &context);
        assert!(matches!(
            desktop.pending_settings_exit,
            Some(SettingsExit::Navigate(ref message)) if matches!(**message, Message::OpenCompose)
        ));
        assert!(desktop.app.compose().is_none());
        desktop.resolve_settings_exit(settings::ExitDecision::Discard, &context);
        let draft = desktop.app.compose().unwrap().id();
        desktop.native_action(macos::Action::NewMessage, &context);
        assert_eq!(desktop.app.compose().unwrap().id(), draft);
        desktop.native_action(macos::Action::Settings, &context);
        assert!(desktop.app.showing_settings());
        desktop.native_action(macos::Action::Settings, &context);
        assert!(desktop.app.showing_settings());
        desktop.dispatch(Message::Logout, &context);
        desktop.native_action(macos::Action::NewMessage, &context);
        assert!(desktop.app.compose().is_none());
    }

    #[test]
    fn changing_folders_releases_a_superseded_poll_even_without_its_response() {
        let context = egui::Context::default();
        let mut desktop = DesktopApp::with_context(&context, true, Settings::default());
        let demo = crate::mail::demo::DemoMailbox::new();
        desktop.dispatch(
            Message::ConversationsLoaded(
                1,
                demo.list_conversations(&Folder::INBOX, 0, 50, crate::mail::demo::now()),
            ),
            &context,
        );
        assert!(!desktop.auto_refresh.should_start(0.0, true, true, true));
        desktop.dispatch(Message::AutoRefreshMailbox, &context);
        let crate::app::ListStatus::Refreshing(request) = desktop.app.mailbox().unwrap().status()
        else {
            panic!("expected refresh");
        };
        desktop.auto_refresh.started(request);

        let sent = Folder::System(crate::mail::MailFolder::Sent);
        desktop.dispatch(Message::SelectFolder(sent), &context);
        // The old result need never arrive for the scheduler to recover.
        assert!(desktop.auto_refresh.repaint_after(0.0, true).is_some());
        assert!(desktop.auto_refresh.should_start(60.0, true, true, true));
    }

    #[test]
    fn stale_pages_do_not_reset_polling_backoff() {
        let context = egui::Context::default();
        let mut desktop = DesktopApp::with_context(&context, true, Settings::default());
        assert!(!desktop.auto_refresh.should_start(0.0, true, true, true));
        assert!(desktop.auto_refresh.should_start(60.0, true, true, true));
        desktop.auto_refresh.started(7);
        desktop.auto_refresh.page_finished(7, false, 61.0);
        let page = crate::mail::demo::DemoMailbox::new().list_conversations(
            &Folder::INBOX,
            0,
            50,
            crate::mail::demo::now(),
        );
        desktop.dispatch(Message::ConversationsLoaded(99, page), &context);
        assert!(!desktop.auto_refresh.should_start(100.0, true, true, true));
        assert!(desktop.auto_refresh.should_start(181.0, true, true, true));
    }

    #[test]
    fn file_dialog_slot_blocks_duplicates_and_releases_after_a_stale_cancellation() {
        let context = egui::Context::default();
        let mut desktop = DesktopApp::with_context(&context, true, Settings::default());
        desktop.dispatch(Message::OpenCompose, &context);
        let draft = desktop.app.compose().unwrap().id();
        // Model an already-open native picker without opening a GUI in CI.
        desktop.file_dialog_open = true;
        desktop.dispatch(Message::PickComposeAttachments, &context);
        desktop.dispatch(
            Message::SaveAttachmentAs("message".into(), "file".into(), "report.pdf".into()),
            &context,
        );
        assert!(desktop.file_dialog_open);
        assert!(desktop.app.saving_attachment().is_none());
        desktop.dispatch(Message::CloseCompose, &context);
        assert!(desktop.app.compose().is_none());
        desktop.dispatch(Message::AddComposeAttachments(draft, Vec::new()), &context);
        assert!(!desktop.file_dialog_open);
        assert!(desktop.app.compose().is_none());
    }

    #[test]
    fn compact_navigation_follows_open_search_and_compose_without_losing_a_draft() {
        let context = egui::Context::default();
        let mut desktop = DesktopApp::with_context(&context, true, Settings::default());
        let page = crate::mail::demo::DemoMailbox::new().list_conversations(
            &Folder::INBOX,
            0,
            crate::mail::demo::PAGE_SIZE,
            crate::mail::demo::now(),
        );
        let id = page.as_ref().unwrap().conversations[0].id.clone();
        desktop.dispatch(Message::ConversationsLoaded(1, page), &context);
        desktop.dispatch(Message::SelectConversation(id.clone()), &context);
        assert_eq!(desktop.ui.compact_view, mailbox::CompactView::Reader);
        desktop.ui.compact_view = mailbox::CompactView::Mail;
        desktop.dispatch(Message::SelectConversation(id), &context);
        assert_eq!(desktop.ui.compact_view, mailbox::CompactView::Reader);
        desktop.dispatch(
            Message::KeyPressed(key(Key::Character('f'), true)),
            &context,
        );
        assert_eq!(desktop.ui.compact_view, mailbox::CompactView::Mail);
        assert!(desktop.ui.focus_search);
        desktop.dispatch(
            Message::KeyPressed(key(Key::Character('n'), true)),
            &context,
        );
        assert_eq!(desktop.ui.compact_view, mailbox::CompactView::Reader);
        desktop.dispatch(
            Message::ComposeChanged(crate::app::ComposeField::Body, "Keep this draft".into()),
            &context,
        );
        desktop.ui.compact_view = mailbox::CompactView::Mail;
        desktop.dispatch(Message::SelectFolder(Folder::INBOX), &context);
        assert_eq!(
            desktop
                .app
                .compose()
                .unwrap()
                .field(crate::app::ComposeField::Body),
            "Keep this draft"
        );
        desktop.dispatch(Message::ShowSettings(true), &context);
        desktop.settings_draft = Some(desktop.app.settings().clone());
        desktop.settings_draft.as_mut().unwrap().confirm_links = false;
        desktop.dispatch(Message::ShowSettings(false), &context);
        assert!(desktop.app.showing_settings());
        assert!(desktop.pending_settings_exit.is_some());
        desktop.resolve_settings_exit(settings::ExitDecision::KeepEditing, &context);
        assert!(desktop.app.showing_settings());
        assert_eq!(
            desktop
                .app
                .compose()
                .unwrap()
                .field(crate::app::ComposeField::Body),
            "Keep this draft"
        );
    }

    fn demo_with_settings() -> (DesktopApp, egui::Context) {
        let context = egui::Context::default();
        let mut desktop = DesktopApp::with_context(&context, true, Settings::default());
        desktop.dispatch(Message::ShowSettings(true), &context);
        desktop.settings_draft = Some(desktop.app.settings().clone());
        (desktop, context)
    }

    #[test]
    fn sign_out_clears_the_saved_conversation_viewport() {
        let context = egui::Context::default();
        let mut desktop = DesktopApp::with_context(&context, true, Settings::default());
        let page = crate::mail::demo::DemoMailbox::new().list_conversations(
            &Folder::INBOX,
            0,
            crate::mail::demo::PAGE_SIZE,
            crate::mail::demo::now(),
        );
        desktop.dispatch(Message::ConversationsLoaded(1, page), &context);
        context
            .run_ui(egui::RawInput::default(), |root| {
                mailbox::show(
                    root,
                    &desktop.app,
                    None,
                    false,
                    None,
                    &mut desktop.ui,
                    &mut Vec::new(),
                );
            })
            .drop_without_applying_deltas();
        assert!(desktop.ui.conversation_scroll.is_some());

        desktop.dispatch(Message::Logout, &context);
        assert!(desktop.app.mailbox().is_none());
        assert!(desktop.ui.conversation_scroll.is_none());
    }

    #[test]
    fn unchanged_settings_close_with_escape_or_a_folder() {
        let (mut desktop, context) = demo_with_settings();
        desktop.dispatch(Message::KeyPressed(key(Key::Escape, false)), &context);
        assert!(!desktop.app.showing_settings());
        assert!(desktop.pending_settings_exit.is_none());

        let (mut desktop, context) = demo_with_settings();
        desktop.dispatch(Message::SelectFolder(Folder::INBOX), &context);
        assert!(!desktop.app.showing_settings());
        assert_eq!(desktop.app.mailbox().unwrap().folder(), &Folder::INBOX);
        assert!(desktop.pending_settings_exit.is_none());
    }

    #[test]
    fn escape_and_command_comma_confirm_dirty_settings() {
        let (mut desktop, context) = demo_with_settings();
        desktop.settings_draft.as_mut().unwrap().confirm_links = false;

        desktop.dispatch(Message::KeyPressed(key(Key::Escape, false)), &context);
        assert!(desktop.app.showing_settings());
        assert!(matches!(
            desktop.pending_settings_exit,
            Some(SettingsExit::Navigate(ref message)) if matches!(**message, Message::ShowSettings(false))
        ));
        desktop.resolve_settings_exit(settings::ExitDecision::KeepEditing, &context);

        desktop.dispatch(
            Message::KeyPressed(key(Key::Character(','), true)),
            &context,
        );
        assert!(matches!(
            desktop.pending_settings_exit,
            Some(SettingsExit::Navigate(ref message)) if matches!(**message, Message::ShowSettings(false))
        ));
        desktop.resolve_settings_exit(settings::ExitDecision::Discard, &context);
        assert!(!desktop.app.showing_settings());
        assert!(desktop.app.settings().confirm_links);
    }

    #[test]
    fn applying_settings_keeps_the_pane_open_and_allows_a_clean_exit() {
        let (mut desktop, context) = demo_with_settings();
        desktop.settings_draft.as_mut().unwrap().confirm_links = false;
        let changes = settings::preference_changes(
            desktop.app.settings(),
            desktop.settings_draft.as_ref().unwrap(),
        );
        for change in changes {
            desktop.dispatch(change, &context);
        }
        assert!(desktop.app.showing_settings());
        assert!(!desktop.app.settings().confirm_links);

        desktop.dispatch(Message::KeyPressed(key(Key::Escape, false)), &context);
        assert!(!desktop.app.showing_settings());
        assert!(desktop.pending_settings_exit.is_none());
    }

    #[test]
    fn dirty_settings_keep_the_requested_folder_until_a_decision() {
        let (mut desktop, context) = demo_with_settings();
        desktop.settings_draft.as_mut().unwrap().confirm_links = false;
        let original = desktop.app.mailbox().unwrap().folder().clone();
        let sent = Folder::System(crate::mail::MailFolder::Sent);

        desktop.dispatch(Message::SelectFolder(sent.clone()), &context);
        assert!(desktop.app.showing_settings());
        assert_eq!(desktop.app.mailbox().unwrap().folder(), &original);
        assert!(matches!(
            desktop.pending_settings_exit,
            Some(SettingsExit::Navigate(ref message)) if matches!(**message, Message::SelectFolder(_))
        ));

        desktop.dispatch(Message::KeyPressed(key(Key::Escape, false)), &context);
        assert!(matches!(
            desktop.pending_settings_exit,
            Some(SettingsExit::Navigate(ref message)) if matches!(**message, Message::SelectFolder(_)),
        ));
        desktop.resolve_settings_exit(settings::ExitDecision::KeepEditing, &context);
        assert!(desktop.app.showing_settings());
        assert!(desktop.pending_settings_exit.is_none());

        desktop.dispatch(Message::SelectFolder(sent.clone()), &context);
        desktop.resolve_settings_exit(settings::ExitDecision::Apply, &context);
        assert!(!desktop.app.showing_settings());
        assert_eq!(desktop.app.mailbox().unwrap().folder(), &sent);
        assert!(!desktop.app.settings().confirm_links);
        assert!(desktop.settings_draft.is_none());
    }

    #[test]
    fn dirty_settings_can_be_discarded_before_composing_or_signing_out() {
        let (mut desktop, context) = demo_with_settings();
        desktop.settings_draft.as_mut().unwrap().appearance = crate::settings::Appearance::Light;
        desktop.dispatch(Message::OpenCompose, &context);
        assert!(desktop.app.compose().is_none());
        desktop.resolve_settings_exit(settings::ExitDecision::Discard, &context);
        assert!(!desktop.app.showing_settings());
        assert!(desktop.app.compose().is_some());
        assert_eq!(
            desktop.app.settings().appearance,
            crate::settings::Appearance::Dark
        );

        let (mut desktop, context) = demo_with_settings();
        desktop.settings_draft.as_mut().unwrap().confirm_links = false;
        desktop.dispatch(Message::Logout, &context);
        assert!(matches!(
            desktop.app.auth_state(),
            AuthState::Authenticated { .. }
        ));
        desktop.resolve_settings_exit(settings::ExitDecision::Discard, &context);
        assert!(matches!(desktop.app.auth_state(), AuthState::SignedOut));
        assert!(desktop.app.settings().confirm_links);
    }

    #[test]
    fn the_title_carries_unread_mail_only_when_there_is_some() {
        assert_eq!(title_label(Some(3), false), "Ruston Mail (3)");
        assert_eq!(title_label(Some(0), false), "Ruston Mail");
        assert_eq!(title_label(None, false), "Ruston Mail");
        assert_eq!(title_label(Some(3), true), "Ruston Mail (demo) (3)");
    }

    #[test]
    fn dock_badge_count_returns_none_when_unauthenticated() {
        let (app, _) = App::boot(false, crate::settings::Settings::default());
        assert_eq!(dock_badge_count(&app), None);
    }

    #[test]
    fn set_dock_badge_does_not_panic() {
        set_dock_badge(Some(5));
        set_dock_badge(None);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn main_viewport_enables_macos_fullsize_content_view() {
        let settings = Settings::default();
        let viewport = main_viewport(&settings);
        assert_eq!(viewport.fullsize_content_view, Some(true));
        assert_eq!(viewport.titlebar_shown, Some(false));
        assert_eq!(viewport.title_shown, Some(false));
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn main_viewport_leaves_system_decorations_intact() {
        let settings = Settings::default();
        let viewport = main_viewport(&settings);
        assert_eq!(viewport.fullsize_content_view, None);
        assert_eq!(viewport.titlebar_shown, None);
        assert_eq!(viewport.title_shown, None);
    }

    #[test]
    fn mailbox_and_composer_share_the_desktop_identity_and_icon() {
        let settings = Settings::default();
        let (mut app, _) = App::boot(true, settings.clone());
        app.update(Message::OpenCompose);
        let main = main_viewport(&settings);
        let composer = compose::window_viewport(app.compose().unwrap());
        assert_eq!(main.app_id.as_deref(), Some(identity::APP_ID));
        assert_eq!(composer.app_id, main.app_id);
        assert!(std::sync::Arc::ptr_eq(
            main.icon.as_ref().unwrap(),
            composer.icon.as_ref().unwrap(),
        ));
    }

    #[test]
    fn dispatch_clears_password_visibility_on_submit_cancel_or_other_auth_step() {
        let context = egui::Context::default();
        let mut desktop = DesktopApp::with_context(&context, true, Settings::default());

        desktop.ui.show_password = true;
        desktop.dispatch(
            Message::SetAppearance(crate::settings::Appearance::Light),
            &context,
        );
        assert!(!desktop.ui.show_password);

        desktop.app = App::signed_out();
        desktop.ui.show_password = true;
        desktop.dispatch(
            Message::SetAppearance(crate::settings::Appearance::Dark),
            &context,
        );
        assert!(desktop.ui.show_password);

        desktop.dispatch(Message::Submit, &context);
        assert!(matches!(desktop.app.auth_state(), AuthState::SignedOut));
        assert!(!desktop.ui.show_password);

        desktop.ui.show_password = true;
        desktop.dispatch(Message::CancelChallenge, &context);
        assert!(!desktop.ui.show_password);
    }

    #[test]
    fn execute_only_requests_repaint_for_visual_effects() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let check_effect = |effect: UiEffect, should_repaint: bool| {
            let boot_ctx = egui::Context::default();
            let mut desktop = DesktopApp::with_context(&boot_ctx, true, Settings::default());
            desktop.runtime.attach(&egui::Context::default());

            let ctx = egui::Context::default();
            let repainted = Arc::new(AtomicBool::new(false));
            let r = repainted.clone();
            ctx.set_request_repaint_callback(move |_info| {
                r.store(true, Ordering::SeqCst);
            });

            desktop.execute(Effects::ui(effect), &ctx);
            assert_eq!(repainted.load(Ordering::SeqCst), should_repaint);
        };

        // Non-visual effects must not request repaint.
        check_effect(UiEffect::CopyText("test".into()), false);
        check_effect(
            UiEffect::NotifyNewMail {
                sender: "a".into(),
                subject: "b".into(),
            },
            false,
        );

        // Visual effects must request repaint.
        check_effect(UiEffect::FocusSearch, true);
        check_effect(UiEffect::ScrollReaderTop, true);
        check_effect(UiEffect::RevealConversation("123".into()), true);

        // Empty effects must not request repaint.
        let boot_ctx = egui::Context::default();
        let mut desktop = DesktopApp::with_context(&boot_ctx, true, Settings::default());
        desktop.runtime.attach(&egui::Context::default());

        let ctx = egui::Context::default();
        let repainted = Arc::new(AtomicBool::new(false));
        let r = repainted.clone();
        ctx.set_request_repaint_callback(move |_info| {
            r.store(true, Ordering::SeqCst);
        });
        desktop.execute(Effects::none(), &ctx);
        assert!(!repainted.load(Ordering::SeqCst));
    }

    #[test]
    fn remember_window_size_deduplicates_unchanged_dimensions() {
        let ctx = egui::Context::default();
        let mut raw_input = egui::RawInput::default();
        raw_input
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .inner_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1000.0, 700.0),
        ));

        let mut output = ctx.run_ui(raw_input, |_| {});
        output.textures_delta.clear();

        let mut desktop = DesktopApp::with_context(&ctx, true, Settings::default());
        assert_eq!(desktop.last_window_size, None);

        // First pass: records the window size and updates last_window_size.
        desktop.remember_window_size(&ctx);
        assert_eq!(
            desktop.last_window_size,
            Some(Window {
                width: 1000.0,
                height: 700.0,
            })
        );

        // Simulate subsequent idle frames where window size has not changed.
        // We track settings revision to verify no new WindowResized messages are dispatched.
        let revision_before = desktop.app.settings_revision();
        for _ in 0..10 {
            desktop.remember_window_size(&ctx);
        }
        assert_eq!(desktop.app.settings_revision(), revision_before);

        // When the window is resized, a new message is dispatched and settings updated.
        let mut resized_input = egui::RawInput::default();
        resized_input
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .inner_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1200.0, 800.0),
        ));
        let mut output2 = ctx.run_ui(resized_input, |_| {});
        output2.textures_delta.clear();

        desktop.remember_window_size(&ctx);
        assert_eq!(
            desktop.last_window_size,
            Some(Window {
                width: 1200.0,
                height: 800.0,
            })
        );
        assert_eq!(desktop.app.settings().window.width, 1200.0);
        assert_eq!(desktop.app.settings().window.height, 800.0);
        assert!(desktop.app.settings_revision() > revision_before);
    }

    #[test]
    fn reset_layout_handles_window_resize_and_rejection() {
        fn input(size: egui::Vec2) -> egui::RawInput {
            let mut input = egui::RawInput::default();
            input
                .viewports
                .entry(egui::ViewportId::ROOT)
                .or_default()
                .inner_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size));
            input
        }

        let context = egui::Context::default();
        let old = Window {
            width: 1_300.0,
            height: 900.0,
        };
        context
            .run_ui(input(egui::vec2(old.width, old.height)), |_| {})
            .drop_without_applying_deltas();
        let mut settings = Settings::default();
        settings.window = old;
        let mut desktop = DesktopApp::with_context(&context, true, settings);
        desktop.remember_window_size(&context);

        let panel_ids = ["mailbox-sidebar", "conversation-list"].map(egui::Id::new);
        context.data_mut(|data| {
            for id in panel_ids {
                data.insert_persisted(
                    id,
                    egui::containers::panel::PanelState {
                        outer_rect: egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(350.0, 900.0),
                        ),
                    },
                );
            }
        });

        desktop.dispatch(Message::ResetLayout, &context);
        desktop.dispatch(
            Message::PanelsResized(crate::settings::Panels {
                sidebar: 0.35,
                conversations: 0.6,
            }),
            &context,
        );
        desktop.remember_window_size(&context);
        assert_eq!(desktop.app.settings().window, Window::default());
        assert_eq!(desktop.app.panels(), crate::settings::Panels::default());
        assert!(desktop.layout_reset_at.is_some());
        for id in panel_ids {
            assert!(egui::containers::panel::PanelState::load(&context, id).is_some());
        }

        let default = Window::default();
        context
            .run_ui(input(egui::vec2(default.width, default.height)), |_| {})
            .drop_without_applying_deltas();
        desktop.remember_window_size(&context);
        assert_eq!(desktop.layout_reset_at, None);
        assert_eq!(desktop.app.settings().window, default);
        for id in panel_ids {
            assert!(egui::containers::panel::PanelState::load(&context, id).is_none());
        }

        // If the window manager refuses a later resize, keep its actual size.
        context
            .run_ui(input(egui::vec2(old.width, old.height)), |_| {})
            .drop_without_applying_deltas();
        desktop.remember_window_size(&context);
        desktop.dispatch(Message::ResetLayout, &context);
        let mut denied = input(egui::vec2(old.width, old.height));
        denied.time = Some(desktop.layout_reset_at.unwrap() + LAYOUT_RESET_WAIT + 1.0);
        context
            .run_ui(denied, |_| {})
            .drop_without_applying_deltas();
        desktop.remember_window_size(&context);
        assert_eq!(desktop.layout_reset_at, None);
        assert_eq!(desktop.app.settings().window, old);
    }

    #[test]
    fn reset_layout_requests_the_default_window_size_at_current_zoom() {
        let context = egui::Context::default();
        let mut settings = Settings::default();
        settings.zoom = 2.0;
        let mut desktop = DesktopApp::with_context(&context, true, settings);
        context.set_zoom_factor(2.0);
        context
            .run_ui(egui::RawInput::default(), |_| {})
            .drop_without_applying_deltas();

        let output = context.run_ui(egui::RawInput::default(), |_| {
            desktop.dispatch(Message::ResetLayout, &context);
        });
        let expected = egui::vec2(
            Window::default().width / 2.0,
            Window::default().height / 2.0,
        );
        let requested = output.viewport_output[&egui::ViewportId::ROOT]
            .commands
            .iter()
            .any(|command| {
                matches!(command, egui::ViewportCommand::InnerSize(size) if *size == expected)
            });
        output.drop_without_applying_deltas();
        assert!(requested);
    }
}
