use eframe::egui::{self, Align, Layout};

use super::theme;
use crate::app::Message;
use crate::mail::{BodyFormat, MailFolder};
use crate::settings::{ComposePlacement, Settings, StartFolder};

const WINDOW_SIZE: [f32; 2] = [720.0, 720.0];
const MIN_WINDOW_SIZE: [f32; 2] = [520.0, 420.0];

pub(super) fn viewport_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("settings-window")
}

pub(super) fn show_window(
    context: &egui::Context,
    current: &Settings,
    draft: &mut Settings,
) -> Vec<Message> {
    context.show_viewport_immediate(
        viewport_id(),
        egui::ViewportBuilder::default()
            .with_title("Settings")
            .with_inner_size(WINDOW_SIZE)
            .with_min_inner_size(MIN_WINDOW_SIZE),
        |root, _class| {
            let mut messages = Vec::new();
            show(root, current, draft, &mut messages);

            if root.input(|input| {
                input.viewport().close_requested() || input.key_pressed(egui::Key::Escape)
            }) {
                messages.push(Message::ShowSettings(false));
            }

            messages
        },
    )
}

fn show(
    root: &mut egui::Ui,
    current: &Settings,
    draft: &mut Settings,
    messages: &mut Vec<Message>,
) {
    egui::Panel::bottom("settings-actions")
        .frame(theme::panel_frame(theme::PANEL).stroke(egui::Stroke::new(1.0, theme::BORDER)))
        .show(root, |ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Close").clicked() {
                    messages.push(Message::ShowSettings(false));
                }

                let changes = preference_changes(current, draft);
                if ui
                    .add_enabled(
                        !changes.is_empty(),
                        egui::Button::new("Apply")
                            .fill(theme::ACCENT_SOFT)
                            .stroke(egui::Stroke::new(1.0, theme::ACCENT)),
                    )
                    .clicked()
                {
                    messages.extend(changes);
                }
            });
        });

    egui::CentralPanel::default()
        .frame(theme::panel_frame(theme::PANEL))
        .show(root, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| page(ui, draft));
        });
}

fn page(ui: &mut egui::Ui, settings: &mut Settings) {
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
    section(ui, "Remembered automatically", |ui| {
        description(
            ui,
            "Window size and pane widths return the way you left them.",
        );
    });
}

fn preference_changes(current: &Settings, draft: &Settings) -> Vec<Message> {
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

/// Offered rather than a free slider: a handful of steps cannot land on a
/// scale that leaves the interface unusable.
/// The settings clamp anything else a file might carry.
const ZOOM_STEPS: [f32; 6] = [0.9, 1.0, 1.15, 1.3, 1.5, 1.75];

/// What the mailbox already answers to. Listed here because a shortcut no one
/// can find is a shortcut no one uses; the keys themselves live in `ui::mod`.
const SHORTCUTS: [(&str, &str); 6] = [
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
];

#[cfg(target_os = "macos")]
const COMMAND_R: &str = "Cmd + R";
#[cfg(target_os = "macos")]
const COMMAND_F: &str = "Cmd + F";
#[cfg(not(target_os = "macos"))]
const COMMAND_R: &str = "Ctrl + R";
#[cfg(not(target_os = "macos"))]
const COMMAND_F: &str = "Ctrl + F";

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

const KEYS_WIDTH: f32 = 92.0;

fn description(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).color(theme::MUTED));
}

fn section(ui: &mut egui::Ui, title: &str, content: impl FnOnce(&mut egui::Ui)) -> egui::Response {
    theme::card()
        .show(ui, |ui| {
            // A card fills the page. Left to size itself it stops at its longest
            // line, which made the settings look like a column with an empty
            // panel beside it.
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
        let changes = preference_changes(&current, &draft);
        assert_eq!(changes.len(), 2);
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
