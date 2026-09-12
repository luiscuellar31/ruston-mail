use eframe::egui;

use super::theme;
use crate::app::{App, AuthState, Message, SignInStep};

const CARD_WIDTH: f32 = 420.0;

pub(super) fn show(root: &mut egui::Ui, app: &App, messages: &mut Vec<Message>) {
    egui::CentralPanel::default().show(root, |ui| {
        ui.centered_and_justified(|ui| {
            ui.set_max_width(CARD_WIDTH);
            theme::card().show(ui, |ui| {
                ui.set_width(CARD_WIDTH - 56.0);
                if matches!(app.auth_state(), AuthState::NeedsHumanVerification { .. }) {
                    verification(ui, app, messages);
                } else {
                    sign_in(ui, app, messages);
                }
            });
        });
    });
}

fn sign_in(ui: &mut egui::Ui, app: &App, messages: &mut Vec<Message>) {
    let (step, busy) = match app.auth_state() {
        AuthState::SigningIn(step) => (*step, true),
        AuthState::NeedsTotp => (SignInStep::Totp, false),
        AuthState::NeedsMailboxPassword => (SignInStep::MailboxPassword, false),
        _ => (SignInStep::Credentials, false),
    };

    brand(ui);
    ui.add_space(10.0);
    ui.heading("Sign in to Proton Mail");
    ui.add_space(14.0);

    match step {
        SignInStep::Credentials => {
            edit_field(
                ui,
                "Username or email",
                "name@proton.me",
                app.username(),
                false,
                busy,
                Message::UsernameChanged,
                messages,
            );
            edit_field(
                ui,
                "Password",
                "Password",
                app.password(),
                true,
                busy,
                Message::PasswordChanged,
                messages,
            );
        }
        SignInStep::Totp => {
            ui.label("Enter the code from your authenticator app.");
            edit_field(
                ui,
                "Two-factor code",
                "123456",
                app.totp(),
                false,
                busy,
                Message::TotpChanged,
                messages,
            );
        }
        SignInStep::MailboxPassword => {
            ui.label("This account uses a separate mailbox password.");
            edit_field(
                ui,
                "Mailbox password",
                "Mailbox password",
                app.mailbox_password(),
                true,
                busy,
                Message::MailboxPasswordChanged,
                messages,
            );
        }
    }

    error(ui, app);
    ui.add_space(6.0);
    let submit = if busy {
        "Signing in…"
    } else {
        match step {
            SignInStep::Credentials => "Sign in",
            SignInStep::Totp => "Verify",
            SignInStep::MailboxPassword => "Unlock mailbox",
        }
    };
    if ui
        .add_enabled(
            !busy,
            egui::Button::new(submit).min_size(egui::vec2(ui.available_width(), 36.0)),
        )
        .clicked()
    {
        messages.push(Message::Submit);
    }
    if step != SignInStep::Credentials
        && ui
            .add_enabled(!busy, egui::Button::new("Back").frame(false))
            .clicked()
    {
        messages.push(Message::CancelChallenge);
    }
}

fn verification(ui: &mut egui::Ui, app: &App, messages: &mut Vec<Message>) {
    brand(ui);
    ui.add_space(10.0);
    ui.heading("Verify you are human");
    ui.add_space(10.0);
    ui.label(
        "Proton wants to confirm this sign-in. Complete the check in your browser, then continue here.",
    );
    ui.label(
        egui::RichText::new("If no page opened, open it again or copy the link into your browser.")
            .small()
            .color(theme::MUTED),
    );
    ui.horizontal(|ui| {
        if ui.button("Open page").clicked() {
            messages.push(Message::OpenVerificationPage);
        }
        if ui.button("Copy link").clicked() {
            messages.push(Message::CopyVerificationLink);
        }
    });
    error(ui, app);
    ui.add_space(8.0);
    if ui
        .add_sized(
            [ui.available_width(), 36.0],
            egui::Button::new("I completed the verification"),
        )
        .clicked()
    {
        messages.push(Message::Submit);
    }
    if ui.button("Back").clicked() {
        messages.push(Message::CancelChallenge);
    }
}

#[allow(clippy::too_many_arguments)]
fn edit_field(
    ui: &mut egui::Ui,
    label: &str,
    hint: &str,
    current: &str,
    password: bool,
    busy: bool,
    changed: fn(String) -> Message,
    messages: &mut Vec<Message>,
) {
    ui.label(egui::RichText::new(label).small().color(theme::MUTED));
    let mut value = current.to_owned();
    let response = ui.add_enabled(
        !busy,
        egui::TextEdit::singleline(&mut value)
            .hint_text(hint)
            .password(password)
            .desired_width(f32::INFINITY),
    );
    if response.changed() {
        messages.push(changed(value));
    }
    if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) && !busy {
        messages.push(Message::Submit);
    }
    ui.add_space(8.0);
}

fn brand(ui: &mut egui::Ui) {
    ui.label(
        egui::RichText::new("R")
            .size(24.0)
            .strong()
            .color(theme::ACCENT),
    );
    ui.heading(egui::RichText::new("Ruston Mail").size(32.0));
}

fn error(ui: &mut egui::Ui, app: &App) {
    if let Some(error) = app.error_message() {
        ui.label(egui::RichText::new(error).color(theme::DANGER));
    }
}
