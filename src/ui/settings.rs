use eframe::egui::{self, Align, Layout};

use super::{mailbox::detail, theme};
use crate::app::Message;
use crate::mail::{BodyFormat, MailFolder};
use crate::settings::{Settings, StartFolder};

pub(super) fn show(root: &mut egui::Ui, settings: &Settings, messages: &mut Vec<Message>) {
    // The page fills the window, with the padding the mailbox panels use: at
    // full width the default margin left it cramped against the window edge.
    egui::CentralPanel::default()
        .frame(theme::panel_frame(theme::PANEL))
        .show(root, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| page(ui, settings, messages));
        });
}

fn page(ui: &mut egui::Ui, settings: &Settings, messages: &mut Vec<Message>) {
    ui.horizontal(|ui| {
        ui.heading(egui::RichText::new("Settings").size(24.0));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.button("Done").clicked() {
                messages.push(Message::ShowSettings(false));
            }
        });
    });
    ui.add_space(18.0);

    section(ui, "Reading", |ui| {
        let mut on = settings.mark_read_on_open;
        if ui
            .checkbox(&mut on, "Mark mail as read when I open it")
            .changed()
        {
            messages.push(Message::SetMarkReadOnOpen(on));
        }
        detail(
            ui,
            "With this off, mail stays unread until you mark it yourself.",
        );
        ui.add_space(6.0);

        let mut on = settings.expand_all_messages;
        if ui
            .checkbox(&mut on, "Open every message in a conversation")
            .changed()
        {
            messages.push(Message::SetExpandAllMessages(on));
        }
        detail(
            ui,
            "With this off, only the newest opens and the rest wait behind their headers.",
        );
        ui.add_space(6.0);

        let mut on = settings.show_quoted_text;
        if ui
            .checkbox(&mut on, "Show quoted text without unfolding it")
            .changed()
        {
            messages.push(Message::SetShowQuotedText(on));
        }
        detail(
            ui,
            "Quoted passages are the thread repeated under a reply, so they stay folded by default.",
        );
    });
    ui.add_space(14.0);
    section(ui, "Links and images", |ui| {
        let mut on = settings.confirm_links;
        if ui.checkbox(&mut on, "Ask before opening a link").changed() {
            messages.push(Message::SetConfirmLinks(on));
        }
        detail(
            ui,
            "The prompt shows the real destination, which helps expose misleading links.",
        );
        detail(
            ui,
            "Images in mail are never downloaded. This protects your privacy from tracking pixels.",
        );
    });
    ui.add_space(14.0);
    section(ui, "Writing", |ui| {
        ui.horizontal_wrapped(|ui| {
            for (format, label) in [
                (BodyFormat::PlainText, "Plain text"),
                (BodyFormat::Html, "HTML"),
            ] {
                let chosen = settings.compose_format == format;
                if ui.add(egui::Button::new(label).selected(chosen)).clicked() {
                    messages.push(Message::SetComposeFormat(format));
                }
            }
        });
        detail(
            ui,
            "How a message you write is sent. Either way you write text: a tag you type is shown as you typed it, never obeyed.",
        );
    });
    ui.add_space(14.0);
    section(ui, "Starting up", |ui| {
        ui.horizontal_wrapped(|ui| {
            start_choice(
                ui,
                settings,
                StartFolder::LastRead,
                "Where I left off",
                messages,
            );
            for folder in MailFolder::ALL {
                let start = StartFolder::Always(folder);
                start_choice(ui, settings, start, folder.name(), messages);
            }
        });
        detail(
            ui,
            "A folder the account made cannot be pinned: it could be renamed or gone by the next run.",
        );
    });
    ui.add_space(14.0);
    section(ui, "Interface", |ui| {
        ui.horizontal_wrapped(|ui| {
            for step in ZOOM_STEPS {
                let chosen = (settings.zoom - step).abs() < f32::EPSILON;
                let label = format!("{:.0}%", step * 100.0);
                if ui.add(egui::Button::new(label).selected(chosen)).clicked() {
                    messages.push(Message::SetZoom(step));
                }
            }
        });
        detail(
            ui,
            "Scales everything, not the text alone, so the window keeps its proportions.",
        );
    });
    ui.add_space(14.0);
    section(ui, "Keyboard", |ui| {
        detail(
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
        detail(
            ui,
            "Window size and pane widths return the way you left them.",
        );
    });
}

fn start_choice(
    ui: &mut egui::Ui,
    settings: &Settings,
    start: StartFolder,
    label: &str,
    messages: &mut Vec<Message>,
) {
    let chosen = settings.start == start;
    if ui.add(egui::Button::new(label).selected(chosen)).clicked() {
        messages.push(Message::SetStartFolder(start));
    }
}

/// Offered rather than a free slider: a handful of steps cannot land on a
/// scale that leaves the interface unusable, and each one is one click away.
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
        ui.vertical(|ui| detail(ui, what));
    });
}

const KEYS_WIDTH: f32 = 92.0;

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
    fn every_offered_zoom_is_one_the_settings_would_keep() {
        // The panel must not offer a scale that loading clamps away, and the
        // scale a fresh install starts at has to be one of the buttons, or
        // none of them would look chosen.
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
