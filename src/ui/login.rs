use eframe::egui;

use super::theme;
use crate::app::{App, AuthState, Message, SignInStep};

const CARD_WIDTH: f32 = 480.0;
const CARD_PADDING: i8 = 28;
const LOGO_SIZE: f32 = 44.0;

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
    let frame = theme::card().inner_margin(CARD_PADDING).corner_radius(16);
    let inside = CARD_WIDTH - frame.inner_margin.sum().x;
    theme::centered_group(ui, |ui| {
        frame.show(ui, |ui| {
            ui.set_width(inside);
            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                content(ui);
            });
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
    ui.add_space(24.0);
    ui.heading(egui::RichText::new("Sign in").size(28.0).strong());
    ui.label(
        egui::RichText::new("Continue to Ruston Mail with your Proton account.")
            .color(theme::MUTED),
    );
    ui.add_space(24.0);

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
    let submit_clicked = ui
        .scope(|ui| {
            ui.visuals_mut().widgets.inactive.weak_bg_fill = theme::ACCENT;
            ui.visuals_mut().widgets.hovered.weak_bg_fill = theme::ACCENT_HOVER;
            ui.visuals_mut().widgets.active.weak_bg_fill = theme::ACCENT_HOVER;
            ui.add_enabled(
                !busy,
                egui::Button::new((
                    egui::Atom::grow(),
                    egui::RichText::new(submit).color(egui::Color32::WHITE),
                    egui::Atom::grow(),
                ))
                .min_size(egui::vec2(ui.available_width(), 44.0))
                .stroke(egui::Stroke::NONE),
            )
            .clicked()
        })
        .inner;
    if submit_clicked {
        messages.push(Message::Submit);
    }
    if step == SignInStep::Credentials {
        ui.add_space(12.0);
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), ui.spacing().interact_size.y),
            egui::Layout::left_to_right(egui::Align::Center).with_main_align(egui::Align::Center),
            |ui| {
                ui.label("New to Proton?");
                if ui.link("Create account").clicked() {
                    messages.push(Message::OpenSignupPage);
                }
            },
        );
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
    ui.add_space(24.0);
    ui.heading(
        egui::RichText::new("Verify you are human")
            .size(28.0)
            .strong(),
    );
    ui.add_space(12.0);
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
            [ui.available_width(), 44.0],
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
    ui.label(egui::RichText::new(label).strong());
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
    ui.horizontal(|ui| {
        ui.add(
            egui::Image::new(egui::include_image!(
                "../../assets/macos/ruston-mail-1024.png"
            ))
            .fit_to_exact_size(egui::Vec2::splat(LOGO_SIZE))
            .alt_text("Ruston Mail logo"),
        );
        ui.label(egui::RichText::new("Ruston Mail").size(23.0).strong());
    });
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
        let (small, minimum) = laid_out(egui::vec2(820.0, 480.0));

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
        assert!(
            minimum.contains_rect(small),
            "the card {small:?} left the minimum window {minimum:?}"
        );
        for (axis, card, window) in [
            ("vertically", card.center().y, window.center().y),
            ("horizontally", card.center().x, window.center().x),
        ] {
            assert!(
                (card - window).abs() <= 2.0,
                "the card is not centred {axis}: {card} against {window}"
            );
        }
    }

    #[test]
    fn card_content_starts_at_its_left_padding() {
        let context = egui::Context::default();
        theme::install(&context);
        let mut card_rect = egui::Rect::NOTHING;
        let mut label_rect = egui::Rect::NOTHING;

        context
            .run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1200.0, 800.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    card_rect = card(ui, |ui| {
                        label_rect = ui.label("Sign in").rect;
                    })
                    .rect;
                },
            )
            .drop_without_applying_deltas();

        assert!(
            (label_rect.left() - card_rect.left() - f32::from(CARD_PADDING)).abs() <= 1.0,
            "label {label_rect:?} is not left-aligned in card {card_rect:?}"
        );
    }
}
