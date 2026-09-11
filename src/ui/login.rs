use iced::widget::{Column, button, column, container, text, text_input};
use iced::{Element, Fill};

use crate::app::{App, AuthState, Message, SignInStep};

const CARD_WIDTH: f32 = 420.0;
const CARD_PADDING: f32 = 32.0;
const FIELD_SPACING: f32 = 18.0;

pub(super) fn view(app: &App) -> Element<'_, Message> {
    let (step, busy) = match app.auth_state() {
        AuthState::SigningIn(step) => (*step, true),
        AuthState::NeedsTotp => (SignInStep::Totp, false),
        AuthState::NeedsMailboxPassword => (SignInStep::MailboxPassword, false),
        _ => (SignInStep::Credentials, false),
    };

    let fields = match step {
        SignInStep::Credentials => credentials_fields(app, busy),
        SignInStep::Totp => totp_fields(app, busy),
        SignInStep::MailboxPassword => mailbox_password_fields(app, busy),
    };
    let submit_label = if busy {
        "Signing in…"
    } else {
        match step {
            SignInStep::Credentials => "Sign in",
            SignInStep::Totp => "Verify",
            SignInStep::MailboxPassword => "Unlock mailbox",
        }
    };
    let submit = button(text(submit_label))
        .width(Fill)
        .on_press_maybe((!busy).then_some(Message::Submit));

    let mut card = column![
        text("Ruston").size(36),
        text("Sign in to Proton Mail").size(24),
        fields,
    ]
    .spacing(FIELD_SPACING);

    if let Some(error) = app.error_message() {
        card = card.push(text(error).size(14));
    }

    card = card.push(submit);
    if step != SignInStep::Credentials {
        card = card.push(
            button(text("Back"))
                .width(Fill)
                .on_press_maybe((!busy).then_some(Message::CancelChallenge)),
        );
    }

    container(
        container(card)
            .width(CARD_WIDTH)
            .padding(CARD_PADDING)
            .style(container::rounded_box),
    )
    .center(Fill)
    .into()
}

fn credentials_fields(app: &App, busy: bool) -> Column<'_, Message> {
    column![
        labeled_input(
            "Username or email",
            "name@proton.me",
            app.username(),
            busy,
            Message::UsernameChanged,
            false,
        ),
        labeled_input(
            "Password",
            "Password",
            app.password(),
            busy,
            Message::PasswordChanged,
            true,
        ),
    ]
    .spacing(FIELD_SPACING)
}

fn totp_fields(app: &App, busy: bool) -> Column<'_, Message> {
    column![
        text("Enter the code from your authenticator app."),
        labeled_input(
            "Two-factor code",
            "123456",
            app.totp(),
            busy,
            Message::TotpChanged,
            false,
        ),
    ]
    .spacing(FIELD_SPACING)
}

fn mailbox_password_fields(app: &App, busy: bool) -> Column<'_, Message> {
    column![
        text("This account uses a separate mailbox password."),
        labeled_input(
            "Mailbox password",
            "Mailbox password",
            app.mailbox_password(),
            busy,
            Message::MailboxPasswordChanged,
            true,
        ),
    ]
    .spacing(FIELD_SPACING)
}

fn labeled_input<'a>(
    label: &'a str,
    placeholder: &'a str,
    value: &'a str,
    busy: bool,
    on_input: fn(String) -> Message,
    secure: bool,
) -> Element<'a, Message> {
    let input = text_input(placeholder, value)
        .secure(secure)
        .on_input_maybe((!busy).then_some(on_input))
        .on_submit_maybe((!busy).then_some(Message::Submit));

    column![text(label).size(14), input].spacing(8).into()
}
