mod login;
mod mailbox;
mod reader;

use iced::widget::{column, container, text};
use iced::{Element, Fill, Font, Size, window};

use crate::app::{App, AuthState, Message};

const WINDOW_SIZE: Size = Size::new(1_100.0, 700.0);
const MIN_WINDOW_SIZE: Size = Size::new(820.0, 480.0);

/// A system family with a real bold face. The toolkit's own default family is
/// usually not installed, and its fallback chain can draw bold text in an
/// unrelated monospace face.
#[cfg(target_os = "macos")]
const APP_FONT: Font = Font::with_name("Helvetica Neue");
#[cfg(target_os = "windows")]
const APP_FONT: Font = Font::with_name("Segoe UI");
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const APP_FONT: Font = Font::DEFAULT;

pub fn run(demo: bool) -> iced::Result {
    iced::application(move || App::boot(demo), App::update, view)
        .title(if demo { "Ruston (demo)" } else { "Ruston" })
        .default_font(APP_FONT)
        .window(window::Settings {
            size: WINDOW_SIZE,
            min_size: Some(MIN_WINDOW_SIZE),
            resizable: true,
            ..window::Settings::default()
        })
        .run()
}

fn view(app: &App) -> Element<'_, Message> {
    match app.auth_state() {
        AuthState::CheckingSession => status_view("Opening Ruston…"),
        AuthState::SignedOut
        | AuthState::SigningIn(_)
        | AuthState::NeedsTotp
        | AuthState::NeedsMailboxPassword
        | AuthState::NeedsHumanVerification { .. } => login::view(app),
        AuthState::Authenticated { email } => authenticated_view(app, email.as_deref(), false),
        AuthState::SigningOut => authenticated_view(app, None, true),
    }
}

fn status_view(message: &str) -> Element<'_, Message> {
    container(column![text("Ruston").size(36), text(message)].spacing(16))
        .center(Fill)
        .into()
}

fn authenticated_view<'a>(
    app: &'a App,
    email: Option<&'a str>,
    signing_out: bool,
) -> Element<'a, Message> {
    match app.mailbox() {
        Some(mailbox) => mailbox::view(app, mailbox, email, signing_out),
        None => status_view("Opening mailbox…"),
    }
}
