use eframe::egui::{self, Align, Layout};

use super::theme;
use crate::app::Message;
use crate::mail::{BodyFormat, MailFolder};
use crate::settings::{Appearance, ComposePlacement, Settings, StartFolder};

pub(super) fn show(
    root: &mut egui::Ui,
    current: &Settings,
    draft: &mut Settings,
    messages: &mut Vec<Message>,
    notification_status: super::notifications::Status,
) {
    egui::Panel::bottom("settings-actions")
        .frame(
            theme::panel_frame(theme::colors(root).panel).outer_margin(egui::Margin {
                top: theme::PANEL_PADDING,
                ..Default::default()
            }),
        )
        .show(root, |ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Back to mail").clicked() {
                    messages.push(Message::ShowSettings(false));
                }

                let changes = preference_changes(current, draft);
                if ui
                    .add_enabled(
                        !changes.is_empty(),
                        egui::Button::new("Apply")
                            .fill(theme::colors(ui).accent_soft)
                            .stroke(egui::Stroke::new(1.0, theme::ACCENT)),
                    )
                    .clicked()
                {
                    messages.extend(changes);
                }
            });
        });

    root.scope(|ui| {
        ui.spacing_mut().scroll = theme::panel_scroll_style();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| page(ui, draft, messages, notification_status));
    });
}

#[derive(Clone, Copy)]
pub(super) enum ExitDecision {
    KeepEditing,
    Discard,
    Apply,
}

pub(super) fn confirm_exit(context: &egui::Context) -> Option<ExitDecision> {
    let mut decision = None;
    let width = (context.content_rect().width() - 48.0).clamp(0.0, 440.0);
    let frame = egui::Frame::popup(&context.style_of(context.theme())).inner_margin(20);
    let modal = egui::Modal::new(egui::Id::new("unsaved-settings"))
        .frame(frame)
        .show(context, |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing = egui::vec2(12.0, 12.0);
            ui.heading("Unsaved settings");
            ui.label("Apply your changes before leaving Settings?");
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add(egui::Button::new("Keep editing").min_size(egui::vec2(0.0, 32.0)))
                    .clicked()
                {
                    decision = Some(ExitDecision::KeepEditing);
                }
                if ui
                    .add(egui::Button::new("Discard changes").min_size(egui::vec2(0.0, 32.0)))
                    .clicked()
                {
                    decision = Some(ExitDecision::Discard);
                }
                if ui
                    .add(
                        egui::Button::new("Apply and continue")
                            .min_size(egui::vec2(0.0, 32.0))
                            .fill(theme::colors(ui).accent_soft)
                            .stroke(egui::Stroke::new(1.0, theme::ACCENT)),
                    )
                    .clicked()
                {
                    decision = Some(ExitDecision::Apply);
                }
            });
        });
    if decision.is_none() && modal.should_close() {
        decision = Some(ExitDecision::KeepEditing);
    }
    decision
}

fn page(
    ui: &mut egui::Ui,
    settings: &mut Settings,
    messages: &mut Vec<Message>,
    notification_status: super::notifications::Status,
) {
    ui.heading(egui::RichText::new("Settings").size(24.0));
    ui.add_space(18.0);

    section(ui, "Reading", |ui| {
        ui.checkbox(
            &mut settings.mark_read_on_open,
            "Mark mail as read when I open it",
        );
        description(
            ui,
            "With this off, mail stays unread until you mark it yourself.",
        );
        ui.add_space(6.0);

        ui.checkbox(
            &mut settings.expand_all_messages,
            "Open every message in a conversation",
        );
        description(
            ui,
            "With this off, only the newest opens and the rest wait behind their headers.",
        );
        ui.add_space(6.0);

        ui.checkbox(
            &mut settings.show_quoted_text,
            "Show quoted text without unfolding it",
        );
        description(
            ui,
            "Quoted passages are the thread repeated under a reply, so they stay folded by default.",
        );
    });
    ui.add_space(14.0);
    section(ui, "Links and images", |ui| {
        ui.checkbox(&mut settings.confirm_links, "Ask before opening a link");
        description(
            ui,
            "The prompt shows the real destination, which helps expose misleading links.",
        );
        description(
            ui,
            "Images in mail are never downloaded. This protects your privacy from tracking pixels.",
        );
    });
    ui.add_space(14.0);
    section(ui, "Writing", |ui| {
        ui.horizontal_wrapped(|ui| {
            for (placement, label) in [
                (ComposePlacement::ReadingPane, "Reading pane"),
                (ComposePlacement::Window, "New window"),
            ] {
                ui.selectable_value(&mut settings.compose_placement, placement, label);
            }
        });
        description(ui, "Where Write, Reply and Forward open.");
        ui.add_space(10.0);

        ui.horizontal_wrapped(|ui| {
            for (format, label) in [
                (BodyFormat::PlainText, "Plain text"),
                (BodyFormat::Html, "HTML"),
            ] {
                ui.selectable_value(&mut settings.compose_format, format, label);
            }
        });
        description(
            ui,
            "How a message you write is sent. Either way you write text: a tag you type is shown as you typed it, never obeyed.",
        );
    });
    ui.add_space(14.0);
    section(ui, "Starting up", |ui| {
        egui::ComboBox::from_id_salt("start-folder")
            .selected_text(start_label(settings.start))
            .width(180.0)
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut settings.start,
                    StartFolder::LastRead,
                    "Where I left off",
                );
                for folder in MailFolder::ALL {
                    ui.selectable_value(
                        &mut settings.start,
                        StartFolder::Always(folder),
                        folder.name(),
                    );
                }
            });
        description(
            ui,
            "A folder the account made cannot be pinned: it could be renamed or gone by the next run.",
        );
    });
    ui.add_space(14.0);
    section(ui, "Interface", |ui| {
        ui.label("Appearance");
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut settings.appearance, Appearance::Dark, "Dark");
            ui.selectable_value(&mut settings.appearance, Appearance::Light, "Light");
        });
        ui.add_space(8.0);
        egui::ComboBox::from_id_salt("interface-scale")
            .selected_text(zoom_label(settings.zoom))
            .width(100.0)
            .show_ui(ui, |ui| {
                for step in ZOOM_STEPS {
                    ui.selectable_value(&mut settings.zoom, step, zoom_label(step));
                }
            });
        description(
            ui,
            "Scales everything, not the text alone, so the window keeps its proportions.",
        );
        ui.add_space(8.0);
        #[cfg(target_os = "macos")]
        {
            ui.checkbox(
                &mut settings.show_unread_badge,
                "Show unread count badge on the Dock icon",
            );
            description(
                ui,
                "Updates the Dock icon with the number of unread messages in your inbox.",
            );
            ui.add_space(8.0);
        }
        ui.checkbox(
            &mut settings.desktop_notifications,
            "Show desktop notifications for new mail",
        );
        description(
            ui,
            "Displays a system notification with the sender and subject when new mail arrives.",
        );
        if settings.desktop_notifications {
            description(ui, notification_status.description());
        }
        ui.add_space(12.0);
        if ui.button("Reset window and pane sizes").clicked() {
            messages.push(Message::ResetLayout);
        }
        description(ui, "Restores the default layout immediately.");
    });
    ui.add_space(14.0);
    section(ui, "Keyboard", |ui| {
        description(
            ui,
            "A key a focused field takes never reaches the mailbox, so these stay out of the way while you type.",
        );
        ui.add_space(6.0);
        for (keys, what) in SHORTCUTS {
            shortcut(ui, keys, what);
        }
    });
    ui.add_space(14.0);
    about(ui, messages);
}

fn about(ui: &mut egui::Ui, messages: &mut Vec<Message>) -> egui::Response {
    section(ui, "About", |ui| {
        ui.horizontal_top(|ui| {
            ui.add(
                egui::Image::new(egui::include_image!("../../assets/ui/ruston-mail-128.png"))
                    .fit_to_exact_size(egui::Vec2::splat(40.0))
                    .alt_text("Ruston Mail logo"),
            );
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new(concat!("Ruston Mail ", env!("CARGO_PKG_VERSION")))
                        .size(18.0)
                        .strong(),
                );
                description(ui, "A native Proton Mail client.");
            });
        });
        ui.add_space(8.0);
        description(
            ui,
            "Built with Rust, egui, and proton-crypto. Not affiliated with Proton.",
        );
        ui.add(
            egui::AtomLayout::new((
                egui::RichText::new("Made with").color(theme::colors(ui).muted),
                egui::Image::new(egui::include_image!(
                    "../../assets/icons/bootstrap/heart.svg"
                ))
                .fit_to_exact_size(egui::Vec2::splat(14.0))
                .tint(theme::ACCENT)
                .alt_text("heart"),
                egui::RichText::new(concat!(
                    "by Luis Cuellar. ",
                    env!("CARGO_PKG_LICENSE"),
                    " License."
                ))
                .color(theme::colors(ui).muted),
            ))
            .gap(ui.spacing().item_spacing.x),
        );
        ui.add_space(8.0);
        if ui.add(theme::compact_button("Source code")).clicked() {
            messages.push(Message::OpenSourceCode);
        }
    })
}

pub(super) fn preference_changes(current: &Settings, draft: &Settings) -> Vec<Message> {
    let mut messages = Vec::new();
    if draft.mark_read_on_open != current.mark_read_on_open {
        messages.push(Message::SetMarkReadOnOpen(draft.mark_read_on_open));
    }
    if draft.confirm_links != current.confirm_links {
        messages.push(Message::SetConfirmLinks(draft.confirm_links));
    }
    if draft.expand_all_messages != current.expand_all_messages {
        messages.push(Message::SetExpandAllMessages(draft.expand_all_messages));
    }
    if draft.show_quoted_text != current.show_quoted_text {
        messages.push(Message::SetShowQuotedText(draft.show_quoted_text));
    }
    if draft.compose_placement != current.compose_placement {
        messages.push(Message::SetComposePlacement(draft.compose_placement));
    }
    if draft.compose_format != current.compose_format {
        messages.push(Message::SetComposeFormat(draft.compose_format));
    }
    if draft.start != current.start {
        messages.push(Message::SetStartFolder(draft.start));
    }
    if (draft.zoom - current.zoom).abs() > f32::EPSILON {
        messages.push(Message::SetZoom(draft.zoom));
    }
    if draft.appearance != current.appearance {
        messages.push(Message::SetAppearance(draft.appearance));
    }
    if draft.show_unread_badge != current.show_unread_badge {
        messages.push(Message::SetShowUnreadBadge(draft.show_unread_badge));
    }
    if draft.desktop_notifications != current.desktop_notifications {
        messages.push(Message::SetDesktopNotifications(
            draft.desktop_notifications,
        ));
    }
    messages
}

fn start_label(start: StartFolder) -> &'static str {
    match start {
        StartFolder::LastRead => "Where I left off",
        StartFolder::Always(folder) => folder.name(),
    }
}

fn zoom_label(zoom: f32) -> String {
    format!("{:.0}%", zoom * 100.0)
}

/// Fixed usable zoom levels; loaded settings clamp other values.
const ZOOM_STEPS: [f32; 6] = [0.9, 1.0, 1.15, 1.3, 1.5, 1.75];

/// What the mailbox already answers to. Listed here because a shortcut no one
/// can find is a shortcut no one uses; the keys themselves live in `ui::mod`.
const SHORTCUTS: [(&str, &str); 10] = [
    ("j  /  Down", "Open the next conversation"),
    ("k  /  Up", "Open the previous one"),
    (
        "Enter",
        "Open the selected conversation, or retry a failed one. In the search field, search all mail",
    ),
    (
        "Esc",
        "Back out one layer: settings, link prompt, search, reader",
    ),
    (COMMAND_R, "Refresh the folder"),
    (COMMAND_F, "Jump to the search field"),
    (COMMAND_COMMA, "Open or close Settings"),
    (COMMAND_N, "Write a new message"),
    (COMMAND_ENTER, "Send the message being written"),
    (COMMAND_TRASH, "Move selected conversation to Trash"),
];

#[cfg(target_os = "macos")]
const COMMAND_R: &str = "Cmd + R";
#[cfg(target_os = "macos")]
const COMMAND_F: &str = "Cmd + F";
#[cfg(target_os = "macos")]
const COMMAND_COMMA: &str = "Cmd + ,";
#[cfg(target_os = "macos")]
const COMMAND_N: &str = "Cmd + N";
#[cfg(target_os = "macos")]
const COMMAND_ENTER: &str = "Cmd + Enter";
#[cfg(target_os = "macos")]
const COMMAND_TRASH: &str = "Cmd + Backspace";

#[cfg(not(target_os = "macos"))]
const COMMAND_R: &str = "Ctrl + R";
#[cfg(not(target_os = "macos"))]
const COMMAND_F: &str = "Ctrl + F";
#[cfg(not(target_os = "macos"))]
const COMMAND_COMMA: &str = "Ctrl + ,";
#[cfg(not(target_os = "macos"))]
const COMMAND_N: &str = "Ctrl + N";
#[cfg(not(target_os = "macos"))]
const COMMAND_ENTER: &str = "Ctrl + Enter";
#[cfg(not(target_os = "macos"))]
const COMMAND_TRASH: &str = "Delete / Ctrl + Backspace";

/// One key and what it does, with the keys in a column of their own so the
/// descriptions line up however wide the window is.
fn shortcut(ui: &mut egui::Ui, keys: &str, what: &str) {
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(KEYS_WIDTH, 0.0),
            Layout::right_to_left(Align::TOP),
            |ui| {
                ui.add(egui::Label::new(
                    egui::RichText::new(keys).monospace().size(12.0),
                ));
            },
        );
        ui.vertical(|ui| description(ui, what));
    });
}

const KEYS_WIDTH: f32 = 120.0;

fn description(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).color(theme::colors(ui).muted));
}

fn section(ui: &mut egui::Ui, title: &str, content: impl FnOnce(&mut egui::Ui)) -> egui::Response {
    theme::card(ui)
        .show(ui, |ui| {
            // Cards fill the page instead of sizing to their longest line.
            ui.set_width(ui.available_width());
            ui.heading(egui::RichText::new(title).size(17.0));
            ui.add_space(4.0);
            content(ui);
        })
        .response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_rect(shape: &egui::epaint::Shape, label: &str) -> Option<egui::Rect> {
        match shape {
            egui::epaint::Shape::Text(text) if text.galley.text() == label => {
                Some(egui::Rect::from_min_size(text.pos, text.galley.size()))
            }
            egui::epaint::Shape::Vec(shapes) => {
                shapes.iter().find_map(|shape| text_rect(shape, label))
            }
            _ => None,
        }
    }

    #[test]
    fn about_is_the_last_settings_section() {
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
                    &mut Settings::default(),
                    &mut Vec::new(),
                    super::super::notifications::Status::Off,
                )
            },
        );
        let find = |label| {
            output
                .shapes
                .iter()
                .find_map(|shape| text_rect(&shape.shape, label))
                .unwrap()
        };
        assert!(find("About").top() > find("Keyboard").bottom());
        assert!(
            find(concat!("Ruston Mail ", env!("CARGO_PKG_VERSION"))).top() > find("About").top()
        );
        assert!(find("Source code").top() > find("About").top());
        output.drop_without_applying_deltas();
    }

    #[test]
    fn about_fits_both_themes_and_source_button_requests_the_repository() {
        for appearance in [Appearance::Dark, Appearance::Light] {
            for width in [240.0, 720.0] {
                let context = egui::Context::default();
                theme::install(&context);
                theme::apply(&context, appearance);
                let viewport =
                    egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 480.0));
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
                            card = about(ui, &mut messages).rect;
                        },
                    )
                };
                draw(Vec::new()).drop_without_applying_deltas();
                let output = draw(Vec::new());
                let button = output
                    .shapes
                    .iter()
                    .find_map(|shape| text_rect(&shape.shape, "Source code"))
                    .unwrap();
                assert!(viewport.contains_rect(button));
                for clipped in &output.shapes {
                    if let egui::epaint::Shape::Text(text) = &clipped.shape {
                        assert!(viewport.contains_rect(egui::Rect::from_min_size(
                            text.pos,
                            text.galley.size()
                        )));
                    }
                }
                output.drop_without_applying_deltas();
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
                assert!(viewport.contains_rect(card));
                assert!(matches!(messages.as_slice(), [Message::OpenSourceCode]));
            }
        }
    }

    #[test]
    fn unsaved_settings_actions_fit_at_large_zoom() {
        fn find_text(shape: &egui::epaint::Shape, label: &str) -> Option<egui::Rect> {
            match shape {
                egui::epaint::Shape::Text(text) if text.galley.text() == label => {
                    Some(egui::Rect::from_min_size(text.pos, text.galley.size()))
                }
                egui::epaint::Shape::Vec(shapes) => {
                    shapes.iter().find_map(|shape| find_text(shape, label))
                }
                _ => None,
            }
        }

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
                    .find_map(|shape| find_text(&shape.shape, label)),
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
        draft.folder = crate::mail::Folder::System(MailFolder::Spam);
        assert!(preference_changes(&current, &draft).is_empty());

        draft.confirm_links = false;
        draft.zoom = 1.15;
        draft.show_unread_badge = false;
        draft.desktop_notifications = false;
        draft.appearance = Appearance::Light;
        let changes = preference_changes(&current, &draft);
        assert_eq!(changes.len(), 5);
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
}
