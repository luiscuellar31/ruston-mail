use eframe::egui::{self, Align, Layout};

use super::theme::{self, Emphasis};
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
                        theme::primary(egui::Button::new("Apply")),
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
    theme::confirmation(
        context,
        "unsaved-settings",
        "Unsaved settings",
        "Apply your changes before leaving Settings?",
        &[
            (
                "Keep editing",
                Emphasis::Secondary,
                ExitDecision::KeepEditing,
            ),
            ("Discard changes", Emphasis::Danger, ExitDecision::Discard),
            ("Apply and continue", Emphasis::Primary, ExitDecision::Apply),
        ],
        ExitDecision::KeepEditing,
    )
}

fn page(
    ui: &mut egui::Ui,
    settings: &mut Settings,
    messages: &mut Vec<Message>,
    notification_status: super::notifications::Status,
) {
    ui.heading(egui::RichText::new("Settings").size(24.0).strong());
    ui.add_space(18.0);

    section(ui, "Reading", |ui| {
        ui.add(theme::Checkbox::new(
            &mut settings.mark_read_on_open,
            "Mark mail as read when I open it",
        ));
        description(
            ui,
            "With this off, mail stays unread until you mark it yourself.",
        );
        ui.add_space(6.0);

        ui.add(theme::Checkbox::new(
            &mut settings.expand_all_messages,
            "Open every message in a conversation",
        ));
        description(
            ui,
            "With this off, only the newest opens and the rest wait behind their headers.",
        );
        ui.add_space(6.0);

        ui.add(theme::Checkbox::new(
            &mut settings.show_quoted_text,
            "Show quoted text without unfolding it",
        ));
        description(
            ui,
            "Quoted passages are the thread repeated under a reply, so they stay folded by default.",
        );
    });
    ui.add_space(14.0);
    section(ui, "Links and images", |ui| {
        ui.add(theme::Checkbox::new(
            &mut settings.confirm_links,
            "Ask before opening a link",
        ));
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
        // eframe gives only the main window an accessibility tree.
        description(
            ui,
            "Screen readers cannot read a new window yet, so with one, keep the reading pane.",
        );
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
        // winit tells the system's mode on macOS and Windows, not on Linux.
        let offer_system = !cfg!(target_os = "linux");
        ui.horizontal_wrapped(|ui| {
            // Following the system keeps the scheme last chosen, for where
            // the system tells none.
            if offer_system
                && ui
                    .selectable_label(settings.follow_system_appearance, "System")
                    .clicked()
            {
                settings.follow_system_appearance = true;
            }
            for (appearance, label) in [(Appearance::Dark, "Dark"), (Appearance::Light, "Light")] {
                let chosen =
                    !settings.follow_system_appearance && settings.appearance == appearance;
                if ui.selectable_label(chosen, label).clicked() {
                    settings.follow_system_appearance = false;
                    settings.appearance = appearance;
                }
            }
        });
        if offer_system {
            description(ui, "System follows your computer's light or dark mode.");
        }
        ui.add_space(8.0);
        ui.add(theme::Checkbox::new(
            &mut settings.system_font,
            "Use system interface font",
        ));
        description(ui, "Keeps text sizes. Uses Ruston's font if unavailable.");
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
            ui.add(theme::Checkbox::new(
                &mut settings.show_unread_badge,
                "Show unread count badge on the Dock icon",
            ));
            description(
                ui,
                "Updates the Dock icon with the number of unread messages in your inbox.",
            );
            ui.add_space(8.0);
        }
        #[cfg(target_os = "windows")]
        {
            ui.add(theme::Checkbox::new(
                &mut settings.show_unread_badge,
                "Show unread count on the taskbar button",
            ));
            description(
                ui,
                "Marks the taskbar button with the number of unread messages in your inbox, \
                 while badges are on in Windows' taskbar settings.",
            );
            ui.add_space(8.0);
        }
        #[cfg(target_os = "linux")]
        {
            ui.add(theme::Checkbox::new(
                &mut settings.show_unread_badge,
                "Show unread count on the dock icon",
            ));
            description(
                ui,
                "Shows the number of unread messages in your inbox on docks that support it, \
                 such as Ubuntu's dock and KDE's task manager.",
            );
            ui.add_space(8.0);
        }
        ui.add(theme::Checkbox::new(
            &mut settings.desktop_notifications,
            "Show desktop notifications for new mail",
        ));
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
    section(ui, "Advanced", |ui| {
        ui.add(theme::Checkbox::new(
            &mut settings.check_recipient_protection,
            "Check recipient encryption while writing",
        ));
        description(
            ui,
            "When you leave a recipient field, Proton is asked for that address's keys to show whether the message will be end-to-end encrypted. With this off, nothing is asked until you send.",
        );
        ui.add_space(6.0);

        ui.add(theme::Checkbox::new(
            &mut settings.accept_international_domains,
            "Accept international domain names",
        ));
        description(
            ui,
            "Lets you write to a domain such as münchen.de. It is sent in its ASCII form, xn--mnchen-3ya.de, which the composer shows beside it: letters from another script can imitate a familiar domain. Email links still refuse such domains.",
        );
    });
    ui.add_space(14.0);
    section(ui, "Keyboard", |ui| {
        description(
            ui,
            "A key a focused field takes never reaches the mailbox, so these stay out of the way while you type.",
        );
        ui.add_space(6.0);
        let width = keys_width(ui);
        for (keys, what) in SHORTCUTS {
            shortcut(ui, keys, what, width);
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
    if draft.check_recipient_protection != current.check_recipient_protection {
        messages.push(Message::SetCheckRecipientProtection(
            draft.check_recipient_protection,
        ));
    }
    if draft.accept_international_domains != current.accept_international_domains {
        messages.push(Message::SetAcceptInternationalDomains(
            draft.accept_international_domains,
        ));
    }
    if draft.start != current.start {
        messages.push(Message::SetStartFolder(draft.start));
    }
    if (draft.zoom - current.zoom).abs() > f32::EPSILON {
        messages.push(Message::SetZoom(draft.zoom));
    }
    let scheme_changed = draft.appearance != current.appearance;
    if scheme_changed || (current.follow_system_appearance && !draft.follow_system_appearance) {
        messages.push(Message::SetAppearance(draft.appearance));
    }
    // Choosing a scheme stops following the system, so following comes after.
    if draft.follow_system_appearance && (scheme_changed || !current.follow_system_appearance) {
        messages.push(Message::SetFollowSystemAppearance(true));
    }
    if draft.system_font != current.system_font {
        messages.push(Message::SetSystemFont(draft.system_font));
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

/// A step of the interface zoom from the keyboard or the View menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ZoomStep {
    In,
    Out,
    Reset,
}

/// The zoom a step leads to from `current`: the next level Settings offers
/// up or down, staying put at either end, or the default.
pub(super) fn zoom_step(current: f32, step: ZoomStep) -> f32 {
    const NEAR: f32 = 0.001;
    let found = match step {
        ZoomStep::In => ZOOM_STEPS.into_iter().find(|level| *level > current + NEAR),
        ZoomStep::Out => ZOOM_STEPS
            .into_iter()
            .rev()
            .find(|level| *level < current - NEAR),
        ZoomStep::Reset => Some(Settings::default().zoom),
    };
    found.unwrap_or(current)
}

/// What the mailbox already answers to. Listed here because a shortcut no one
/// can find is a shortcut no one uses; the keys themselves live in `ui::mod`.
const SHORTCUTS: [(&str, &str); 11] = [
    ("j  /  Down", "Open the next conversation"),
    ("k  /  Up", "Open the previous one"),
    (
        "Enter",
        "Open the selected conversation, or retry a failed one. In the search field, search all mail",
    ),
    (
        "Esc",
        "Close a menu, or back out one layer: link prompt, settings, search, reader",
    ),
    (COMMAND_R, "Refresh the folder"),
    (COMMAND_F, "Jump to the search field"),
    (COMMAND_COMMA, "Open or close Settings"),
    (COMMAND_N, "Write a new message"),
    (COMMAND_ENTER, "Send the message being written"),
    (COMMAND_TRASH, "Move selected conversation to Trash"),
    (COMMAND_ZOOM, "Zoom in, zoom out, or back to 100%"),
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
#[cfg(target_os = "macos")]
const COMMAND_ZOOM: &str = "Cmd + Plus / Minus / 0";

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
#[cfg(not(target_os = "macos"))]
const COMMAND_ZOOM: &str = "Ctrl + Plus / Minus / 0";

/// One key and what it does, with the keys in a column of their own so the
/// descriptions line up however wide the window is.
fn shortcut(ui: &mut egui::Ui, keys: &str, what: &str, keys_width: f32) {
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(keys_width, 0.0),
            Layout::right_to_left(Align::TOP),
            |ui| {
                ui.add(egui::Label::new(key_text(keys)));
            },
        );
        ui.vertical(|ui| description(ui, what));
    });
}

fn key_text(keys: &str) -> egui::RichText {
    egui::RichText::new(keys).monospace().size(12.0)
}

/// The width of the keys column: its widest key, or more. A key wider than
/// its column would reach past the section's edge and pull the sections
/// under it to the left.
fn keys_width(ui: &egui::Ui) -> f32 {
    SHORTCUTS
        .iter()
        .map(|(keys, _)| {
            egui::WidgetText::from(key_text(keys))
                .into_galley(
                    ui,
                    Some(egui::TextWrapMode::Extend),
                    f32::INFINITY,
                    egui::TextStyle::Body,
                )
                .size()
                .x
        })
        .fold(KEYS_MIN_WIDTH, f32::max)
}

const KEYS_MIN_WIDTH: f32 = 120.0;

fn description(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).color(theme::colors(ui).muted));
}

fn section(ui: &mut egui::Ui, title: &str, content: impl FnOnce(&mut egui::Ui)) -> egui::Response {
    theme::card(ui)
        .show(ui, |ui| {
            // Cards fill the page instead of sizing to their longest line.
            ui.set_width(ui.available_width());
            ui.heading(egui::RichText::new(title).size(17.0).strong());
            ui.add_space(4.0);
            content(ui);
        })
        .response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::test_support::shape_text_rect;

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
                    &mut Settings::default(),
                    &mut Vec::new(),
                    super::super::notifications::Status::Off,
                )
            },
        );
        let lefts: Vec<(String, f32)> = SHORTCUTS
            .iter()
            .chain([&("About", "")])
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
                .find_map(|shape| shape_text_rect(&shape.shape, label))
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
    fn advanced_choices_share_a_card_after_the_everyday_ones() {
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
                .find_map(|shape| shape_text_rect(&shape.shape, label))
                .unwrap_or_else(|| panic!("missing {label}"))
        };
        let advanced = find("Advanced");
        assert!(advanced.top() > find("Interface").top());
        for choice in [
            "Check recipient encryption while writing",
            "Accept international domain names",
        ] {
            let choice = find(choice);
            assert!(choice.top() > advanced.top());
            assert!(choice.bottom() < find("Keyboard").top());
        }
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
                    .find_map(|shape| shape_text_rect(&shape.shape, "Source code"))
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
        draft.system_font = true;
        draft.check_recipient_protection = false;
        draft.accept_international_domains = true;
        let changes = preference_changes(&current, &draft);
        assert_eq!(changes.len(), 8);
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
