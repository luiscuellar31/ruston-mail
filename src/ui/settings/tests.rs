use super::*;
use crate::ui::test_support::{shape_text_rect, text_rect, visible_text_rect};

#[test]
fn every_shortcut_key_stays_inside_its_section() {
    let context = egui::Context::default();
    theme::install(&context);
    let output = context.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(720.0, 4000.0),
            )),
            ..Default::default()
        },
        |ui| {
            page(
                ui,
                SettingsSection::Keyboard,
                &mut Settings::default(),
                &mut Vec::new(),
                super::super::notifications::Status::Off,
                None,
                None,
            )
        },
    );
    let lefts: Vec<(String, f32)> = SHORTCUTS
        .iter()
        .map(|(label, _)| {
            let rect = output
                .shapes
                .iter()
                .find_map(|shape| shape_text_rect(&shape.shape, label))
                .unwrap_or(egui::Rect::NOTHING);
            (label.to_string(), rect.left())
        })
        .collect();
    let keyboard = output
        .shapes
        .iter()
        .find_map(|shape| shape_text_rect(&shape.shape, "Keyboard"))
        .map(|rect| rect.left());
    output.drop_without_applying_deltas();

    let keyboard = keyboard.expect("the Keyboard section is drawn");
    for (label, left) in lefts {
        assert!(
            left >= keyboard - 0.5,
            "{label} starts at {left}, left of its section at {keyboard}"
        );
    }
}

/// What the page of `section` draws, 720 points wide.
fn drawn(section: SettingsSection) -> Vec<egui::epaint::ClippedShape> {
    let context = egui::Context::default();
    theme::install(&context);
    let output = context.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(720.0, 4000.0),
            )),
            ..Default::default()
        },
        |ui| {
            page(
                ui,
                section,
                &mut Settings::default(),
                &mut Vec::new(),
                super::super::notifications::Status::Off,
                Some(Path::new("THIRD_PARTY_NOTICES.txt")),
                None,
            )
        },
    );
    let shapes = output.shapes.clone();
    output.drop_without_applying_deltas();
    shapes
}

#[test]
fn each_section_shows_alone_under_its_own_title() {
    for section in SettingsSection::ALL {
        let shapes = drawn(section);
        for other in SettingsSection::ALL {
            assert_eq!(
                visible_text_rect(&shapes, title(other)).is_some(),
                other == section,
                "{other:?} on the page of {section:?}"
            );
        }
    }
}

#[test]
fn about_is_the_last_settings_section() {
    assert_eq!(SettingsSection::ALL.last(), Some(&SettingsSection::About));
    let shapes = drawn(SettingsSection::About);
    let about = text_rect(&shapes, "About");
    for label in [
        concat!("Ruston Mail ", env!("CARGO_PKG_VERSION")),
        "Source code",
        "Third-party notices",
    ] {
        assert!(text_rect(&shapes, label).top() > about.top(), "{label}");
    }
}

#[test]
fn advanced_choices_have_a_section_after_the_everyday_ones() {
    use SettingsSection::{About, Advanced, Keyboard};
    const CHOICES: [&str; 3] = [
        "Check recipient encryption while writing",
        "Accept international domain names",
        "Search this device when Proton cannot be reached",
    ];
    assert_eq!(SettingsSection::ALL[4..], [Advanced, Keyboard, About]);
    for section in SettingsSection::ALL {
        let shapes = drawn(section);
        for choice in CHOICES {
            assert_eq!(
                visible_text_rect(&shapes, choice).is_some(),
                section == Advanced,
                "{choice} on the page of {section:?}"
            );
        }
    }
}

#[test]
fn how_long_notices_stay_is_chosen_under_general() {
    let shapes = drawn(SettingsSection::General);
    let general = text_rect(&shapes, "General");
    for label in ["Notice duration", "5 seconds"] {
        assert!(
            text_rect(&shapes, label).top() > general.bottom(),
            "{label}"
        );
    }
    assert!(visible_text_rect(&drawn(SettingsSection::Advanced), "Notice duration").is_none());
    assert_eq!(
        (MIN_NOTICE_SECONDS..=MAX_NOTICE_SECONDS)
            .map(seconds_label)
            .collect::<Vec<_>>(),
        [
            "3 seconds",
            "4 seconds",
            "5 seconds",
            "6 seconds",
            "7 seconds",
            "8 seconds",
            "9 seconds",
            "10 seconds"
        ]
    );
}

#[test]
fn about_fits_both_themes_and_its_buttons_ask_for_what_they_name() {
    for appearance in [Appearance::Dark, Appearance::Light] {
        for width in [240.0, 720.0] {
            let context = egui::Context::default();
            theme::install(&context);
            theme::apply(&context, appearance);
            let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 480.0));
            let mut messages = Vec::new();
            let mut card = egui::Rect::NOTHING;
            let mut draw = |events| {
                context.run_ui(
                    egui::RawInput {
                        screen_rect: Some(viewport),
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        card = about(
                            ui,
                            &mut Settings::default(),
                            &mut messages,
                            Some(Path::new("THIRD_PARTY_NOTICES.txt")),
                            None,
                        )
                        .rect;
                    },
                )
            };
            draw(Vec::new()).drop_without_applying_deltas();
            let output = draw(Vec::new());
            let buttons = ["Source code", "Third-party notices"]
                .map(|label| text_rect(&output.shapes, label));
            for clipped in &output.shapes {
                if let egui::epaint::Shape::Text(text) = &clipped.shape {
                    assert!(
                        viewport
                            .contains_rect(egui::Rect::from_min_size(text.pos, text.galley.size()))
                    );
                }
            }
            output.drop_without_applying_deltas();
            for button in buttons {
                assert!(viewport.contains_rect(button));
                for pressed in [true, false] {
                    draw(vec![
                        egui::Event::PointerMoved(button.center()),
                        egui::Event::PointerButton {
                            pos: button.center(),
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: Default::default(),
                        },
                    ])
                    .drop_without_applying_deltas();
                }
            }
            assert!(viewport.contains_rect(card));
            assert!(matches!(
                messages.as_slice(),
                [Message::OpenSourceCode, Message::OpenThirdPartyNotices]
            ));
        }
    }
}

#[test]
fn about_offers_no_notices_the_app_did_not_come_with() {
    let context = egui::Context::default();
    theme::install(&context);
    let draw = || {
        context.run_ui(egui::RawInput::default(), |ui| {
            about(ui, &mut Settings::default(), &mut Vec::new(), None, None);
        })
    };
    draw().drop_without_applying_deltas();
    let output = draw();
    let drawn = |label| {
        output
            .shapes
            .iter()
            .any(|shape| shape_text_rect(&shape.shape, label).is_some())
    };
    let (source, notices) = (drawn("Source code"), drawn("Third-party notices"));
    output.drop_without_applying_deltas();

    assert!(source);
    assert!(!notices);
}

#[test]
fn about_offers_to_check_for_updates_and_says_what_the_last_check_found() {
    let newer = crate::releases::Version::parse("999.0.0").unwrap();
    let lines = [
        "Checking for updates…",
        "You have the latest version.",
        "Could not check for updates.",
        "Ruston Mail 999.0.0 is available.",
    ];
    for (status, said) in [
        (None, None),
        (Some(UpdateStatus::Checking), Some(lines[0])),
        (Some(UpdateStatus::Latest), Some(lines[1])),
        (Some(UpdateStatus::Failed), Some(lines[2])),
        (Some(UpdateStatus::Available(newer)), Some(lines[3])),
    ] {
        let context = egui::Context::default();
        theme::install(&context);
        let mut settings = Settings::default();
        let mut messages = Vec::new();
        let mut draw = |events| {
            context.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ui| {
                    about(ui, &mut settings, &mut messages, None, status);
                },
            )
        };
        draw(Vec::new()).drop_without_applying_deltas();
        let output = draw(Vec::new());
        let drawn = |label| {
            output
                .shapes
                .iter()
                .any(|shape| shape_text_rect(&shape.shape, label).is_some())
        };
        for line in lines {
            assert_eq!(drawn(line), said == Some(line), "{line} for {status:?}");
        }
        let checkbox = text_rect(&output.shapes, "Check for updates");
        let download = drawn("Download").then(|| text_rect(&output.shapes, "Download"));
        output.drop_without_applying_deltas();
        assert_eq!(download.is_some(), said == Some(lines[3]));

        // The box turns checking on in the draft, for Apply to send;
        // Download asks for the release's page.
        for target in [Some(checkbox), download].into_iter().flatten() {
            for pressed in [true, false] {
                draw(vec![
                    egui::Event::PointerMoved(target.center()),
                    egui::Event::PointerButton {
                        pos: target.center(),
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ])
                .drop_without_applying_deltas();
            }
        }
        assert!(settings.check_for_updates);
        match download {
            Some(_) => assert!(matches!(messages.as_slice(), [Message::OpenReleasePage])),
            None => assert!(messages.is_empty()),
        }
    }
}

#[test]
fn unsaved_settings_actions_fit_at_large_zoom() {
    let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(410.0, 240.0));
    let context = egui::Context::default();
    context
        .run_ui(
            egui::RawInput {
                screen_rect: Some(viewport),
                ..Default::default()
            },
            |_| {
                confirm_exit(&context);
            },
        )
        .drop_without_applying_deltas();
    let output = context.run_ui(
        egui::RawInput {
            screen_rect: Some(viewport),
            ..Default::default()
        },
        |_| {
            confirm_exit(&context);
        },
    );

    let actions = ["Keep editing", "Discard changes", "Apply and continue"].map(|label| {
        (
            label,
            output
                .shapes
                .iter()
                .find_map(|shape| crate::ui::test_support::shape_text_rect(&shape.shape, label)),
        )
    });
    output.drop_without_applying_deltas();
    for (label, text) in actions {
        let text = text.unwrap_or_else(|| panic!("missing {label}"));
        assert!(
            viewport.contains_rect(text),
            "{label} falls outside {viewport:?}"
        );
    }
}

#[test]
fn a_card_fills_the_page_however_little_it_says() {
    // Left to size itself a card stops at its longest line, which made
    // the settings read as a column with an empty panel beside it.
    let context = egui::Context::default();
    let mut card = 0.0;
    let mut page = 0.0;

    context
        .run_ui(egui::RawInput::default(), |ui| {
            ui.set_width(900.0);
            page = ui.available_width();
            card = section(ui, "Reading", |ui| {
                ui.label("short");
            })
            .rect
            .width();
        })
        .drop_without_applying_deltas();

    assert!(
        (card - page).abs() <= 1.0,
        "a card {card} wide on a page {page} wide"
    );
}

#[test]
fn apply_reports_only_preferences_changed_in_the_window() {
    let current = Settings::default();
    let mut draft = current.clone();
    draft.window.width += 100.0;
    draft.panels.sidebar += 0.1;
    draft
        .accounts
        .set_folder(crate::mail::Folder::System(MailFolder::Spam));
    assert!(preference_changes(&current, &draft).is_empty());

    draft.confirm_links = false;
    draft.zoom = 1.15;
    draft.text_scale = 1.3;
    draft.show_unread_badge = false;
    draft.desktop_notifications = false;
    draft.appearance = Appearance::Light;
    draft.system_font = true;
    draft.check_recipient_protection = false;
    draft.accept_international_domains = true;
    draft.search_device_offline = true;
    draft.show_hidden_text = true;
    draft.sender_text_styles = false;
    draft.data_tables = false;
    draft.show_inline_images = false;
    draft.check_for_updates = true;
    draft.notice_seconds = 8;
    let changes = preference_changes(&current, &draft);
    assert_eq!(changes.len(), 16);
    assert!(
        changes
            .iter()
            .any(|message| matches!(message, Message::SetCheckForUpdates(true)))
    );
    assert!(
        changes
            .iter()
            .any(|message| matches!(message, Message::SetNoticeSeconds(8)))
    );
    assert!(
        changes
            .iter()
            .any(|message| matches!(message, Message::SetSearchDeviceOffline(true)))
    );
    assert!(
        changes
            .iter()
            .any(|message| matches!(message, Message::SetCheckRecipientProtection(false)))
    );
    assert!(
        changes
            .iter()
            .any(|message| matches!(message, Message::SetAcceptInternationalDomains(true)))
    );
    assert!(
        changes
            .iter()
            .any(|message| matches!(message, Message::SetConfirmLinks(false)))
    );
    assert!(
        changes
            .iter()
            .any(|message| matches!(message, Message::SetZoom(zoom) if *zoom == 1.15))
    );
    assert!(
        changes
            .iter()
            .any(|message| matches!(message, Message::SetTextScale(scale) if *scale == 1.3))
    );
    assert!(
        changes
            .iter()
            .any(|message| matches!(message, Message::SetShowUnreadBadge(false)))
    );
    assert!(
        changes
            .iter()
            .any(|message| matches!(message, Message::SetDesktopNotifications(false)))
    );
    assert!(
        changes
            .iter()
            .any(|message| matches!(message, Message::SetAppearance(Appearance::Light)))
    );
    assert!(
        changes
            .iter()
            .any(|message| matches!(message, Message::SetSystemFont(true)))
    );
}

#[test]
fn following_the_system_and_choosing_a_scheme_again_reach_apply() {
    let dark = Settings::default();
    let mut light = Settings::default();
    light.appearance = Appearance::Light;
    let mut following = Settings::default();
    following.follow_system_appearance = true;
    assert!(matches!(
        preference_changes(&dark, &following)[..],
        [Message::SetFollowSystemAppearance(true)]
    ));
    assert!(matches!(
        preference_changes(&following, &dark)[..],
        [Message::SetAppearance(Appearance::Dark)]
    ));
    assert!(matches!(
        preference_changes(&following, &light)[..],
        [Message::SetAppearance(Appearance::Light)]
    ));
    assert!(preference_changes(&following, &following).is_empty());
    // A scheme changed while following still keeps following.
    let mut following_light = light.clone();
    following_light.follow_system_appearance = true;
    assert!(matches!(
        preference_changes(&dark, &following_light)[..],
        [
            Message::SetAppearance(Appearance::Light),
            Message::SetFollowSystemAppearance(true)
        ]
    ));
}

#[test]
fn zoom_steps_move_between_the_offered_levels() {
    assert_eq!(zoom_step(1.0, ZoomStep::In), 1.15);
    assert_eq!(zoom_step(1.0, ZoomStep::Out), 0.9);
    assert_eq!(zoom_step(1.5, ZoomStep::Reset), 1.0);
    // Either end stays put.
    assert_eq!(zoom_step(0.9, ZoomStep::Out), 0.9);
    assert_eq!(zoom_step(1.75, ZoomStep::In), 1.75);
    // A level from an older file joins the nearest offered one.
    assert_eq!(zoom_step(1.05, ZoomStep::In), 1.15);
    assert_eq!(zoom_step(1.05, ZoomStep::Out), 1.0);
}

#[test]
fn the_appearance_and_general_choices_show_their_whole_text_at_every_text_size() {
    for scale in TEXT_SCALES {
        let context = egui::Context::default();
        theme::install(&context);
        theme::set_text_scale(&context, scale);
        // The longest of each choice's labels.
        let mut settings = Settings::default();
        settings.notice_seconds = MAX_NOTICE_SECONDS;
        settings.text_scale = scale;
        let output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(720.0, 4000.0),
                )),
                ..Default::default()
            },
            |ui| {
                for section in [SettingsSection::Appearance, SettingsSection::General] {
                    page(
                        ui,
                        section,
                        &mut settings,
                        &mut Vec::new(),
                        super::super::notifications::Status::Off,
                        None,
                        None,
                    );
                }
            },
        );
        let shapes = output.shapes.clone();
        output.drop_without_applying_deltas();

        for label in [
            "Interface scale",
            "Text size",
            &percent_label(scale),
            "Where I left off",
            "10 seconds",
        ] {
            assert!(
                visible_text_rect(&shapes, label).is_some(),
                "{scale}: {label} is cut short"
            );
        }
    }
}

#[test]
fn every_offered_zoom_is_one_the_settings_would_keep() {
    // The panel must not offer a scale that loading clamps away, and the
    // scale a fresh install starts at has to be one of the choices.
    for step in ZOOM_STEPS {
        assert!(
            (crate::settings::MIN_ZOOM..=crate::settings::MAX_ZOOM).contains(&step),
            "{step} is outside what the settings keep"
        );
    }
    assert!(
        ZOOM_STEPS.contains(&Settings::default().zoom),
        "the scale everyone starts at has no button"
    );
}
