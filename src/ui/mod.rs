mod login;

use iced::widget::{button, column, container, row, text};
use iced::{Element, Fill, Size};

use crate::app::{App, AuthState, Message};

const WINDOW_SIZE: Size = Size::new(1_100.0, 700.0);
const SIDEBAR_WIDTH: f32 = 240.0;
const SIDEBAR_PADDING: f32 = 28.0;
const CONTENT_PADDING: f32 = 48.0;
const CONTENT_SPACING: f32 = 20.0;

pub fn run() -> iced::Result {
    iced::application(App::boot, App::update, view)
        .title("Ruston")
        .window_size(WINDOW_SIZE)
        .resizable(true)
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
    let sidebar = container(
        column![
            text("Ruston").size(32),
            text("Mail").size(18),
            text("Connected to Proton Mail").size(14),
        ]
        .spacing(CONTENT_SPACING),
    )
    .width(SIDEBAR_WIDTH)
    .height(Fill)
    .padding(SIDEBAR_PADDING)
    .style(container::dark);

    let account = email
        .map(|email| text(email).size(16))
        .unwrap_or_else(|| text("Proton Mail account").size(16));
    let logout = button(text(if signing_out {
        "Signing out…"
    } else {
        "Sign out"
    }))
    .on_press_maybe((!signing_out).then_some(Message::Logout));

    let mut content = column![
        text("Welcome to Ruston").size(30),
        account,
        container(
            column![
                text("Authentication complete").size(20),
                text("Mailbox support is coming next."),
            ]
            .spacing(12),
        )
        .padding(24)
        .style(container::rounded_box),
        logout,
    ]
    .spacing(CONTENT_SPACING);

    if let Some(error) = app.error_message() {
        content = content.push(text(error).size(14));
    }

    let content = container(content)
        .width(Fill)
        .height(Fill)
        .padding(CONTENT_PADDING);

    row![sidebar, content].into()
}
