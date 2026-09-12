mod login;
mod mailbox;
mod reader;
mod selectable;
mod settings;

use iced::widget::text::IntoFragment;
use iced::widget::{Text, column, container, text};
use iced::{Element, Fill, Font, Size, window};

use crate::app::{App, AuthState, Message};
use crate::mail::MailFolder;
use crate::settings::Settings;

/// Size of the small print: times, counts, addresses and every other line
/// that supports the one above it.
pub(super) const DETAIL_SIZE: f32 = 12.0;

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
    let settings = Settings::load();
    let size = Size::new(settings.window.width, settings.window.height);

    iced::application(move || App::boot(demo, settings.clone()), App::update, view)
        .title(move |app: &App| window_title(app, demo))
        .subscription(App::subscription)
        .default_font(APP_FONT)
        .window(window::Settings {
            size,
            min_size: Some(MIN_WINDOW_SIZE),
            resizable: true,
            ..window::Settings::default()
        })
        .run()
}

/// The window title, carrying the unread count so it is readable from the
/// dock or task bar. Inbox, not the open folder: it is the one people watch.
fn window_title(app: &App, demo: bool) -> String {
    let unread = app
        .mailbox()
        .and_then(|mailbox| mailbox.counts()?.unread(MailFolder::Inbox));

    title_label(unread, demo)
}

/// An empty inbox says nothing, so the count appears only when it matters.
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

/// Small secondary text. Both panes share it so their detail lines cannot
/// drift apart; callers still choose their own wrapping and width.
pub(super) fn detail_text<'a>(content: impl IntoFragment<'a>) -> Text<'a> {
    text(content).size(DETAIL_SIZE).style(text::secondary)
}

fn view(app: &App) -> Element<'_, Message> {
    match app.auth_state() {
        AuthState::CheckingSession => status_view("Opening Ruston Mail…"),
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
    container(column![text("Ruston Mail").size(36), text(message)].spacing(16))
        .center(Fill)
        .into()
}

fn authenticated_view<'a>(
    app: &'a App,
    email: Option<&'a str>,
    signing_out: bool,
) -> Element<'a, Message> {
    if app.showing_settings() {
        return settings::view(app.settings());
    }

    match app.mailbox() {
        Some(mailbox) => mailbox::view(app, mailbox, email, signing_out),
        None => status_view("Opening mailbox…"),
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
