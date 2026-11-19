use super::*;

#[test]
fn the_sign_in_screen_says_who_gets_a_kept_draft_back() {
    let mut app = App::signed_in_as("user-1", "alex@proton.me");
    let _ = app.update(Message::OpenCompose);
    let _ = app.update(Message::ComposeChanged(
        crate::app::ComposeField::Body,
        "Keep this draft".into(),
    ));
    let _ = app.update(Message::ConversationsLoaded(
        1,
        Err(crate::mail::MailboxError::SessionExpired),
    ));
    let context = egui::Context::default();
    theme::install(&context);

    let output = context.run_ui(egui::RawInput::default(), |ui| {
        sign_in(ui, &mut app, &mut false, &mut Vec::new());
    });
    let shown: Vec<String> = output
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::epaint::Shape::Text(text) => Some(text.galley.text().to_owned()),
            _ => None,
        })
        .collect();
    output.drop_without_applying_deltas();

    assert!(
        shown
            .iter()
            .any(|text| text.contains("sign in again as alex@proton.me")),
        "{shown:?}"
    );
}

#[test]
fn the_sign_in_screen_offers_to_try_a_saved_session_again() {
    let (mut app, _) = App::boot(false, crate::settings::Settings::default());
    let _ = app.update(Message::SessionChecked(crate::mail::ResumeOutcome::Failed(
        crate::mail::AuthError::Connection,
    )));
    let context = egui::Context::default();
    theme::install(&context);
    let frame = |app: &mut App, events: Vec<egui::Event>| {
        let mut messages = Vec::new();
        let output = context.run_ui(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ui| sign_in(ui, app, &mut false, &mut messages),
        );
        let button = output
            .shapes
            .iter()
            .find_map(|clipped| {
                crate::ui::test_support::shape_text_rect(&clipped.shape, "Try again")
            })
            .map(|rect| rect.center());
        output.drop_without_applying_deltas();
        (button, messages)
    };

    let (button, _) = frame(&mut app, Vec::new());
    let center = button.expect("the saved session can be tried again");
    let press = |pressed| egui::Event::PointerButton {
        pos: center,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::default(),
    };
    frame(&mut app, vec![egui::Event::PointerMoved(center)]);
    let (_, messages) = frame(&mut app, vec![press(true), press(false)]);
    assert!(
        messages
            .iter()
            .any(|message| matches!(message, Message::RetrySavedSession))
    );
}

#[test]
fn screen_readers_hear_each_sign_in_field_by_its_name() {
    let context = egui::Context::default();
    theme::install(&context);
    context.enable_accesskit();
    let mut names = Vec::new();
    for state in [
        AuthState::SignedOut,
        AuthState::NeedsTotp,
        AuthState::NeedsMailboxPassword,
    ] {
        let mut app = App::signed_out();
        app.set_auth_state(state);
        let output = context.run_ui(egui::RawInput::default(), |ui| {
            sign_in(ui, &mut app, &mut false, &mut Vec::new());
        });
        names.extend(theme::text_field_names(&output));
        output.drop_without_applying_deltas();
    }

    for name in [
        "Username or email",
        "Password",
        "Two-factor code",
        "Mailbox password",
    ] {
        assert!(
            names.iter().any(|heard| heard == name),
            "{name} in {names:?}"
        );
    }
}

#[test]
fn signing_in_is_drawn_at_full_strength_and_takes_no_clicks() {
    fn walk(
        shape: &egui::epaint::Shape,
        texts: &mut Vec<egui::epaint::TextShape>,
        rects: &mut Vec<egui::epaint::RectShape>,
    ) {
        match shape {
            egui::epaint::Shape::Text(text) => texts.push(text.clone()),
            egui::epaint::Shape::Rect(rect) => rects.push(rect.clone()),
            egui::epaint::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, texts, rects);
                }
            }
            _ => {}
        }
    }

    struct Frame {
        label: egui::epaint::TextShape,
        fills: Vec<egui::epaint::RectShape>,
        unavailable: Option<bool>,
        messages: Vec<Message>,
    }

    let mut app = App::signed_out();
    app.set_auth_state(AuthState::SigningIn(SignInStep::Credentials));
    let context = egui::Context::default();
    theme::install(&context);
    context.enable_accesskit();
    let frame = |app: &mut App, events: Vec<egui::Event>| {
        let mut messages = Vec::new();
        let output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 900.0),
                )),
                events,
                ..Default::default()
            },
            |ui| sign_in(ui, app, &mut false, &mut messages),
        );
        let (mut texts, mut fills) = (Vec::new(), Vec::new());
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut texts, &mut fills);
        }
        let unavailable = output
            .platform_output
            .accesskit_update
            .iter()
            .flat_map(|update| &update.nodes)
            .find(|(_, node)| node.label() == Some("Signing in…"))
            .map(|(_, node)| node.is_disabled());
        output.drop_without_applying_deltas();
        let label = texts
            .into_iter()
            .find(|text| text.galley.text() == "Signing in…")
            .expect("the busy button");
        Frame {
            label,
            fills,
            unavailable,
            messages,
        }
    };
    let full_strength = |frame: &Frame| {
        let label = &frame.label;
        let bounds = egui::Rect::from_min_size(label.pos, label.galley.size());
        let drawn: Vec<egui::Color32> = label
            .galley
            .rows
            .iter()
            .flat_map(|placed| placed.row.visuals.mesh.vertices.iter())
            .map(|vertex| {
                if vertex.color == egui::Color32::PLACEHOLDER {
                    label.fallback_color
                } else {
                    vertex.color
                }
            })
            .collect();
        assert!(
            !drawn.is_empty() && drawn.iter().all(|color| *color == theme::ON_ACCENT),
            "the label is faded: {:?}",
            drawn.first()
        );
        assert!(
            frame
                .fills
                .iter()
                .any(|rect| rect.rect.contains_rect(bounds) && rect.fill == theme::ACCENT),
            "no accent at rest behind the label"
        );
        bounds.center()
    };

    let shown = frame(&mut app, Vec::new());
    let center = full_strength(&shown);
    assert_eq!(
        shown.unavailable,
        Some(true),
        "screen readers hear it as usable"
    );

    let press = |pressed| egui::Event::PointerButton {
        pos: center,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::default(),
    };
    let pressed = frame(
        &mut app,
        vec![egui::Event::PointerMoved(center), press(true)],
    );
    // Under the pointer and pressed, it keeps its look: nothing to click.
    full_strength(&pressed);
    let released = frame(&mut app, vec![press(false)]);
    assert!(
        !released
            .messages
            .iter()
            .any(|message| matches!(message, Message::Submit)),
        "a sign-in under way was submitted again"
    );
}

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

/// Where the card-and-footer group of the login screen lands, and the
/// window it was laid out in, in egui points.
struct Layout {
    group: egui::Rect,
    window: egui::Rect,
}

/// Lays out a physical window at the requested zoom.
fn laid_out(physical_size: egui::Vec2, appearance: Appearance, zoom: f32) -> Layout {
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

    Layout {
        group: card,
        window,
    }
}

#[test]
fn the_login_group_is_centered_vertically_and_scrolls_when_space_is_short() {
    let regular = laid_out(egui::vec2(1200.0, 800.0), Appearance::Dark, 1.0);
    let taller = laid_out(egui::vec2(1200.0, 1400.0), Appearance::Dark, 1.0);
    let small = laid_out(egui::vec2(820.0, 480.0), Appearance::Dark, 1.0);
    let zoomed = laid_out(egui::vec2(820.0, 480.0), Appearance::Dark, 2.0);
    let light = laid_out(egui::vec2(820.0, 480.0), Appearance::Light, 1.0);

    assert!(
        regular.group.height() > 0.0,
        "the login group was never laid out"
    );
    assert!(
        (regular.group.height() - taller.group.height()).abs() <= 1.0,
        "the login group grew with the window: {} against {}",
        regular.group.height(),
        taller.group.height()
    );
    for (case, Layout { group, window }) in [("regular", &regular), ("taller", &taller)] {
        assert!(
            (group.center().y - window.center().y).abs() <= 12.0,
            "{case}: login group {group:?} is not centered vertically in {window:?}"
        );
    }
    for (case, Layout { group, window }) in [
        ("regular", &regular),
        ("small", &small),
        ("zoomed", &zoomed),
        ("light", &light),
    ] {
        assert!(
            group.left() >= window.left(),
            "{case}: login group overflows left: {group:?}"
        );
        assert!(
            group.right() <= window.right(),
            "{case}: login group overflows right: {group:?}"
        );
        assert!(
            (group.center().x - window.center().x).abs() <= 12.0,
            "{case}: login group is not horizontally centered: {group:?} in {window:?}"
        );
    }
    for (case, Layout { group, window }) in
        [("small", &small), ("zoomed", &zoomed), ("light", &light)]
    {
        assert!(
            group.top() >= window.top(),
            "{case}: login group overflows the top"
        );
        assert!(
            group.bottom() > window.bottom(),
            "{case}: the login group should scroll when available height is short"
        );
    }
}

#[test]
fn bundled_login_icons_render_and_can_be_tinted() {
    for svg in [
        include_bytes!("../../../assets/icons/bootstrap/sun.svg").as_slice(),
        include_bytes!("../../../assets/icons/bootstrap/moon.svg").as_slice(),
        include_bytes!("../../../assets/icons/bootstrap/eye.svg").as_slice(),
        include_bytes!("../../../assets/icons/bootstrap/eye-slash.svg").as_slice(),
        include_bytes!("../../../assets/icons/bootstrap/heart.svg").as_slice(),
    ] {
        let image = egui_extras::image::load_svg_bytes(svg, &Default::default())
            .expect("bundled SVG must render");
        assert!(image.pixels.iter().any(|pixel| pixel.a() > 0));
        assert!(
            image
                .pixels
                .iter()
                .any(|pixel| { pixel.r() > 0 && pixel.r() == pixel.g() && pixel.g() == pixel.b() })
        );
    }
}

#[test]
fn footer_heart_is_filled() {
    let heart = egui_extras::image::load_svg_bytes(
        include_bytes!("../../../assets/icons/bootstrap/heart.svg"),
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
fn a_busy_password_toggle_is_announced_as_unavailable() {
    let context = egui::Context::default();
    theme::install(&context);
    context.enable_accesskit();
    for busy in [false, true] {
        let output = context.run_ui(egui::RawInput::default(), |ui| {
            password_toggle(ui, &mut false, busy);
        });
        let unavailable = output
            .platform_output
            .accesskit_update
            .iter()
            .flat_map(|update| &update.nodes)
            .find(|(_, node)| node.label() == Some("Show password"))
            .map(|(_, node)| node.is_disabled());
        output.drop_without_applying_deltas();
        assert_eq!(unavailable, Some(busy), "busy: {busy}");
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
