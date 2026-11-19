use eframe::egui;

use super::theme;
use crate::app::{App, AuthState, Message, SignInStep};
use crate::settings::Appearance;

const CARD_WIDTH: f32 = 480.0;
const CARD_PADDING: i8 = 36;
const LOGO_SIZE: f32 = 44.0;
/// The heading of each sign-in step.
const HEADING_SIZE: f32 = 28.0;

pub(super) fn show(
    root: &mut egui::Ui,
    app: &mut App,
    show_password: &mut bool,
    messages: &mut Vec<Message>,
) -> egui::Response {
    // The sizing pass must not change the live password visibility.
    let mut measured_password_visibility = *show_password;
    let response = egui::CentralPanel::default()
        .frame(
            egui::Frame::new()
                .fill(theme::colors(root).login_background)
                .inner_margin(egui::Margin {
                    left: 32,
                    right: 32,
                    top: 0,
                    bottom: 24,
                }),
        )
        .show(root, |ui| {
            let viewport_top = ui.min_rect().top();
            let viewport_height = ui.available_height();
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.add_space(theme::titlebar_inset(ui.ctx()) + 12.0);
                    // The toggle offers the other scheme from the one on
                    // screen, which is the system's while following it.
                    let showing = if ui.visuals().dark_mode {
                        Appearance::Dark
                    } else {
                        Appearance::Light
                    };
                    header(ui, showing, messages);
                    // The invisible sizing pass disables interaction and measures whichever
                    // authentication step is currently visible before placing the group.
                    let mut measure = ui.new_child(
                        egui::UiBuilder::new()
                            .id_salt("login-body-measure")
                            .sizing_pass()
                            .invisible(),
                    );
                    body(
                        &mut measure,
                        app,
                        &mut measured_password_visibility,
                        &mut Vec::new(),
                    );
                    let body_height = measure.min_rect().height();
                    let centered_top = viewport_top + (viewport_height - body_height) * 0.5;
                    ui.add_space((centered_top - ui.cursor().top()).max(24.0));
                    let response = body(ui, app, show_password, messages);
                    ui.add_space(24.0);
                    response
                })
                .inner
        })
        .inner;
    version_overlay(root.ctx());
    response
}

fn version_overlay(context: &egui::Context) -> egui::Response {
    egui::Area::new(egui::Id::new("login-version"))
        .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-12.0, -5.0))
        .interactable(false)
        .fade_in(false)
        .show(context, super::version_label)
        .inner
}

fn body(
    ui: &mut egui::Ui,
    app: &mut App,
    show_password: &mut bool,
    messages: &mut Vec<Message>,
) -> egui::Response {
    ui.scope(|ui| {
        card(ui, |ui| {
            if matches!(app.auth_state(), AuthState::NeedsHumanVerification { .. }) {
                verification(ui, app, messages);
            } else {
                sign_in(ui, app, show_password, messages);
            }
        });
        ui.add_space(36.0);
        footer(ui);
    })
    .response
}

fn header(
    ui: &mut egui::Ui,
    appearance: Appearance,
    messages: &mut Vec<Message>,
) -> egui::Response {
    ui.horizontal(|ui| {
        brand(ui);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            theme_toggle(ui, appearance, messages)
        })
        .inner
    })
    .inner
}

fn theme_toggle(
    ui: &mut egui::Ui,
    appearance: Appearance,
    messages: &mut Vec<Message>,
) -> egui::Response {
    let (icon, label, next) = match appearance {
        Appearance::Dark => (
            egui::include_image!("../../assets/icons/bootstrap/sun.svg"),
            "Switch to light mode",
            Appearance::Light,
        ),
        Appearance::Light => (
            egui::include_image!("../../assets/icons/bootstrap/moon.svg"),
            "Switch to dark mode",
            Appearance::Dark,
        ),
    };
    let response = icon_button(ui, icon, label);
    if response.clicked() {
        messages.push(Message::SetAppearance(next));
    }
    response
}

/// A button that shows an icon and is announced by `label`; it is disabled
/// inside a disabled `ui`.
fn icon_button(ui: &mut egui::Ui, icon: egui::ImageSource<'static>, label: &str) -> egui::Response {
    let image = egui::Image::new(icon)
        .fit_to_exact_size(egui::Vec2::splat(16.0))
        .tint(theme::colors(ui).muted);
    let enabled = ui.is_enabled();
    let response = ui.add(egui::Button::image(image).min_size(theme::ICON_BUTTON_MIN_SIZE));
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    response.on_hover_text(label)
}

/// The card stays horizontally centred while the whole page can scroll at
/// small window sizes or high zoom levels.
fn card(ui: &mut egui::Ui, content: impl FnOnce(&mut egui::Ui)) -> egui::Response {
    let mut frame = theme::card(ui).inner_margin(CARD_PADDING).corner_radius(20);
    if !ui.visuals().dark_mode {
        frame = frame.stroke(egui::Stroke::NONE).shadow(egui::Shadow {
            offset: [0, 6],
            blur: 20,
            spread: 0,
            color: egui::Color32::from_black_alpha(18),
        });
    }
    let inside = (CARD_WIDTH.min(ui.available_width()) - frame.inner_margin.sum().x).max(1.0);
    ui.vertical_centered(|ui| {
        frame
            .show(ui, |ui| {
                ui.set_width(inside);
                ui.with_layout(egui::Layout::top_down(egui::Align::Min), content);
            })
            .response
    })
    .inner
}

fn sign_in(
    ui: &mut egui::Ui,
    app: &mut App,
    show_password: &mut bool,
    messages: &mut Vec<Message>,
) {
    let (step, busy) = match app.auth_state() {
        AuthState::SigningIn(step) => (*step, true),
        AuthState::NeedsTotp => (SignInStep::Totp, false),
        AuthState::NeedsMailboxPassword => (SignInStep::MailboxPassword, false),
        _ => (SignInStep::Credentials, false),
    };

    ui.heading(egui::RichText::new("Sign in").size(HEADING_SIZE).strong());
    ui.label(
        egui::RichText::new("Continue to Ruston Mail with your Proton account.")
            .color(theme::colors(ui).muted),
    );
    ui.add_space(30.0);

    let fields = match step {
        SignInStep::Credentials => {
            let username = edit_field(
                ui,
                "Username or email",
                "name@proton.me",
                app.username_mut(),
                busy,
                messages,
            );
            if username.changed() {
                app.login_edited();
            }
            let password = edit_secret_field(
                ui,
                "Password",
                "Password",
                app.password_mut(),
                show_password,
                busy,
                messages,
            );
            if password.changed() {
                app.login_edited();
            }
            vec![
                (username, app.username_mut().is_empty()),
                (password, app.password_mut().is_empty()),
            ]
        }
        SignInStep::Totp => {
            ui.label("Enter the code from your authenticator app.");
            let field = edit_field(
                ui,
                "Two-factor code",
                "123456",
                app.totp_mut(),
                busy,
                messages,
            );
            if field.changed() {
                app.login_edited();
            }
            vec![(field, app.totp_mut().is_empty())]
        }
        SignInStep::MailboxPassword => {
            ui.label("This account uses a separate mailbox password.");
            let field = edit_secret_field(
                ui,
                "Mailbox password",
                "Mailbox password",
                app.mailbox_password_mut(),
                show_password,
                busy,
                messages,
            );
            if field.changed() {
                app.login_edited();
            }
            vec![(field, app.mailbox_password_mut().is_empty())]
        }
    };
    // A field waiting for a reply cannot take the cursor.
    if !busy {
        focus_on_arrival(ui, step, &fields);
    }

    error(ui, app);
    parked_draft(ui, app);
    // The saved session is still there, and may open now.
    if app.can_retry_saved_session() && ui.button("Try again").clicked() {
        messages.push(Message::RetrySavedSession);
    }
    ui.add_space(12.0);
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
            theme::primary_visuals(ui);
            let button = egui::Button::new((egui::Atom::grow(), submit, egui::Atom::grow()))
                .min_size(egui::vec2(ui.available_width(), 44.0))
                .stroke(egui::Stroke::NONE);
            if busy {
                // Readable while the sign-in runs, yet it takes no clicks
                // and screen readers still hear it as unavailable.
                theme::busy_visuals(ui);
                ui.add(button.sense(egui::Sense::hover())).widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Button, false, submit)
                });
                false
            } else {
                ui.add(button).clicked()
            }
        })
        .inner;
    if submit_clicked {
        messages.push(Message::Submit);
    }
    if step == SignInStep::Credentials {
        ui.add_space(16.0);
        signup_prompt(ui, messages);
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
    ui.heading(
        egui::RichText::new("Verify you are human")
            .size(HEADING_SIZE)
            .strong(),
    );
    ui.add_space(12.0);
    ui.label(
        "Proton wants to confirm this sign-in. Complete the check in your browser, then continue here.",
    );
    ui.label(
        egui::RichText::new("If no page opened, open it again or copy the link into your browser.")
            .small()
            .color(theme::colors(ui).muted),
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
            theme::primary(egui::Button::new("I completed the verification")),
        )
        .clicked()
    {
        messages.push(Message::Submit);
    }
    if ui.button("Back").clicked() {
        messages.push(Message::CancelChallenge);
    }
}

fn edit_field(
    ui: &mut egui::Ui,
    label: &str,
    hint: &str,
    value: &mut String,
    busy: bool,
    messages: &mut Vec<Message>,
) -> egui::Response {
    let name = ui.label(egui::RichText::new(label).strong());
    let mut unseen = String::new();
    let response = ui
        .add_enabled(
            !busy,
            theme::text_field(ui, measured(ui, value, &mut unseen))
                .hint_text(hint)
                .desired_width(f32::INFINITY),
        )
        .labelled_by(name.id);
    keep_no_history(ui.ctx(), response.id);
    if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) && !busy {
        messages.push(Message::Submit);
    }
    ui.add_space(14.0);
    response
}

fn edit_secret_field(
    ui: &mut egui::Ui,
    label: &str,
    hint: &str,
    value: &mut String,
    show_password: &mut bool,
    busy: bool,
    messages: &mut Vec<Message>,
) -> egui::Response {
    let name = ui.label(egui::RichText::new(label).strong());
    let width =
        (ui.available_width() - theme::ICON_BUTTON_MIN_SIZE.x - ui.spacing().item_spacing.x)
            .max(1.0);
    let mut unseen = String::new();
    let response = ui
        .horizontal(|ui| {
            let response = ui
                .add_enabled(
                    !busy,
                    theme::text_field(ui, measured(ui, value, &mut unseen))
                        .hint_text(hint)
                        .password(!*show_password)
                        .desired_width(width),
                )
                .labelled_by(name.id);
            keep_no_history(ui.ctx(), response.id);
            if response.lost_focus()
                && ui.input(|input| input.key_pressed(egui::Key::Enter))
                && !busy
            {
                messages.push(Message::Submit);
            }
            password_toggle(ui, show_password, busy);
            response
        })
        .inner;
    ui.add_space(14.0);
    response
}

/// Remembers which step last gave a field the cursor.
const FOCUSED_STEP: &str = "sign-in-focused-step";

/// Puts the cursor in the first empty field the first time a step shows, or
/// in the last field when all are filled, so typing can start at once.
fn focus_on_arrival(ui: &egui::Ui, step: SignInStep, fields: &[(egui::Response, bool)]) {
    let shown = egui::Id::new(FOCUSED_STEP);
    if ui.is_sizing_pass() || ui.data(|data| data.get_temp::<SignInStep>(shown)) == Some(step) {
        return;
    }
    ui.data_mut(|data| data.insert_temp(shown, step));
    if let Some((field, _)) = fields.iter().find(|(_, empty)| *empty).or(fields.last()) {
        field.request_focus();
        // The field was made before it was given focus; show it focused now.
        ui.ctx().request_repaint();
    }
}

/// Lets the next step, or the same one shown again after an attempt or a
/// visit to the mailbox, put the cursor in its fields.
pub(super) fn forget_focus(context: &egui::Context) {
    context.data_mut(|data| data.remove::<SignInStep>(egui::Id::new(FOCUSED_STEP)));
}

/// What a field shows. The invisible measuring pass only needs its size, and
/// egui copies a field's text each time it lays it out, so that pass gets an
/// empty field instead of what was typed.
fn measured<'a>(ui: &egui::Ui, value: &'a mut String, unseen: &'a mut String) -> &'a mut String {
    if ui.is_sizing_pass() { unseen } else { value }
}

/// Sign-in fields keep no undo history: it would hold what was typed,
/// passwords included, for anyone at the window after signing out.
fn keep_no_history(context: &egui::Context, field: egui::Id) {
    if let Some(mut state) = egui::TextEdit::load_state(context, field) {
        state.clear_undoer();
        egui::TextEdit::store_state(context, field, state);
    }
}

fn password_toggle(ui: &mut egui::Ui, show_password: &mut bool, busy: bool) -> egui::Response {
    let (icon, label) = if *show_password {
        (
            egui::include_image!("../../assets/icons/bootstrap/eye-slash.svg"),
            "Hide password",
        )
    } else {
        (
            egui::include_image!("../../assets/icons/bootstrap/eye.svg"),
            "Show password",
        )
    };
    let response = ui
        .add_enabled_ui(!busy, |ui| icon_button(ui, icon, label))
        .inner;
    if response.clicked() {
        *show_password = !*show_password;
    }
    response
}

fn signup_prompt(ui: &mut egui::Ui, messages: &mut Vec<Message>) -> egui::Rect {
    // A horizontal layout fills the available width, so center the two
    // separate widgets using their measured widths.
    let font = egui::TextStyle::Body.resolve(ui.style());
    let text_width = |text: &str| {
        ui.painter()
            .layout_no_wrap(text.to_owned(), font.clone(), ui.visuals().text_color())
            .size()
            .x
    };
    let gap = ui.spacing().item_spacing.x;
    let content_width = text_width("New to Proton?") + gap + text_width("Create account");
    let inset = ((ui.available_width() - content_width) * 0.5).max(0.0);
    ui.horizontal(|ui| {
        ui.add_space(inset);
        let label = ui.label("New to Proton?");
        let link = ui.link("Create account");
        if link.clicked() {
            messages.push(Message::OpenSignupPage);
        }
        label.rect.union(link.rect)
    })
    .inner
}

fn brand(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.add(
            egui::Image::new(egui::include_image!("../../assets/ui/ruston-mail-128.png"))
                .fit_to_exact_size(egui::Vec2::splat(LOGO_SIZE))
                .alt_text("Ruston Mail logo"),
        );
        ui.label(
            egui::RichText::new("Ruston Mail")
                .size(theme::APP_NAME_SIZE)
                .strong(),
        );
    });
}

fn footer(ui: &mut egui::Ui) -> egui::Rect {
    let color = theme::colors(ui).muted;
    ui.vertical_centered(|ui| {
        ui.label(
            egui::RichText::new("Not affiliated with Proton")
                .size(theme::FOOTNOTE_SIZE)
                .color(color),
        );
        ui.add(
            egui::AtomLayout::new((
                egui::RichText::new("Made with")
                    .size(theme::FOOTNOTE_SIZE)
                    .color(color),
                egui::Image::new(egui::include_image!(
                    "../../assets/icons/bootstrap/heart.svg"
                ))
                .fit_to_exact_size(egui::Vec2::splat(14.0))
                .tint(theme::ACCENT)
                .alt_text("heart"),
                egui::RichText::new("by Luis Cuellar")
                    .size(theme::FOOTNOTE_SIZE)
                    .color(color),
            ))
            .gap(ui.spacing().item_spacing.x),
        )
        .rect
    })
    .inner
}

fn error(ui: &mut egui::Ui, app: &App) {
    if let Some(error) = app.error_message() {
        ui.label(egui::RichText::new(error).color(theme::colors(ui).danger));
    }
}

/// Says that the message being written when Proton ended the session waits
/// for its own account, and that another account's sign-in drops it.
fn parked_draft(ui: &mut egui::Ui, app: &App) {
    if !app.has_parked_draft() {
        return;
    }
    let whom = app.parked_draft_address().map_or_else(
        || "to the same account".to_owned(),
        |address| format!("as {address}"),
    );
    ui.label(format!(
        "Your unsent message comes back when you sign in again {whom}; \
         signing in to another account discards it."
    ));
}

#[cfg(test)]
mod tests;
