use super::*;
use crate::ui::test_support::contrast;
use crate::ui::theme::{DARK, LIGHT, install, set_text_scale};

#[test]
fn a_check_box_is_square_with_an_outline_or_the_accent() {
    let context = egui::Context::default();
    install(&context);
    for checked in [false, true] {
        let mut value = checked;
        let output = context.run_ui(egui::RawInput::default(), |ui| {
            ui.add(Checkbox::new(&mut value, "Ask before opening a link"));
        });
        let boxes: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Rect(rect)
                    if rect.corner_radius == CornerRadius::same(CHECKBOX_RADIUS) =>
                {
                    Some(rect.clone())
                }
                _ => None,
            })
            .collect();
        output.drop_without_applying_deltas();

        let [check_box] = boxes.as_slice() else {
            panic!("expected one box, checked: {checked}");
        };
        assert_eq!(check_box.rect.width(), check_box.rect.height());
        if checked {
            assert_eq!(check_box.fill, ACCENT);
        } else {
            assert_eq!(check_box.stroke, Stroke::new(1.0, DARK.control_border));
        }
    }
}

#[test]
fn clicking_a_check_box_toggles_it() {
    let context = egui::Context::default();
    install(&context);
    let mut value = false;
    let mut rect = egui::Rect::NOTHING;
    context
        .run_ui(egui::RawInput::default(), |ui| {
            rect = ui.add(Checkbox::new(&mut value, "Ask")).rect;
        })
        .drop_without_applying_deltas();
    let click = egui::RawInput {
        events: crate::ui::test_support::click_events(rect.center(), egui::PointerButton::Primary),
        ..Default::default()
    };
    let mut changed = false;
    context
        .run_ui(click, |ui| {
            changed = ui.add(Checkbox::new(&mut value, "Ask")).changed();
        })
        .drop_without_applying_deltas();

    assert!(value);
    assert!(changed);
}

#[test]
fn buttons_come_in_three_weights() {
    let context = egui::Context::default();
    install(&context);
    for emphasis in [Emphasis::Primary, Emphasis::Secondary, Emphasis::Danger] {
        let mut neutral = Color32::PLACEHOLDER;
        let output = context.run_ui(egui::RawInput::default(), |ui| {
            neutral = ui.visuals().widgets.inactive.weak_bg_fill;
            ui.add(emphasized(egui::Button::new("Send"), emphasis));
        });
        let (frame, text) = output
            .shapes
            .iter()
            .fold((None, None), |(frame, text), clipped| {
                match &clipped.shape {
                    egui::Shape::Rect(rect) => (Some((rect.fill, rect.stroke)), text),
                    egui::Shape::Text(shape) => (frame, Some(shape.fallback_color)),
                    _ => (frame, text),
                }
            });
        output.drop_without_applying_deltas();

        let (fill, stroke) = frame.expect("a button frame");
        let danger = DARK.danger;
        let expected = match emphasis {
            Emphasis::Primary => (ACCENT, Stroke::NONE, ON_ACCENT),
            Emphasis::Secondary => (neutral, Stroke::NONE, DARK.text),
            Emphasis::Danger => (Color32::TRANSPARENT, Stroke::new(1.0, danger), danger),
        };
        assert_eq!(
            (fill, stroke, text),
            (expected.0, expected.1, Some(expected.2))
        );
    }
}

#[test]
fn text_fields_are_outlined_at_rest_with_a_readable_hint() {
    for (mode, colors) in [(egui::Theme::Dark, DARK), (egui::Theme::Light, LIGHT)] {
        let context = egui::Context::default();
        install(&context);
        context.set_theme(mode);
        let mut text = String::new();
        let output = context.run_ui(egui::RawInput::default(), |ui| {
            ui.add(text_field(ui, &mut text).hint_text("Search mail…"));
        });
        let fill = context.style_of(mode).visuals.text_edit_bg_color();
        let outlined = output.shapes.iter().any(|clipped| {
            matches!(&clipped.shape, egui::epaint::Shape::Rect(rect)
                if rect.fill == fill && rect.stroke.color == colors.control_border)
        });
        let hint = output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::epaint::Shape::Text(text) if text.galley.text() == "Search mail…" => {
                    Some(text.galley.job.sections[0].format.color)
                }
                _ => None,
            })
            .expect("the hint should be drawn");
        output.drop_without_applying_deltas();

        assert!(outlined, "{mode:?}: a field at rest needs its outline");
        assert_eq!(hint, colors.placeholder, "{mode:?}");
        let ratio = contrast(hint, fill);
        assert!(ratio >= 4.5, "{mode:?}: hint at {ratio:.2}:1");
        assert!(
            contrast(colors.control_border, colors.panel) >= 3.0,
            "{mode:?}"
        );
    }
}

#[test]
fn compact_buttons_keep_a_comfortable_height() {
    // At every text size, with room around their words as at the usual one.
    for scale in crate::settings::TEXT_SCALES {
        let context = egui::Context::default();
        install(&context);
        set_text_scale(&context, scale);
        context
            .run_ui(egui::RawInput::default(), |ui| {
                let button = ui.add(compact_button(ui, "More options"));
                // egui keeps lengths on a grid of 1/32 pt.
                assert!(
                    button.rect.height() >= COMPACT_BUTTON_HEIGHT * scale - 1.0 / 32.0,
                    "{scale}: {}",
                    button.rect.height()
                );
                assert_eq!(compact_button_height(ui), COMPACT_BUTTON_HEIGHT * scale);
            })
            .drop_without_applying_deltas();
    }
}

#[test]
fn icon_buttons_are_as_tall_as_buttons_of_words_at_every_text_size() {
    for scale in crate::settings::TEXT_SCALES {
        let context = egui::Context::default();
        install(&context);
        set_text_scale(&context, scale);
        context
            .run_ui(egui::RawInput::default(), |ui| {
                ui.horizontal(|ui| {
                    let icon = ui.add(IconButton::new(Icon::Archive, "Archive"));
                    let words = ui.button("Move to…");
                    if scale <= 1.0 {
                        assert_eq!(icon.rect.height(), ICON_BUTTON_MIN_SIZE.y, "{scale}");
                    } else {
                        assert_eq!(icon.rect.height(), words.rect.height(), "{scale}");
                    }
                    assert!(icon.rect.height() >= words.rect.height(), "{scale}");
                });
            })
            .drop_without_applying_deltas();
    }
}

#[test]
fn a_compact_button_keeps_its_size_on_hover() {
    let context = egui::Context::default();
    install(&context);
    let render = |input| {
        let mut rect = egui::Rect::NOTHING;
        context
            .run_ui(input, |ui| {
                rect = ui
                    .add(
                        compact_button(ui, "Reply")
                            .fill(DARK.accent_soft)
                            .stroke(Stroke::new(1.0, ACCENT)),
                    )
                    .rect;
            })
            .drop_without_applying_deltas();
        rect
    };

    let idle = render(egui::RawInput::default());
    let hovered = render(egui::RawInput {
        events: vec![egui::Event::PointerMoved(idle.center())],
        ..Default::default()
    });

    assert_eq!(hovered.size(), idle.size());
}

#[test]
fn vector_icon_buttons_are_consistent_click_targets() {
    egui::__run_test_ui(|ui| {
        for icon in [
            Icon::Inbox,
            Icon::Drafts,
            Icon::Sent,
            Icon::Star,
            Icon::Archive,
            Icon::Spam,
            Icon::Trash,
            Icon::Folder,
            Icon::Label,
            Icon::Mail,
        ] {
            let response = ui.add(IconButton::new(icon, "Action"));
            assert!(response.rect.width() >= 32.0);
            assert!(response.rect.height() >= 28.0);
        }
    });
}
