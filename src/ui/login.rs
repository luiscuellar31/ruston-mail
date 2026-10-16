use eframe::egui;

use super::theme;
use crate::app::{App, AuthState, Message, SignInStep};
use crate::settings::Appearance;

const CARD_WIDTH: f32 = 480.0;
const CARD_PADDING: i8 = 36;
const LOGO_SIZE: f32 = 44.0;

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
                    header(ui, app.settings().appearance, messages);
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
    let response = icon_button(ui, icon, label, true);
    if response.clicked() {
        messages.push(Message::SetAppearance(next));
    }
    response
}

fn icon_button(
    ui: &mut egui::Ui,
    icon: egui::ImageSource<'static>,
    label: &str,
    enabled: bool,
) -> egui::Response {
    let image = egui::Image::new(icon)
        .fit_to_exact_size(egui::Vec2::splat(16.0))
        .tint(theme::colors(ui).muted);
    let response = ui.add_enabled(
        enabled,
        egui::Button::image(image).min_size(theme::ICON_BUTTON_MIN_SIZE),
    );
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

    ui.heading(egui::RichText::new("Sign in").size(28.0).strong());
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
            ui.add_enabled(
                !busy,
                egui::Button::new((egui::Atom::grow(), submit, egui::Atom::grow()))
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
    ui.label(egui::RichText::new(label).strong());
    let mut unseen = String::new();
    let response = ui.add_enabled(
        !busy,
        theme::text_field(ui, measured(ui, value, &mut unseen))
            .hint_text(hint)
            .desired_width(f32::INFINITY),
    );
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
    ui.label(egui::RichText::new(label).strong());
    let width =
        (ui.available_width() - theme::ICON_BUTTON_MIN_SIZE.x - ui.spacing().item_spacing.x)
            .max(1.0);
    let mut unseen = String::new();
    let response = ui
        .horizontal(|ui| {
            let response = ui.add_enabled(
                !busy,
                theme::text_field(ui, measured(ui, value, &mut unseen))
                    .hint_text(hint)
                    .password(!*show_password)
                    .desired_width(width),
            );
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
    let response = icon_button(ui, icon, label, !busy);
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
        ui.label(egui::RichText::new("Ruston Mail").size(23.0).strong());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_uses_login_corner_without_reserving_a_footer() {
        // The smallest viewport matches an 820x480 window at 2x zoom.
        for size in [
            egui::vec2(410.0, 240.0),
            egui::vec2(820.0, 480.0),
            egui::vec2(1200.0, 800.0),
        ] {
            let context = egui::Context::default();
            theme::install(&context);
            let window = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
            let mut version = egui::Rect::NOTHING;
            let mut content = egui::Rect::NOTHING;
            for _ in 0..2 {
                context
                    .run_ui(
                        egui::RawInput {
                            screen_rect: Some(window),
                            ..Default::default()
                        },
                        |root| {
                            content = egui::CentralPanel::default()
                                .show(root, |ui| ui.max_rect())
                                .inner;
                            version = version_overlay(root.ctx()).rect;
                        },
                    )
                    .drop_without_applying_deltas();
            }

            assert!(window.contains_rect(version));
            assert!(window.right() - version.right() <= 20.0);
            assert!(window.bottom() - version.bottom() <= 20.0);
            assert!(window.bottom() - content.bottom() <= 10.0);
        }
    }

    /// Lays out a physical window at the requested zoom and returns the
    /// card-and-footer group's rectangle and the window in egui points.
    fn laid_out(
        physical_size: egui::Vec2,
        appearance: Appearance,
        zoom: f32,
    ) -> (egui::Rect, egui::Rect) {
        let mut app = App::signed_out();
        let _ = app.update(Message::SetAppearance(appearance));
        let _ = app.update(Message::SetZoom(zoom));
        let mut show_password = false;
        let context = egui::Context::default();
        theme::install(&context);
        theme::apply(&context, appearance);
        let physical_window = egui::Rect::from_min_size(egui::Pos2::ZERO, physical_size);
        // egui applies a zoom change on the next pass using the previous viewport.
        context
            .run_ui(
                egui::RawInput {
                    screen_rect: Some(physical_window),
                    ..Default::default()
                },
                |_| {},
            )
            .drop_without_applying_deltas();
        context.set_zoom_factor(zoom);
        let window = egui::Rect::from_min_size(egui::Pos2::ZERO, physical_size / zoom);
        assert_eq!(app.settings().appearance, appearance);
        assert_eq!(
            context.theme(),
            match appearance {
                Appearance::Dark => egui::Theme::Dark,
                Appearance::Light => egui::Theme::Light,
            }
        );
        assert_eq!(app.settings().zoom, zoom);
        let mut card = egui::Rect::NOTHING;
        context
            .run_ui(
                egui::RawInput {
                    screen_rect: Some(window),
                    ..Default::default()
                },
                |ui| {
                    assert_eq!(ui.pixels_per_point(), zoom);
                    card = show(ui, &mut app, &mut show_password, &mut Vec::new()).rect;
                },
            )
            .drop_without_applying_deltas();
        assert_eq!(context.zoom_factor(), zoom);

        (card, window)
    }

    #[test]
    fn the_login_group_is_centered_vertically_and_scrolls_when_space_is_short() {
        let (group, window) = laid_out(egui::vec2(1200.0, 800.0), Appearance::Dark, 1.0);
        let (taller, tall_window) = laid_out(egui::vec2(1200.0, 1400.0), Appearance::Dark, 1.0);
        let (small, minimum) = laid_out(egui::vec2(820.0, 480.0), Appearance::Dark, 1.0);
        let (zoomed, zoomed_window) = laid_out(egui::vec2(820.0, 480.0), Appearance::Dark, 2.0);
        let (light, _) = laid_out(egui::vec2(820.0, 480.0), Appearance::Light, 1.0);

        assert!(group.height() > 0.0, "the login group was never laid out");
        assert!(
            (group.height() - taller.height()).abs() <= 1.0,
            "the login group grew with the window: {} against {}",
            group.height(),
            taller.height()
        );
        for (group, window) in [(group, window), (taller, tall_window)] {
            assert!(
                (group.center().y - window.center().y).abs() <= 12.0,
                "login group {group:?} is not centered vertically in {window:?}"
            );
        }
        for (group, window) in [
            (group, window),
            (small, minimum),
            (zoomed, zoomed_window),
            (light, minimum),
        ] {
            assert!(
                group.left() >= window.left(),
                "login group overflows left: {group:?}"
            );
            assert!(
                group.right() <= window.right(),
                "login group overflows right: {group:?}"
            );
            assert!(
                (group.center().x - window.center().x).abs() <= 12.0,
                "login group is not horizontally centered: {group:?} in {window:?}"
            );
        }
        for (group, window) in [(small, minimum), (zoomed, zoomed_window), (light, minimum)] {
            assert!(group.top() >= window.top());
            assert!(
                group.bottom() > window.bottom(),
                "the login group should scroll when available height is short"
            );
        }
    }

    #[test]
    fn bundled_login_icons_render_and_can_be_tinted() {
        for svg in [
            include_bytes!("../../assets/icons/bootstrap/sun.svg").as_slice(),
            include_bytes!("../../assets/icons/bootstrap/moon.svg").as_slice(),
            include_bytes!("../../assets/icons/bootstrap/eye.svg").as_slice(),
            include_bytes!("../../assets/icons/bootstrap/eye-slash.svg").as_slice(),
            include_bytes!("../../assets/icons/bootstrap/heart.svg").as_slice(),
        ] {
            let image = egui_extras::image::load_svg_bytes(svg, &Default::default())
                .expect("bundled SVG must render");
            assert!(image.pixels.iter().any(|pixel| pixel.a() > 0));
            assert!(image.pixels.iter().any(|pixel| {
                pixel.r() > 0 && pixel.r() == pixel.g() && pixel.g() == pixel.b()
            }));
        }
    }

    #[test]
    fn footer_heart_is_filled() {
        let heart = egui_extras::image::load_svg_bytes(
            include_bytes!("../../assets/icons/bootstrap/heart.svg"),
            &Default::default(),
        )
        .expect("bundled heart SVG must render");
        let center = 8 * heart.size[0] + 8;
        assert!(heart.pixels[center].a() > 200, "heart center is hollow");
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

    #[test]
    fn signup_prompt_is_centered_under_the_card() {
        let context = egui::Context::default();
        theme::install(&context);
        let mut card_rect = egui::Rect::NOTHING;
        let mut prompt_rect = egui::Rect::NOTHING;
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
                        prompt_rect = signup_prompt(ui, &mut Vec::new());
                    })
                    .rect;
                },
            )
            .drop_without_applying_deltas();
        assert!(
            (prompt_rect.center().x - card_rect.center().x).abs() <= 2.0,
            "signup {prompt_rect:?} is not centered under card {card_rect:?}"
        );
    }

    #[test]
    fn footer_credit_is_centered_in_the_card() {
        let context = egui::Context::default();
        theme::install(&context);
        let mut card_rect = egui::Rect::NOTHING;
        let mut credit_rect = egui::Rect::NOTHING;
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
                        credit_rect = footer(ui);
                    })
                    .rect;
                },
            )
            .drop_without_applying_deltas();
        assert!(
            (credit_rect.center().x - card_rect.center().x).abs() <= 2.0,
            "footer credit {credit_rect:?} is not centered in card {card_rect:?}"
        );
    }

    #[test]
    fn theme_button_stays_at_the_right_of_the_header() {
        for width in [820.0, 1200.0] {
            let context = egui::Context::default();
            theme::install(&context);
            let mut button = egui::Rect::NOTHING;
            let mut available = egui::Rect::NOTHING;
            let mut spacing = 0.0;
            context
                .run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 480.0),
                        )),
                        ..Default::default()
                    },
                    |ui| {
                        available = ui.available_rect_before_wrap();
                        spacing = ui.spacing().item_spacing.x;
                        button = header(ui, Appearance::Dark, &mut Vec::new()).rect;
                    },
                )
                .drop_without_applying_deltas();
            assert!(
                button.center().x > available.center().x,
                "theme button stayed beside the logo: {button:?} in {available:?}"
            );
            assert!(button.right() <= available.right());
            assert!(available.right() - button.right() <= spacing);
        }
    }

    #[test]
    fn login_controls_switch_theme_and_reveal_password_only_when_enabled() {
        fn render(
            context: &egui::Context,
            input: egui::RawInput,
            messages: &mut Vec<Message>,
            show_password: &mut bool,
            busy: bool,
        ) -> (egui::Rect, egui::Rect) {
            let mut rects = (egui::Rect::NOTHING, egui::Rect::NOTHING);
            context
                .run_ui(input, |ui| {
                    rects.0 = theme_toggle(ui, Appearance::Dark, messages).rect;
                    rects.1 = password_toggle(ui, show_password, busy).rect;
                })
                .drop_without_applying_deltas();
            rects
        }

        let context = egui::Context::default();
        theme::install(&context);
        let mut show_password = false;
        let mut messages = Vec::new();
        let rects = render(
            &context,
            egui::RawInput::default(),
            &mut messages,
            &mut show_password,
            false,
        );
        let click = |position: egui::Pos2, pressed| egui::Event::PointerButton {
            pos: position,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        let theme_position = rects.0.center();
        render(
            &context,
            egui::RawInput {
                events: vec![
                    egui::Event::PointerMoved(theme_position),
                    click(theme_position, true),
                ],
                ..Default::default()
            },
            &mut messages,
            &mut show_password,
            false,
        );
        render(
            &context,
            egui::RawInput {
                events: vec![
                    egui::Event::PointerMoved(theme_position),
                    click(theme_position, false),
                ],
                ..Default::default()
            },
            &mut messages,
            &mut show_password,
            false,
        );
        assert!(
            messages
                .iter()
                .any(|message| matches!(message, Message::SetAppearance(Appearance::Light)))
        );

        let password_position = rects.1.center();
        render(
            &context,
            egui::RawInput {
                events: vec![
                    egui::Event::PointerMoved(password_position),
                    click(password_position, true),
                ],
                ..Default::default()
            },
            &mut messages,
            &mut show_password,
            false,
        );
        render(
            &context,
            egui::RawInput {
                events: vec![
                    egui::Event::PointerMoved(password_position),
                    click(password_position, false),
                ],
                ..Default::default()
            },
            &mut messages,
            &mut show_password,
            false,
        );
        assert!(show_password);

        show_password = false;
        render(
            &context,
            egui::RawInput {
                events: vec![
                    egui::Event::PointerMoved(password_position),
                    click(password_position, true),
                ],
                ..Default::default()
            },
            &mut messages,
            &mut show_password,
            true,
        );
        render(
            &context,
            egui::RawInput {
                events: vec![
                    egui::Event::PointerMoved(password_position),
                    click(password_position, false),
                ],
                ..Default::default()
            },
            &mut messages,
            &mut show_password,
            true,
        );
        assert!(!show_password);
    }

    #[test]
    fn a_cleared_password_cannot_be_brought_back_with_undo() {
        let context = egui::Context::default();
        theme::install(&context);
        let mut password = String::new();
        let mut show_password = false;
        let mut time = 0.0;
        let mut frame = |password: &mut String, events: Vec<egui::Event>| {
            time += 1.5;
            let mut field = egui::Id::NULL;
            context
                .run_ui(
                    egui::RawInput {
                        events,
                        time: Some(time),
                        ..Default::default()
                    },
                    |ui| {
                        field = edit_secret_field(
                            ui,
                            "Password",
                            "Password",
                            password,
                            &mut show_password,
                            false,
                            &mut Vec::new(),
                        )
                        .id;
                    },
                )
                .drop_without_applying_deltas();
            field
        };
        let undo = || {
            vec![egui::Event::Key {
                key: egui::Key::Z,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::COMMAND,
            }]
        };

        let field = frame(&mut password, Vec::new());
        context.memory_mut(|memory| memory.request_focus(field));
        frame(&mut password, Vec::new());
        frame(&mut password, vec![egui::Event::Text("hunter2".into())]);
        assert_eq!(password, "hunter2");
        // Pauses are when an editor records an undo step.
        frame(&mut password, Vec::new());
        frame(&mut password, Vec::new());

        // Signing out clears the form; the field must not remember it.
        password.clear();
        frame(&mut password, Vec::new());
        frame(&mut password, undo());
        assert!(password.is_empty(), "undo brought back a typed password");
    }

    #[test]
    fn each_sign_in_step_puts_the_cursor_in_its_first_empty_field() {
        let mut app = App::signed_out();
        let context = egui::Context::default();
        theme::install(&context);
        let frame = |app: &mut App, events: Vec<egui::Event>| {
            context
                .run_ui(
                    egui::RawInput {
                        events,
                        ..Default::default()
                    },
                    |root| {
                        show(root, app, &mut false, &mut Vec::new());
                    },
                )
                .drop_without_applying_deltas();
        };

        // The cursor arrives on the first pass and takes typing on the next.
        frame(&mut app, Vec::new());
        frame(&mut app, vec![egui::Event::Text("ada".into())]);
        assert_eq!(app.username_mut().as_str(), "ada");

        // Shown again, as after a failed attempt, the step starts in the
        // field still empty.
        forget_focus(&context);
        frame(&mut app, Vec::new());
        frame(&mut app, vec![egui::Event::Text("x".into())]);
        assert_eq!(app.username_mut().as_str(), "ada");
        assert_eq!(app.password_mut().as_str(), "x");
    }

    #[test]
    fn typing_a_password_never_moves_it_to_a_new_buffer() {
        let mut app = App::signed_out();
        let context = egui::Context::default();
        theme::install(&context);
        let mut time = 0.0;
        let mut frame = |app: &mut App, events: Vec<egui::Event>| {
            time += 1.0;
            let mut field = egui::Id::NULL;
            context
                .run_ui(
                    egui::RawInput {
                        events,
                        time: Some(time),
                        ..Default::default()
                    },
                    |ui| {
                        field = edit_secret_field(
                            ui,
                            "Password",
                            "Password",
                            app.password_mut(),
                            &mut false,
                            false,
                            &mut Vec::new(),
                        )
                        .id;
                    },
                )
                .drop_without_applying_deltas();
            field
        };
        let field = frame(&mut app, Vec::new());
        context.memory_mut(|memory| memory.request_focus(field));
        frame(&mut app, Vec::new());

        // A buffer that grows moves, and the old one is freed unerased.
        let buffer = app.password_mut().as_ptr();
        let typed = "correct horse battery staple, typed one key at a time";
        for key in typed.chars() {
            frame(&mut app, vec![egui::Event::Text(key.to_string())]);
        }
        assert_eq!(app.password_mut().as_str(), typed);
        assert_eq!(app.password_mut().as_ptr(), buffer);
    }

    #[test]
    fn the_measuring_pass_never_lays_out_what_was_typed() {
        let context = egui::Context::default();
        theme::install(&context);
        let mut password = String::from("hunter2");
        let mut username = String::from("ada@proton.me");
        let output = context.run_ui(egui::RawInput::default(), |ui| {
            // Visible here, unlike the real pass, so its text can be read.
            let mut measure = ui.new_child(egui::UiBuilder::new().sizing_pass());
            edit_field(
                &mut measure,
                "Username",
                "name@proton.me",
                &mut username,
                false,
                &mut Vec::new(),
            );
            edit_secret_field(
                &mut measure,
                "Password",
                "Password",
                &mut password,
                &mut true,
                false,
                &mut Vec::new(),
            );
        });
        let texts: Vec<String> = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::epaint::Shape::Text(text) => Some(text.galley.text().to_owned()),
                _ => None,
            })
            .collect();
        output.drop_without_applying_deltas();

        assert!(texts.iter().any(|text| text == "Password"), "{texts:?}");
        assert!(
            !texts.iter().any(|text| text.contains("hunter2")),
            "{texts:?}"
        );
        assert!(!texts.iter().any(|text| text.contains("ada@")), "{texts:?}");
        // The fields themselves are untouched.
        assert_eq!(password, "hunter2");
        assert_eq!(username, "ada@proton.me");
    }

    #[test]
    fn password_visibility_survives_measurement() {
        let mut app = App::signed_out();
        let context = egui::Context::default();
        theme::install(&context);
        let mut show_password = true;
        context
            .run_ui(egui::RawInput::default(), |ui| {
                show(ui, &mut app, &mut show_password, &mut Vec::new());
            })
            .drop_without_applying_deltas();
        assert!(show_password);
    }
}
