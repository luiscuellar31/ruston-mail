use eframe::egui::{self, Align, Layout};

use super::{mailbox::detail, theme};
use crate::app::Message;
use crate::settings::Settings;

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
            "Window size, pane widths and the last folder return the way you left them.",
        );
    });
}

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

fn section(ui: &mut egui::Ui, title: &str, content: impl FnOnce(&mut egui::Ui)) {
    theme::card().show(ui, |ui| {
        // A card fills the page. Left to size itself it stops at its longest
        // line, which made the settings look like a column with an empty
        // panel beside it.
        ui.set_width(ui.available_width());
        ui.heading(egui::RichText::new(title).size(17.0));
        ui.add_space(4.0);
        content(ui);
    });
}
