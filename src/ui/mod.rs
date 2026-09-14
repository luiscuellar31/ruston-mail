mod compose;
mod login;
mod mailbox;
mod reader;
mod settings;
mod theme;

use eframe::egui;

use crate::app::{App, AuthState, Effects, Key, KeyPress, Message, UiEffect};
use crate::mail::Folder;
use crate::runtime::Runtime;
use crate::settings::{ComposePlacement, Settings, Window};

const MIN_WINDOW_SIZE: [f32; 2] = [820.0, 480.0];
/// How long the settings have to stay still before they are written. Long
/// enough that a drag is one write rather than dozens, short enough that a
/// process killed rather than closed loses at most this much.
const SETTINGS_QUIET: f32 = 0.75;

#[derive(Default)]
pub(super) struct UiState {
    focus_search: bool,
    scroll_reader_top: bool,
    reveal_conversation: Option<String>,
}

pub fn run(demo: bool) -> eframe::Result {
    let settings = Settings::load();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([settings.window.width, settings.window.height])
            .with_min_inner_size(MIN_WINDOW_SIZE),
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
    /// Preferences being edited in the settings window until Apply is pressed.
    settings_draft: Option<Settings>,
    /// The settings revision last seen, and when it was seen. Writing is held
    /// back until it stops moving: a window edge or a divider being dragged
    /// changes the settings many times a second.
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
            settings_draft: None,
            settings_seen: 0,
            settings_seen_at: 0.0,
        };
        desktop.execute(effects, &creation.egui_ctx);
        desktop
    }

    fn dispatch(&mut self, message: Message, context: &egui::Context) {
        let effects = self.app.update(message);
        self.execute(effects, context);
    }

    fn execute(&mut self, effects: Effects, context: &egui::Context) {
        for effect in self.runtime.execute(effects) {
            match effect {
                UiEffect::CopyText(text) => context.copy_text(text),
                UiEffect::FocusSearch => self.ui.focus_search = true,
                UiEffect::ScrollReaderTop => self.ui.scroll_reader_top = true,
                UiEffect::RevealConversation(id) => self.ui.reveal_conversation = Some(id),
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
        // egui reports the window in the points it lays out in, which the zoom
        // scales. The window is built before the zoom is applied, so it is
        // asked for in unscaled points: the zoom is taken back out here, and
        // what is remembered is the window itself, the same number whatever
        // the interface is scaled to.
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
            }
            presses
        });

        for press in presses {
            self.dispatch(Message::KeyPressed(press), context);
        }
    }

    /// Keeps egui's scale on the chosen zoom. egui measures window sizes in
    /// the same scaled points and undoes the scale when a window is created,
    /// so the remembered size stays the same window whatever the zoom.
    fn apply_zoom(&self, context: &egui::Context) {
        let zoom = self.app.settings().zoom;
        if (context.zoom_factor() - zoom).abs() > f32::EPSILON {
            context.set_zoom_factor(zoom);
        }
    }

    /// Writes the settings once they have been still for [`SETTINGS_QUIET`].
    ///
    /// A repaint is asked for so the frame that does the writing happens at
    /// all: egui sleeps when nothing is going on, and the moment a drag ends
    /// is exactly such a moment.
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
}

impl eframe::App for DesktopApp {
    fn logic(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.runtime.attach(context);
        self.apply_zoom(context);
        self.drain_runtime(context);
        self.remember_window_size(context);
        self.keyboard_shortcuts(context);
        self.save_settled_settings(context);
        self.update_title(context);
    }

    /// Closing is the one moment the settings must reach the file whether or
    /// not they have settled.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.app.save_settings();
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
                login::show(ui, &self.app, &mut messages);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_title_carries_unread_mail_only_when_there_is_some() {
        assert_eq!(title_label(Some(3), false), "Ruston Mail (3)");
        assert_eq!(title_label(Some(0), false), "Ruston Mail");
        assert_eq!(title_label(None, false), "Ruston Mail");
        assert_eq!(title_label(Some(3), true), "Ruston Mail (demo) (3)");
    }
}
