mod login;
mod mailbox;
mod reader;

use iced::widget::{column, container, text};
use iced::{Element, Fill, Size, window};

use crate::app::{App, AuthState, Message};

const WINDOW_SIZE: Size = Size::new(1_100.0, 700.0);
const MIN_WINDOW_SIZE: Size = Size::new(820.0, 480.0);

pub fn run(demo: bool) -> iced::Result {
    iced::application(move || App::boot(demo), App::update, view)
        .title(if demo { "Ruston (demo)" } else { "Ruston" })
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
        | AuthState::NeedsMailboxPassword => login::view(app),
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
