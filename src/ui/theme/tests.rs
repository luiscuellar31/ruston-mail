use super::*;

#[test]
fn bundled_ui_icons_render_and_can_be_tinted() {
    let context = egui::Context::default();
    install(&context);

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
        Icon::Lock,
        Icon::Unlock,
    ] {
        let egui::ImageSource::Bytes { bytes, .. } = icon.source() else {
            panic!("{icon:?} must be bundled");
        };
        let image = egui_extras::image::load_svg_bytes(bytes.as_ref(), &Default::default())
            .expect("bundled icon must render");
        assert_eq!(image.size, [16, 16], "{icon:?}");
        assert!(
            image.pixels.iter().any(|pixel| {
                pixel.a() > 0 && pixel.r() == 255 && pixel.g() == 255 && pixel.b() == 255
            }),
            "{icon:?} must have white pixels for tinting"
        );
        assert!(
            egui::Image::new(icon.source())
                .load_for_size(&context, egui::Vec2::splat(ICON_SIZE))
                .expect("bundled icon must load through egui")
                .is_ready(),
            "{icon:?} must be ready to paint"
        );
    }
}

/// WCAG contrast ratio between two opaque colors.
fn contrast(a: Color32, b: Color32) -> f64 {
    let linear = |channel: u8| {
        let value = f64::from(channel) / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    let luminance = |color: Color32| {
        0.2126 * linear(color.r()) + 0.7152 * linear(color.g()) + 0.0722 * linear(color.b())
    };
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// The rectangles the focus ring marks in one frame of `content`.
fn focus_rings(
    context: &egui::Context,
    input: egui::RawInput,
    mut content: impl FnMut(&mut egui::Ui),
) -> Vec<egui::Rect> {
    let output = context.run_ui(input, |ui| {
        content(ui);
        paint_focus_ring(ui.ctx());
    });
    let rings = output
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Rect(rect) if rect.stroke.color == DARK.focus => Some(rect.rect),
            _ => None,
        })
        .collect();
    output.drop_without_applying_deltas();
    rings
}

#[test]
fn the_widget_with_keyboard_focus_gets_a_ring() {
    let context = egui::Context::default();
    install(&context);
    let mut text = String::new();
    for field in [false, true] {
        let mut target = egui::Rect::NOTHING;
        let rings = focus_rings(&context, egui::RawInput::default(), |ui| {
            let button = ui.button("Archive");
            let edit = ui.add(text_field(ui, &mut text));
            let focused = if field { edit } else { button };
            focused.request_focus();
            target = focused.rect;
        });
        assert_eq!(rings, [target], "field: {field}");
    }
    // The ring takes the place of the border egui gives a focused field.
    assert_eq!(
        context
            .style_of(egui::Theme::Dark)
            .visuals
            .selection
            .stroke
            .width,
        0.0
    );

    // Focus outlasts a widget for the rest of the pass it disappears in,
    // but the ring does not.
    assert!(focus_rings(&context, egui::RawInput::default(), |_| {}).is_empty());
    // Nor does a window in the background show one.
    let background = egui::RawInput {
        focused: false,
        ..Default::default()
    };
    let rings = focus_rings(&context, background, |ui| {
        ui.button("Archive").request_focus()
    });
    assert!(rings.is_empty());
}

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
        events: vec![
            egui::Event::PointerMoved(rect.center()),
            egui::Event::PointerButton {
                pos: rect.center(),
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            },
            egui::Event::PointerButton {
                pos: rect.center(),
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            },
        ],
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
fn small_text_is_drawn_at_footnote_size() {
    let context = egui::Context::default();
    install(&context);
    for mode in [egui::Theme::Dark, egui::Theme::Light] {
        let style = context.style_of(mode);
        assert_eq!(
            egui::TextStyle::Small.resolve(&style).size,
            FOOTNOTE_SIZE,
            "{mode:?}"
        );
    }
}

#[test]
fn text_on_accent_buttons_keeps_adequate_contrast() {
    for fill in [ACCENT, ACCENT_HOVER, ACCENT_PRESSED] {
        let ratio = contrast(ON_ACCENT, fill);
        assert!(ratio >= 4.5, "text on {fill:?} has {ratio:.2}:1 contrast");
    }
}

#[test]
fn normal_text_stands_above_secondary_text_in_both_themes() {
    let context = egui::Context::default();
    install(&context);
    for (mode, colors) in [(egui::Theme::Dark, DARK), (egui::Theme::Light, LIGHT)] {
        let visuals = &context.style_of(mode).visuals;
        let text = visuals.text_color();
        // A button at rest never reads dimmer than the text beside it.
        assert_eq!(visuals.widgets.inactive.text_color(), text, "{mode:?}");
        for surface in [
            colors.panel,
            colors.panel_raised,
            colors.sidebar,
            colors.accent_soft,
        ] {
            let normal = contrast(text, surface);
            let secondary = contrast(colors.muted, surface);
            assert!(
                normal > secondary && normal >= 7.0,
                "{mode:?} on {surface:?}: text {normal:.2}:1, secondary {secondary:.2}:1"
            );
        }
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
fn a_hovered_sidebar_place_stays_quieter_than_the_open_one() {
    for (mode, colors) in [(egui::Theme::Dark, DARK), (egui::Theme::Light, LIGHT)] {
        let hover = contrast(colors.sidebar_hover, colors.sidebar);
        let selection = contrast(colors.accent_soft, colors.sidebar);
        assert!(
            1.03 < hover && hover < selection,
            "{mode:?}: hover {hover:.2}:1, selection {selection:.2}:1"
        );
    }
}

/// Lays `content` out through [`centered_group`] in a 400x600 panel and
/// answers with the group's rectangle and the panel's.
fn centered(content: impl FnMut(&mut egui::Ui) + Copy) -> (egui::Rect, egui::Rect) {
    let context = egui::Context::default();
    let panel = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 600.0));
    let mut result = (egui::Rect::NOTHING, egui::Rect::NOTHING);
    context
        .run_ui(
            egui::RawInput {
                screen_rect: Some(panel),
                ..Default::default()
            },
            |ui| {
                let available = ui.available_rect_before_wrap();
                result = (centered_group(ui, content).rect, available);
            },
        )
        .drop_without_applying_deltas();
    result
}

fn assert_centered(group: egui::Rect, available: egui::Rect) {
    assert!(group.height() > 0.0, "the group was never laid out");
    assert!(group.height() < available.height(), "it filled the height");
    assert!(group.width() < available.width(), "it filled the width");
    for (axis, group_center, available_center) in [
        ("vertically", group.center().y, available.center().y),
        ("horizontally", group.center().x, available.center().x),
    ] {
        assert!(
            (group_center - available_center).abs() <= 1.0,
            "the group is not centered {axis}: {group_center} against {available_center}"
        );
    }
}

#[test]
fn every_mark_the_interface_draws_has_a_glyph() {
    let context = egui::Context::default();
    install(&context);
    // Fonts are built on the first pass, not before it.
    context
        .run_ui(egui::RawInput::default(), |_| {})
        .drop_without_applying_deltas();
    let font = egui::FontId::proportional(14.0);
    // The parser's list bullet must be available too.
    for mark in [STAR, DOT, "•"] {
        assert!(
            context.fonts_mut(|fonts| fonts.has_glyphs(&font, mark)),
            "{mark:?} has no glyph and would be drawn as the replacement box"
        );
    }
}

#[test]
fn a_centered_group_of_widgets_sits_in_the_middle_of_the_space_it_is_given() {
    let (group, available) = centered(|ui| {
        ui.heading("Ruston Mail");
        ui.label("Opening Ruston Mail…");
        ui.spinner();
    });

    assert_centered(group, available);
}

#[test]
fn a_centered_group_built_from_a_row_is_centered_too() {
    // A nested layout is not moved by the one around it, so this is the
    // case that stays against the left edge if only the height is placed.
    let (group, available) = centered(|ui| {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Loading conversation…");
        });
    });

    assert_centered(group, available);
}

#[test]
fn selection_is_limited_to_mail_content() {
    let context = egui::Context::default();
    install(&context);
    assert!(
        !context
            .style_of(egui::Theme::Dark)
            .interaction
            .selectable_labels
    );

    egui::__run_test_ui(|ui| {
        ui.style_mut().interaction.selectable_labels = false;
        assert!(!ui.style().interaction.selectable_labels);
        selectable_text(ui, |ui| {
            assert!(ui.style().interaction.selectable_labels);
        });
        assert!(!ui.style().interaction.selectable_labels);
    });
}

#[test]
fn compact_buttons_keep_a_comfortable_height() {
    egui::__run_test_ui(|ui| {
        let button = ui.add(compact_button("More options"));
        assert!(button.rect.height() >= COMPACT_BUTTON_HEIGHT);
    });
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
                        compact_button("Reply")
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

#[test]
fn scrollbars_reserve_space_and_share_a_short_animation() {
    let context = egui::Context::default();
    install(&context);
    let style = context.style_of(egui::Theme::Dark);

    assert!(!style.spacing.scroll.floating);
    assert!(style.spacing.scroll.allocated_width() > 0.0);
    assert_eq!(style.scroll_animation.points_per_second, 1_200.0);
    assert_eq!(
        style.scroll_animation.duration,
        egui::Rangef::new(0.08, 0.24)
    );
}

#[test]
fn appearance_switches_visuals_and_custom_colors() {
    let context = egui::Context::default();
    install(&context);
    assert_eq!(context.theme(), egui::Theme::Dark);
    assert_eq!(
        context.style_of(egui::Theme::Dark).visuals.panel_fill,
        DARK.panel
    );

    apply(&context, Appearance::Light);
    assert_eq!(context.theme(), egui::Theme::Light);
    assert_eq!(
        context.style_of(egui::Theme::Light).visuals.panel_fill,
        LIGHT.panel
    );
    context
        .run_ui(egui::RawInput::default(), |ui| {
            assert!(!ui.visuals().dark_mode);
            assert_eq!(colors(ui).muted, LIGHT.muted);
            assert_eq!(card(ui).fill, LIGHT.panel_raised);
        })
        .drop_without_applying_deltas();

    apply(&context, Appearance::Dark);
    assert_eq!(context.theme(), egui::Theme::Dark);
    assert_eq!(
        context.style_of(egui::Theme::Dark).visuals.panel_fill,
        DARK.panel
    );
}

#[test]
fn the_waiting_mark_pulses_in_place_without_jumping() {
    let context = egui::Context::default();
    install(&context);
    let drawn = |time: f64| {
        let raw = egui::RawInput {
            focused: true,
            time: Some(time),
            ..Default::default()
        };
        let output = context.run_ui(raw, |ui| {
            spinner(ui);
        });
        let shapes = output.shapes.clone();
        output.drop_without_applying_deltas();
        shapes
            .into_iter()
            .find_map(|clipped| match clipped.shape {
                egui::Shape::Circle(ring) => Some(ring),
                _ => None,
            })
            .expect("the waiting mark")
    };

    // One frame of its clock apart.
    let (now, next) = (drawn(1.0), drawn(1.0 + ANIMATION_FRAME.as_secs_f64()));
    assert_eq!((now.center, now.radius), (next.center, next.radius));
    let step = [
        now.stroke.color.r().abs_diff(next.stroke.color.r()),
        now.stroke.color.g().abs_diff(next.stroke.color.g()),
        now.stroke.color.b().abs_diff(next.stroke.color.b()),
    ];
    assert!(step.iter().all(|change| *change <= 16), "{step:?}");
    // Half a period later, it has pulsed.
    let later = drawn(1.0 + SKELETON_PERIOD_SECONDS / 2.0);
    assert_ne!(now.stroke.color, later.stroke.color);
}

#[test]
fn reduced_motion_holds_loading_still_and_gives_animations_back() {
    let context = egui::Context::default();
    install(&context);
    let installed = context.style_of(egui::Theme::Dark);
    // When the next frame of a waiting screen is asked for, if at all.
    let next_frame = || {
        let mut delay = Duration::MAX;
        for _ in 0..2 {
            let raw = egui::RawInput {
                focused: true,
                ..Default::default()
            };
            let mut output = context.run_ui(raw, |ui| {
                skeleton_fill(ui);
                spinner(ui);
            });
            delay = output
                .viewport_output
                .remove(&egui::ViewportId::ROOT)
                .unwrap()
                .repaint_delay;
            output.drop_without_applying_deltas();
        }
        delay
    };
    assert!(next_frame() <= ANIMATION_FRAME);

    reduce_motion(&context, true);
    for mode in [egui::Theme::Dark, egui::Theme::Light] {
        let style = context.style_of(mode);
        assert_eq!(style.animation_time, 0.0);
        assert_eq!(style.scroll_animation, egui::style::ScrollAnimation::none());
    }
    assert_eq!(next_frame(), Duration::MAX);

    reduce_motion(&context, false);
    let restored = context.style_of(egui::Theme::Dark);
    assert_eq!(restored.animation_time, installed.animation_time);
    assert_eq!(restored.scroll_animation, installed.scroll_animation);
    assert!(next_frame() <= ANIMATION_FRAME);
}

#[test]
fn following_the_system_draws_in_its_mode_and_otherwise_in_the_chosen_one() {
    let context = egui::Context::default();
    install(&context);
    let drawn = |system_theme| {
        let raw = egui::RawInput {
            system_theme,
            ..Default::default()
        };
        context.run_ui(raw, |_| {}).drop_without_applying_deltas();
        context.theme()
    };

    follow_system(&context, Appearance::Dark);
    assert_eq!(drawn(Some(egui::Theme::Light)), egui::Theme::Light);
    assert_eq!(drawn(Some(egui::Theme::Dark)), egui::Theme::Dark);
    // Where the system tells none, the scheme last chosen.
    follow_system(&context, Appearance::Light);
    assert_eq!(drawn(None), egui::Theme::Light);
    apply(&context, Appearance::Dark);
    assert_eq!(drawn(Some(egui::Theme::Light)), egui::Theme::Dark);
}

#[cfg(target_os = "windows")]
#[test]
fn a_title_bar_gets_its_theme_again_when_windows_may_have_taken_it() {
    let context = egui::Context::default();
    install(&context);
    let mut title_bar = TitleBar::default();
    let mut pass = |focused, system_theme| {
        let raw = egui::RawInput {
            focused,
            system_theme,
            ..Default::default()
        };
        let mut output = context.run_ui(raw, |ui| title_bar.keep(ui.ctx()));
        let commands = output
            .viewport_output
            .remove(&egui::ViewportId::ROOT)
            .map(|viewport| viewport.commands)
            .unwrap_or_default();
        output.drop_without_applying_deltas();
        commands
    };
    let dark = vec![egui::ViewportCommand::SetTheme(egui::SystemTheme::Dark)];
    let light = vec![egui::ViewportCommand::SetTheme(egui::SystemTheme::Light)];
    let windows_light = Some(egui::Theme::Light);
    let windows_dark = Some(egui::Theme::Dark);

    // egui's own request, which the bar keeps until something changes.
    assert_eq!(pass(true, windows_light), dark);
    assert!(pass(true, windows_light).is_empty());
    // Windows changed theme, which winit gave the title bar too.
    assert_eq!(pass(true, windows_dark), dark);
    // Back in front: a setting may have changed meanwhile.
    assert!(pass(false, windows_dark).is_empty());
    assert_eq!(pass(true, windows_dark), dark);
    // egui's request for the new theme, not repeated.
    apply(&context, Appearance::Light);
    assert_eq!(pass(true, windows_dark), light);
    assert!(pass(true, windows_dark).is_empty());
    // Following the system hands the bar back to Windows' own theme.
    let system = vec![egui::ViewportCommand::SetTheme(
        egui::SystemTheme::SystemDefault,
    )];
    follow_system(&context, Appearance::Light);
    assert_eq!(pass(true, windows_dark), system);
    assert!(pass(false, windows_dark).is_empty());
    assert_eq!(pass(true, windows_dark), system);
}

#[test]
fn skeleton_pulse_stays_between_its_theme_colors() {
    for colors in [DARK, LIGHT] {
        assert_eq!(
            skeleton_fill_at(SKELETON_PERIOD_SECONDS * 0.25, colors),
            colors.skeleton_high
        );
        assert_eq!(
            skeleton_fill_at(SKELETON_PERIOD_SECONDS * 0.75, colors),
            colors.skeleton_low
        );
    }
}

#[test]
fn titlebar_inset_tracks_fullscreen_and_platform() {
    let context = egui::Context::default();
    if cfg!(target_os = "macos") {
        assert_eq!(titlebar_inset(&context), 28.0);
        context.data_mut(|data| {
            data.insert_temp(egui::Id::new("native-titlebar-height"), 35.0_f32);
        });
        assert_eq!(titlebar_inset(&context), 35.0);
        context.set_zoom_factor(1.75);
        context
            .run_ui(egui::RawInput::default(), |_| {})
            .drop_without_applying_deltas();
        assert_eq!(titlebar_inset(&context), 20.0);
        let mut raw = egui::RawInput::default();
        raw.viewports.insert(
            egui::ViewportId::ROOT,
            egui::ViewportInfo {
                fullscreen: Some(true),
                ..Default::default()
            },
        );
        context.run_ui(raw, |_| {}).drop_without_applying_deltas();
        assert_eq!(titlebar_inset(&context), 0.0);
    } else {
        assert_eq!(titlebar_inset(&context), 0.0);
    }
}

#[test]
fn top_panel_frame_adds_inset_to_panel_padding() {
    let zero_inset = top_panel_frame(DARK.sidebar, 0.0);
    assert_eq!(zero_inset.inner_margin.top, PANEL_PADDING);
    assert_eq!(zero_inset.inner_margin.left, PANEL_PADDING);
    assert_eq!(zero_inset.inner_margin.right, PANEL_PADDING);
    assert_eq!(zero_inset.inner_margin.bottom, PANEL_PADDING);

    let inset_frame = top_panel_frame(DARK.sidebar, 28.0);
    assert_eq!(inset_frame.inner_margin.top, PANEL_PADDING + 28);
    assert_eq!(inset_frame.inner_margin.left, PANEL_PADDING);
    assert_eq!(inset_frame.inner_margin.right, PANEL_PADDING);
    assert_eq!(inset_frame.inner_margin.bottom, PANEL_PADDING);
    assert_eq!(
        top_panel_frame(DARK.sidebar, 20.1).inner_margin.top,
        PANEL_PADDING + 21
    );
}
