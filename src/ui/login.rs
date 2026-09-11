use iced::widget::{Column, button, column, container, row, text, text_input};
use iced::{Element, Fill};

use crate::app::{App, AuthState, Message, SignInStep};

const CARD_WIDTH: f32 = 420.0;
const CARD_PADDING: f32 = 32.0;
const FIELD_SPACING: f32 = 18.0;

pub(super) fn view(app: &App) -> Element<'_, Message> {
    if matches!(app.auth_state(), AuthState::NeedsHumanVerification { .. }) {
        return verification_view(app);
    }

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

    centered_card(card)
}

fn verification_view(app: &App) -> Element<'_, Message> {
    let mut card = column![
        text("Ruston").size(36),
        text("Verify you are human").size(24),
        text(
            "Proton wants to confirm this sign-in. Complete the check in your browser, \
             then continue here."
        ),
        text("If no page opened, open it again or copy the link into your browser.").size(14),
        row![
            button(text("Open page")).on_press(Message::OpenVerificationPage),
            button(text("Copy link")).on_press(Message::CopyVerificationLink),
        ]
        .spacing(8),
    ]
    .spacing(FIELD_SPACING);

    if let Some(error) = app.error_message() {
        card = card.push(text(error).size(14));
    }

    centered_card(
        card.push(
            button(text("I completed the verification"))
                .width(Fill)
                .on_press(Message::Submit),
        )
        .push(
            button(text("Back"))
                .width(Fill)
                .on_press(Message::CancelChallenge),
        ),
    )
}

fn centered_card(card: Column<'_, Message>) -> Element<'_, Message> {
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
