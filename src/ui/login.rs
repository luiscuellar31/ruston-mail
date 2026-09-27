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
    egui::CentralPanel::default()
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

    match step {
        SignInStep::Credentials => {
            let changed = edit_field(
                ui,
                "Username or email",
                "name@proton.me",
                app.username_mut(),
                busy,
                messages,
            );
            if changed {
                app.login_edited();
            }
            let changed = edit_secret_field(
                ui,
                "Password",
                "Password",
                app.password_mut(),
                show_password,
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
                busy,
                messages,
            );
            if changed {
                app.login_edited();
            }
        }
        SignInStep::MailboxPassword => {
            ui.label("This account uses a separate mailbox password.");
            let changed = edit_secret_field(
                ui,
                "Mailbox password",
                "Mailbox password",
                app.mailbox_password_mut(),
                show_password,
                busy,
                messages,
            );
            if changed {
                app.login_edited();
            }
        }
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
            ui.visuals_mut().widgets.inactive.weak_bg_fill = theme::ACCENT;
            ui.visuals_mut().widgets.hovered.weak_bg_fill = theme::ACCENT_HOVER;
            ui.visuals_mut().widgets.active.weak_bg_fill = theme::ACCENT_HOVER;
            let text_color = if busy {
                ui.visuals().widgets.noninteractive.fg_stroke.color
            } else {
                egui::Color32::WHITE
            };
            ui.add_enabled(
                !busy,
                egui::Button::new((
                    egui::Atom::grow(),
                    egui::RichText::new(submit).color(text_color),
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

fn edit_field(
    ui: &mut egui::Ui,
    label: &str,
    hint: &str,
    value: &mut String,
    busy: bool,
    messages: &mut Vec<Message>,
) -> bool {
    ui.label(egui::RichText::new(label).strong());
    let response = ui.add_enabled(
        !busy,
        theme::text_field(value)
            .hint_text(hint)
            .desired_width(f32::INFINITY),
    );
    if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) && !busy {
        messages.push(Message::Submit);
    }
    ui.add_space(14.0);
    response.changed()
}

fn edit_secret_field(
    ui: &mut egui::Ui,
    label: &str,
    hint: &str,
    value: &mut String,
    show_password: &mut bool,
    busy: bool,
    messages: &mut Vec<Message>,
) -> bool {
    ui.label(egui::RichText::new(label).strong());
    let width =
        (ui.available_width() - theme::ICON_BUTTON_MIN_SIZE.x - ui.spacing().item_spacing.x)
            .max(1.0);
    let mut changed = false;
    ui.horizontal(|ui| {
        let response = ui.add_enabled(
            !busy,
            theme::text_field(value)
                .hint_text(hint)
                .password(!*show_password)
                .desired_width(width),
        );
        if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) && !busy {
            messages.push(Message::Submit);
        }
        changed = response.changed();
        password_toggle(ui, show_password, busy);
    });
    ui.add_space(14.0);
    changed
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
            egui::Image::new(egui::include_image!(
                "../../assets/macos/ruston-mail-1024.png"
            ))
            .fit_to_exact_size(egui::Vec2::splat(LOGO_SIZE))
            .alt_text("Ruston Mail logo"),
        );
        ui.label(egui::RichText::new("Ruston Mail").size(23.0).strong());
    });
}

fn footer(ui: &mut egui::Ui) {
    let color = theme::colors(ui).muted;
    ui.vertical_centered(|ui| {
        ui.label(
            egui::RichText::new("Not affiliated with Proton")
                .small()
                .color(color),
        );
    });
    let font = egui::TextStyle::Small.resolve(ui.style());
    let text_width = |text: &str| {
        ui.painter()
            .layout_no_wrap(text.to_owned(), font.clone(), color)
            .size()
            .x
    };
    let gap = ui.spacing().item_spacing.x;
    let width = text_width("Made with") + 16.0 + text_width("by Luis Cuellar") + gap * 2.0;
    let inset = ((ui.available_width() - width) * 0.5).max(0.0);
    ui.horizontal(|ui| {
        ui.add_space(inset);
        ui.label(egui::RichText::new("Made with").small().color(color));
        ui.add(
            egui::Image::new(egui::include_image!(
                "../../assets/icons/bootstrap/heart.svg"
            ))
            .fit_to_exact_size(egui::Vec2::splat(16.0))
            .tint(theme::ACCENT)
            .alt_text("heart"),
        );
        ui.label(egui::RichText::new("by Luis Cuellar").small().color(color));
    });
}

fn error(ui: &mut egui::Ui, app: &App) {
    if let Some(error) = app.error_message() {
        ui.label(egui::RichText::new(error).color(theme::colors(ui).danger));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lays the sign-in screen out in a window of `size` and answers with the
    /// card-and-footer group's rectangle and the window's.
    fn laid_out(size: egui::Vec2, appearance: Appearance) -> (egui::Rect, egui::Rect) {
        let mut app = App::signed_out();
        let mut show_password = false;
        let context = egui::Context::default();
        theme::install(&context);
        theme::apply(&context, appearance);
        let window = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
        let mut card = egui::Rect::NOTHING;
        context
            .run_ui(
                egui::RawInput {
                    screen_rect: Some(window),
                    ..Default::default()
                },
                |ui| card = show(ui, &mut app, &mut show_password, &mut Vec::new()).rect,
            )
            .drop_without_applying_deltas();

        (card, window)
    }

    #[test]
    fn the_login_group_is_centered_vertically_and_scrolls_when_space_is_short() {
        let (group, window) = laid_out(egui::vec2(1200.0, 800.0), Appearance::Dark);
        let (taller, tall_window) = laid_out(egui::vec2(1200.0, 1400.0), Appearance::Dark);
        let (small, minimum) = laid_out(egui::vec2(820.0, 480.0), Appearance::Dark);
        let (zoomed, narrow) = laid_out(egui::vec2(410.0, 240.0), Appearance::Dark);
        let (light, _) = laid_out(egui::vec2(820.0, 480.0), Appearance::Light);

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
            (zoomed, narrow),
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
        for (group, window) in [(small, minimum), (zoomed, narrow), (light, minimum)] {
            assert!(group.top() >= window.top());
            assert!(
                group.bottom() > window.bottom(),
                "small window should scroll"
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
    fn theme_button_stays_at_the_right_of_the_header() {
        let context = egui::Context::default();
        theme::install(&context);
        let mut rect = egui::Rect::NOTHING;
        context
            .run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(820.0, 480.0),
                    )),
                    ..Default::default()
                },
                |ui| rect = header(ui, Appearance::Dark, &mut Vec::new()).rect,
            )
            .drop_without_applying_deltas();
        assert!(
            rect.left() > 700.0,
            "theme button stayed beside the logo: {rect:?}"
        );
        assert!(rect.right() <= 820.0);
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
