use eframe::egui::{self, Align, Layout};

use super::{mailbox::detail, reader, theme};
use crate::app::{Answering, Compose, ComposeField, Message, Sending};

const WINDOW_SIZE: [f32; 2] = [640.0, 600.0];
const MIN_WINDOW_SIZE: [f32; 2] = [480.0, 420.0];

pub(super) fn viewport_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("compose-window")
}

pub(super) fn show_in_pane(ui: &mut egui::Ui, compose: &Compose, messages: &mut Vec<Message>) {
    page(ui, compose, messages);
}

pub(super) fn show_window(context: &egui::Context, compose: &Compose) -> Vec<Message> {
    context.show_viewport_immediate(
        viewport_id(),
        egui::ViewportBuilder::default()
            .with_title(heading(compose))
            .with_inner_size(WINDOW_SIZE)
            .with_min_inner_size(MIN_WINDOW_SIZE),
        |root, _class| {
            let mut messages = Vec::new();
            show(root, compose, &mut messages);

            if root.input(|input| input.viewport().close_requested()) {
                if compose.in_flight() {
                    root.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::CancelClose);
                } else if compose.is_untouched() || compose.confirming_discard() {
                    messages.push(Message::DiscardCompose);
                } else {
                    root.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::CancelClose);
                    messages.push(Message::CloseCompose);
                }
            } else if root.input(|input| input.key_pressed(egui::Key::Escape)) {
                if compose.confirming_discard() {
                    messages.push(Message::CancelDiscard);
                } else {
                    messages.push(Message::CloseCompose);
                }
            } else if root
                .input(|input| input.modifiers.command && input.key_pressed(egui::Key::Enter))
                && compose.not_ready().is_none()
                && !compose.in_flight()
            {
                messages.push(Message::Send);
            }

            messages
        },
    )
}

fn show(root: &mut egui::Ui, compose: &Compose, messages: &mut Vec<Message>) {
    egui::CentralPanel::default()
        .frame(theme::panel_frame(theme::PANEL))
        .show(root, |ui| page(ui, compose, messages));
}

fn page(ui: &mut egui::Ui, compose: &Compose, messages: &mut Vec<Message>) {
    let sending = compose.sending();
    let leaving = sending == Sending::InFlight;

    let heading = heading(compose);
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
            if ui
                .add_enabled(!leaving, egui::Button::new("Attach…"))
                .clicked()
            {
                messages.push(Message::PickComposeAttachments);
            }
        });
    });
    ui.add_space(10.0);

    if compose.confirming_discard() {
        theme::card()
            .stroke(egui::Stroke::new(1.0, theme::DANGER))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Discard unsaved message?")
                            .strong()
                            .color(theme::DANGER),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .button(egui::RichText::new("Discard").color(theme::DANGER))
                            .clicked()
                        {
                            messages.push(Message::DiscardCompose);
                        }
                        if ui.button("Keep writing").clicked() {
                            messages.push(Message::CancelDiscard);
                        }
                    });
                });
            });
        ui.add_space(10.0);
    }

    if let Some(answer) = compose.answering() {
        answered(ui, answer, compose.asks_for_recipients());
    }

    if compose.asks_for_recipients() {
        line(ui, "To", ComposeField::To, compose, leaving, messages);
        if compose.showing_more() {
            line(ui, "Cc", ComposeField::Cc, compose, leaving, messages);
            line(ui, "Bcc", ComposeField::Bcc, compose, leaving, messages);
        }
        let more = if compose.showing_more() {
            "Fewer options"
        } else {
            "More options"
        };
        if ui
            .add_enabled(!leaving, theme::compact_button(more))
            .clicked()
        {
            messages.push(Message::ToggleComposeCopies);
        }
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
    ui.add_space(6.0);

    if !compose.attachments().is_empty() {
        detail(ui, "Attachments");
        for (index, path) in compose.attachments().iter().enumerate() {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("Attachment");
            let size = std::fs::metadata(path)
                .ok()
                .map(|m| format!(" ({})", reader::size_label(m.len())))
                .unwrap_or_default();
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("📎 {name}{size}"))
                        .color(ui.visuals().text_color()),
                );
                if ui
                    .add_enabled(!leaving, theme::compact_button("Remove"))
                    .clicked()
                {
                    messages.push(Message::RemoveComposeAttachment(index));
                }
            });
        }
        ui.add_space(4.0);
    }

    let attach_label = if compose.attachments().is_empty() {
        "Attach files…"
    } else {
        "Attach more…"
    };
    if ui
        .add_enabled(!leaving, theme::compact_button(attach_label))
        .clicked()
    {
        messages.push(Message::PickComposeAttachments);
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
            .margin(egui::Margin::same(8))
            .desired_width(f32::INFINITY)
            .desired_rows(16),
    );
    if response.changed() {
        messages.push(Message::ComposeChanged(ComposeField::Body, body));
    }
}

fn heading(compose: &Compose) -> &'static str {
    match compose.answering() {
        None => "New message",
        Some(answer) if answer.everyone => "Reply to everyone",
        Some(_) if !compose.asks_for_recipients() => "Reply",
        Some(_) => "Forward",
    }
}

/// What is being answered.
/// Proton determines reply recipients from the original message.
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
) -> egui::Response {
    detail(ui, label);
    let mut value = compose.field(field).to_owned();
    let response = ui.add_enabled(
        !leaving,
        theme::text_field(&mut value)
            .id(egui::Id::new(("compose", label)))
            .desired_width(f32::INFINITY),
    );
    if response.changed() {
        messages.push(Message::ComposeChanged(field, value));
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compose_fields_start_at_the_left_and_are_taller_than_the_default() {
        let context = egui::Context::default();
        let mut geometry = None;
        context
            .run_ui(egui::RawInput::default(), |ui| {
                ui.set_width(400.0);
                let left = ui.available_rect_before_wrap().left();
                let mut messages = Vec::new();
                let field = line(
                    ui,
                    "To",
                    ComposeField::To,
                    &Compose::default(),
                    false,
                    &mut messages,
                );

                let mut plain = String::new();
                let plain = ui.add(egui::TextEdit::singleline(&mut plain));
                geometry = Some((left, field.rect, plain.rect.height()));
            })
            .drop_without_applying_deltas();

        let (left, field, plain_height) = geometry.expect("the field was laid out");
        assert!((field.left() - left).abs() <= 1.0);
        assert!(field.height() >= plain_height + 8.0);
    }
}
