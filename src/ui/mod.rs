mod auto_refresh;
mod compose;
mod login;
mod mailbox;
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

#[derive(Default)]
pub(super) struct UiState {
    focus_search: bool,
    scroll_reader_top: bool,
    reveal_conversation: Option<String>,
    undo_notice: Option<UndoNotice>,
}

struct UndoNotice {
    offer: UndoMove,
    expires_at: f64,
}

pub(crate) fn main_viewport(settings: &Settings) -> egui::ViewportBuilder {
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([settings.window.width, settings.window.height])
        .with_min_inner_size(MIN_WINDOW_SIZE);

    #[cfg(target_os = "macos")]
    {
        viewport = viewport
            .with_fullsize_content_view(true)
            .with_titlebar_shown(false)
            .with_title_shown(false);
    }

    viewport
}

pub fn run(demo: bool) -> eframe::Result {
    let settings = Settings::load();
    let options = eframe::NativeOptions {
        viewport: main_viewport(&settings),
        ..Default::default()
    };

    eframe::run_native(
        "Ruston Mail",
        options,
        Box::new(move |creation| Ok(Box::new(DesktopApp::new(creation, demo, settings)))),
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
    /// Preferences being edited in the settings window until Apply is pressed.
    settings_draft: Option<Settings>,
    /// Revision and time used to debounce settings writes.
    settings_seen: u64,
    settings_seen_at: f64,
}

impl DesktopApp {
    fn new(creation: &eframe::CreationContext<'_>, demo: bool, settings: Settings) -> Self {
        theme::install(&creation.egui_ctx);
        let runtime = Runtime::new().expect("failed to start background runtime");
        runtime.attach(&creation.egui_ctx);
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
            settings_seen: 0,
            settings_seen_at: 0.0,
        };
        desktop.execute(effects, &creation.egui_ctx);
        desktop
    }

    fn dispatch(&mut self, message: Message, context: &egui::Context) {
        let now = context.input(|input| input.time);
        if matches!(&message, Message::RefreshMailbox) {
            self.auto_refresh.postpone(now);
        }
        let page = match &message {
            Message::ConversationsLoaded(request, result) => Some((*request, result.is_ok())),
            _ => None,
        };
        let effects = self.app.update(message);
        self.execute(effects, context);
        if let Some((request, succeeded)) = page {
            self.auto_refresh.page_finished(request, succeeded, now);
        }
    }

    fn execute(&mut self, effects: Effects, context: &egui::Context) {
        for effect in self.runtime.execute(effects) {
            match effect {
                UiEffect::CopyText(text) => context.copy_text(text),
                UiEffect::FocusSearch => self.ui.focus_search = true,
                UiEffect::ScrollReaderTop => self.ui.scroll_reader_top = true,
                UiEffect::RevealConversation(id) => self.ui.reveal_conversation = Some(id),
                UiEffect::NotifyNewMail { sender, subject } => {
                    show_desktop_notification(&sender, &subject);
                }
            }
        }
        context.request_repaint();
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
        self.dispatch(
            Message::WindowResized(Window {
                width: unscaled.x,
                height: unscaled.y,
            }),
            context,
        );
    }

    fn keyboard_shortcuts(&mut self, context: &egui::Context) {
        if !matches!(self.app.auth_state(), AuthState::Authenticated { .. }) {
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
            || self.app.showing_settings()
                && context.input_for(settings::viewport_id(), |input| input.focused)
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
}

impl eframe::App for DesktopApp {
    fn logic(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.runtime.attach(context);
        self.apply_zoom(context);
        self.drain_runtime(context);
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
        self.app.save_settings();
        set_dock_badge(None);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let context = ui.ctx().clone();
        let mut messages = Vec::new();
        match self.app.auth_state() {
            AuthState::CheckingSession => status_view(ui, "Opening Ruston Mail…"),
            AuthState::SignedOut
            | AuthState::SigningIn(_)
            | AuthState::NeedsTotp
            | AuthState::NeedsMailboxPassword
            | AuthState::NeedsHumanVerification { .. } => {
                login::show(ui, &mut self.app, &mut messages);
            }
            AuthState::Authenticated { email } => {
                if self.app.mailbox().is_some() {
                    mailbox::show(
                        ui,
                        &self.app,
                        email.as_deref(),
                        false,
                        &mut self.ui,
                        &mut messages,
                    );
                } else {
                    status_view(ui, "Opening mailbox…");
                }

                if self.app.showing_settings() {
                    let draft = self
                        .settings_draft
                        .get_or_insert_with(|| self.app.settings().clone());
                    messages.extend(settings::show_window(&context, self.app.settings(), draft));
                } else {
                    self.settings_draft = None;
                }

                if self.app.settings().compose_placement == ComposePlacement::Window
                    && let Some(writing) = self.app.compose()
                {
                    messages.extend(compose::show_window(&context, writing));
                }
            }
            AuthState::SigningOut => {
                if self.app.mailbox().is_some() {
                    mailbox::show(ui, &self.app, None, true, &mut self.ui, &mut messages);
                } else {
                    status_view(ui, "Signing out…");
                }
            }
        }

        for message in messages {
            self.dispatch(message, &context);
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

#[cfg(target_os = "macos")]
#[allow(deprecated)]
fn show_desktop_notification(sender: &str, subject: &str) {
    use objc2_foundation::{NSString, NSUserNotification, NSUserNotificationCenter};

    let center: Option<objc2::rc::Retained<NSUserNotificationCenter>> = unsafe {
        objc2::msg_send![
            objc2::class!(NSUserNotificationCenter),
            defaultUserNotificationCenter
        ]
    };
    if let Some(center) = center {
        let notification = NSUserNotification::new();
        notification.setTitle(Some(&NSString::from_str(sender)));
        notification.setInformativeText(Some(&NSString::from_str(subject)));
        notification.setSoundName(Some(&NSString::from_str(
            "NSUserNotificationDefaultSoundName",
        )));
        center.deliverNotification(&notification);
    }
}

#[cfg(not(target_os = "macos"))]
fn set_dock_badge(_count: Option<u32>) {}

#[cfg(not(target_os = "macos"))]
fn show_desktop_notification(_sender: &str, _subject: &str) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn show_desktop_notification_does_not_panic() {
        show_desktop_notification("Alice", "Hello World");
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
}
