use eframe::egui::{self, Align, Layout};

use super::{mailbox::detail, theme};
use crate::app::Message;
use crate::settings::Settings;

const PAGE_WIDTH: f32 = 620.0;

pub(super) fn show(root: &mut egui::Ui, settings: &Settings, messages: &mut Vec<Message>) {
    egui::CentralPanel::default().show(root, |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.set_max_width(PAGE_WIDTH);
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
                if ui.checkbox(&mut on, "Mark mail as read when I open it").changed() {
                    messages.push(Message::SetMarkReadOnOpen(on));
                }
                detail(ui, "With this off, mail stays unread until you mark it yourself.");
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
            section(ui, "Remembered automatically", |ui| {
                detail(
                    ui,
                    "Window size, pane widths and the last folder return the way you left them.",
                );
            });
        });
    });
}

fn section(ui: &mut egui::Ui, title: &str, content: impl FnOnce(&mut egui::Ui)) {
    theme::card().show(ui, |ui| {
        ui.heading(egui::RichText::new(title).size(17.0));
        ui.add_space(4.0);
        content(ui);
    });
}
