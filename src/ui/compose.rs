use eframe::egui::{self, Align, Layout};

use super::{mailbox::detail, theme};
use crate::app::{Answering, Compose, ComposeField, Message, Sending};

/// How wide a recipient or subject line is before the body.
const LABEL_WIDTH: f32 = 74.0;

pub(super) fn show(root: &mut egui::Ui, compose: &Compose, messages: &mut Vec<Message>) {
    egui::CentralPanel::default()
        .frame(theme::panel_frame(theme::PANEL))
        .show(root, |ui| page(ui, compose, messages));
}

fn page(ui: &mut egui::Ui, compose: &Compose, messages: &mut Vec<Message>) {
    let sending = compose.sending();
    let leaving = sending == Sending::InFlight;

    let heading = match compose.answering() {
        None => "New message",
        Some(answer) if answer.everyone => "Reply to everyone",
        Some(_) if !compose.asks_for_recipients() => "Reply",
        Some(_) => "Forward",
    };
    ui.horizontal(|ui| {
        ui.heading(egui::RichText::new(heading).size(24.0));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            // Nothing is offered while the message is on its way: it is out
            // of the sender's hands, and a second press must not send twice.
            let ready = compose.not_ready().is_none();
            if ui
                .add_enabled(
                    ready && !leaving,
                    egui::Button::new(if leaving { "Sending…" } else { "Send" })
                        .fill(theme::ACCENT_SOFT)
                        .stroke(egui::Stroke::new(1.0, theme::ACCENT)),
                )
                .clicked()
            {
                messages.push(Message::Send);
            }
            if ui
                .add_enabled(!leaving, egui::Button::new("Discard"))
                .clicked()
            {
                messages.push(Message::CloseCompose);
            }
        });
    });
    ui.add_space(10.0);

    if let Some(answer) = compose.answering() {
        answered(ui, answer, compose.asks_for_recipients());
    }

    if compose.asks_for_recipients() {
        line(ui, "To", ComposeField::To, compose, leaving, messages);
        if compose.showing_more() {
            line(ui, "Cc", ComposeField::Cc, compose, leaving, messages);
            line(ui, "Bcc", ComposeField::Bcc, compose, leaving, messages);
        }
        ui.horizontal(|ui| {
            ui.add_space(LABEL_WIDTH);
            let more = if compose.showing_more() {
                "Fewer fields"
            } else {
                "Cc and Bcc"
            };
            if ui
                .add_enabled(!leaving, egui::Button::new(more).small())
                .clicked()
            {
                messages.push(Message::ToggleComposeCopies);
            }
        });
        ui.add_space(4.0);
    }
    if compose.asks_for_subject() {
        line(
            ui,
            "Subject",
            ComposeField::Subject,
            compose,
            leaving,
            messages,
        );
    }
    ui.add_space(10.0);

    // Whichever of the two matters: what went wrong beats what is missing,
    // because a refusal is news and an unfinished line is not.
    match sending {
        Sending::Failed(error) => {
            ui.label(
                egui::RichText::new(error.message())
                    .small()
                    .color(theme::DANGER),
            );
        }
        _ => {
            if let Some(problem) = compose.not_ready().filter(|_| !compose.is_untouched()) {
                detail(ui, &problem.message());
            }
        }
    }
    ui.add_space(6.0);

    let mut body = compose.field(ComposeField::Body).to_owned();
    let response = ui.add_enabled(
        !leaving,
        egui::TextEdit::multiline(&mut body)
            .id(egui::Id::new("compose-body"))
            .hint_text("Write your message…")
            .desired_width(f32::INFINITY)
            .desired_rows(16),
    );
    if response.changed() {
        messages.push(Message::ComposeChanged(ComposeField::Body, body));
    }
}

/// What is being answered.
///
/// It says which message, not who will receive the answer: Proton reads the
/// recipients off the message itself, and only it knows whether that message
/// asked for replies to go somewhere other than its sender.
fn answered(ui: &mut egui::Ui, answer: &Answering, addressed_here: bool) {
    detail(
        ui,
        &format!("Answering “{}” from {}.", answer.subject, answer.sender),
    );
    if !addressed_here {
        detail(
            ui,
            "Proton addresses the reply from that message, and writes the subject.",
        );
    }
    ui.add_space(8.0);
}

/// One labelled single-line field.
fn line(
    ui: &mut egui::Ui,
    label: &str,
    field: ComposeField,
    compose: &Compose,
    leaving: bool,
    messages: &mut Vec<Message>,
) {
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(LABEL_WIDTH, 0.0),
            Layout::right_to_left(Align::Center),
            |ui| {
                ui.add_space(8.0);
                detail(ui, label);
            },
        );
        let mut value = compose.field(field).to_owned();
        let response = ui.add_enabled(
            !leaving,
            egui::TextEdit::singleline(&mut value)
                .id(egui::Id::new(("compose", label)))
                .desired_width(f32::INFINITY),
        );
        if response.changed() {
            messages.push(Message::ComposeChanged(field, value));
        }
    });
}
