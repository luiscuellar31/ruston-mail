use eframe::egui;

use super::theme;
use crate::app::{App, AuthState, Message, SignInStep};

const CARD_WIDTH: f32 = 420.0;

pub(super) fn show(
    root: &mut egui::Ui,
    app: &mut App,
    messages: &mut Vec<Message>,
) -> egui::Response {
    egui::CentralPanel::default()
        .show(root, |ui| {
            card(ui, |ui| {
                if matches!(app.auth_state(), AuthState::NeedsHumanVerification { .. }) {
                    verification(ui, app, messages);
                } else {
                    sign_in(ui, app, messages);
                }
            })
        })
        .inner
}

/// Puts the card in the middle of the window, as tall as what it holds.
/// `centered_and_justified` would stretch the first top-down widget.
fn card(ui: &mut egui::Ui, mut content: impl FnMut(&mut egui::Ui)) -> egui::Response {
    let frame = theme::card();
    let inside = CARD_WIDTH - frame.inner_margin.sum().x;
    theme::centered_group(ui, |ui| {
        frame.show(ui, |ui| {
            ui.set_width(inside);
            content(ui);
        });
    })
}

fn sign_in(ui: &mut egui::Ui, app: &mut App, messages: &mut Vec<Message>) {
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
            let changed = edit_field(
                ui,
                "Username or email",
                "name@proton.me",
                app.username_mut(),
                false,
                busy,
                messages,
            );
            if changed {
                app.login_edited();
            }
            let changed = edit_field(
                ui,
                "Password",
                "Password",
                app.password_mut(),
                true,
                busy,
                messages,
            );
            if changed {
                app.login_edited();
            }
        }
        SignInStep::Totp => {
            ui.label("Enter the code from your authenticator app.");
            let changed = edit_field(
                ui,
                "Two-factor code",
                "123456",
                app.totp_mut(),
                false,
                busy,
                messages,
            );
            if changed {
                app.login_edited();
            }
        }
        SignInStep::MailboxPassword => {
            ui.label("This account uses a separate mailbox password.");
            let changed = edit_field(
                ui,
                "Mailbox password",
                "Mailbox password",
                app.mailbox_password_mut(),
                true,
                busy,
                messages,
            );
            if changed {
                app.login_edited();
            }
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
    let cancelled = if busy {
        ui.button("Cancel").clicked()
    } else {
        step != SignInStep::Credentials && ui.button("Back").clicked()
    };
    if cancelled {
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
    value: &mut String,
    password: bool,
    busy: bool,
    messages: &mut Vec<Message>,
) -> bool {
    ui.label(egui::RichText::new(label).small().color(theme::MUTED));
    let response = ui.add_enabled(
        !busy,
        theme::text_field(value)
            .hint_text(hint)
            .password(password)
            .desired_width(f32::INFINITY),
    );
    if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) && !busy {
        messages.push(Message::Submit);
    }
    ui.add_space(8.0);
    response.changed()
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Lays the sign-in screen out in a window of `size` and answers with the
    /// card's rectangle and the window's.
    fn laid_out(size: egui::Vec2) -> (egui::Rect, egui::Rect) {
        let mut app = App::signed_out();
        let context = egui::Context::default();
        theme::install(&context);
        let window = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
        let mut card = egui::Rect::NOTHING;
        context
            .run_ui(
                egui::RawInput {
                    screen_rect: Some(window),
                    ..Default::default()
                },
                |ui| card = show(ui, &mut app, &mut Vec::new()).rect,
            )
            .drop_without_applying_deltas();

        (card, window)
    }

    #[test]
    fn the_sign_in_card_is_centred_and_only_as_tall_as_it_holds() {
        // The card must fit its contents rather than stretch to the window.
        let (card, window) = laid_out(egui::vec2(1200.0, 800.0));
        let (taller, _) = laid_out(egui::vec2(1200.0, 1400.0));

        assert!(card.height() > 0.0, "the card was never laid out");
        assert!(
            (card.height() - taller.height()).abs() <= 1.0,
            "the card grew with the window: {} against {}",
            card.height(),
            taller.height()
        );
        assert!(
            window.contains_rect(card),
            "the card {card:?} left the window {window:?}"
        );
        for (axis, card, window) in [
            ("vertically", card.center().y, window.center().y),
            ("horizontally", card.center().x, window.center().x),
        ] {
            assert!(
                (card - window).abs() <= 1.0,
                "the card is not centred {axis}: {card} against {window}"
            );
        }
    }
}
