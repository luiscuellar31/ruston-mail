use eframe::egui::{self, Align, Layout};

use super::{identity, mailbox::detail, reader, theme};
use crate::app::{Answering, Compose, ComposeField, Message, Sending};
use crate::mail::SendError;

const WINDOW_SIZE: [f32; 2] = [640.0, 600.0];
const MIN_WINDOW_SIZE: [f32; 2] = [480.0, 420.0];

pub(super) fn viewport_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("compose-window")
}

pub(super) fn show_in_pane(ui: &mut egui::Ui, compose: &Compose, messages: &mut Vec<Message>) {
    ui.scope(|ui| {
        ui.spacing_mut().scroll = theme::panel_scroll_style();
        egui::ScrollArea::vertical()
            .id_salt("compose-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| page(ui, compose, messages));
    });
}

pub(super) fn window_viewport(compose: &Compose) -> egui::ViewportBuilder {
    identity::viewport()
        .with_title(heading(compose))
        .with_inner_size(WINDOW_SIZE)
        .with_min_inner_size(MIN_WINDOW_SIZE)
}

pub(super) fn show_window(
    context: &egui::Context,
    compose: &Compose,
    blocked: bool,
) -> Vec<Message> {
    context.show_viewport_immediate(viewport_id(), window_viewport(compose), |root, _class| {
        let mut messages = Vec::new();
        root.add_enabled_ui(!blocked, |root| show(root, compose, &mut messages));

        if blocked {
            if root.input(|input| input.viewport().close_requested()) {
                root.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::CancelClose);
            }
            return Vec::new();
        }

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
        } else if root.input(|input| input.modifiers.command && input.key_pressed(egui::Key::Enter))
            && compose.not_ready().is_none()
            && !compose.in_flight()
        {
            messages.push(Message::Send);
        }

        messages
    })
}

fn show(root: &mut egui::Ui, compose: &Compose, messages: &mut Vec<Message>) {
    egui::CentralPanel::default()
        .frame(theme::panel_frame(theme::colors(root).panel))
        .show(root, |ui| show_in_pane(ui, compose, messages));
}

fn page(ui: &mut egui::Ui, compose: &Compose, messages: &mut Vec<Message>) {
    let sending = compose.sending();
    let leaving = sending == Sending::InFlight;

    let heading = heading(compose);
    ui.horizontal_wrapped(|ui| {
        ui.heading(egui::RichText::new(heading).size(24.0));
        ui.with_layout(
            Layout::right_to_left(Align::Center).with_main_wrap(true),
            |ui| {
                // Nothing is offered while the message is on its way: it is out
                // of the sender's hands, and a second press must not send twice.
                let ready = compose.not_ready().is_none();
                if ui
                    .add_enabled(
                        ready && !leaving,
                        egui::Button::new(if leaving {
                            "Sending…"
                        } else if sending == Sending::Failed(SendError::Unconfirmed) {
                            "Send again"
                        } else {
                            "Send"
                        })
                        .fill(theme::colors(ui).accent_soft)
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
            },
        );
    });
    ui.add_space(10.0);

    if compose.confirming_discard() {
        theme::card(ui)
            .stroke(egui::Stroke::new(1.0, theme::colors(ui).danger))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new("Discard unsaved message?")
                            .strong()
                            .color(theme::colors(ui).danger),
                    );
                    ui.with_layout(
                        Layout::right_to_left(Align::Center).with_main_wrap(true),
                        |ui| {
                            if ui
                                .button(
                                    egui::RichText::new("Discard").color(theme::colors(ui).danger),
                                )
                                .clicked()
                            {
                                messages.push(Message::DiscardCompose);
                            }
                            if ui.button("Keep writing").clicked() {
                                messages.push(Message::CancelDiscard);
                            }
                        },
                    );
                });
            });
        ui.add_space(10.0);
    }

    if let Some(answer) = compose.answering() {
        answered(ui, answer, compose.asks_for_recipients());
    }

    if compose.asks_for_recipients() {
        let mut left = line(ui, "To", ComposeField::To, compose, leaving, messages).lost_focus();
        if compose.showing_more() {
            left |= line(ui, "Cc", ComposeField::Cc, compose, leaving, messages).lost_focus();
            left |= line(ui, "Bcc", ComposeField::Bcc, compose, leaving, messages).lost_focus();
        }
        // Addresses are looked up once typing has moved on, never per keystroke.
        if left {
            messages.push(Message::ComposeRecipientsLeft);
        }
        if let Some(notice) = compose.protection_notice() {
            detail(ui, &notice.message());
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

    // A send result takes priority over validation hints.
    match sending {
        Sending::Failed(error) => {
            ui.label(
                egui::RichText::new(error.message())
                    .small()
                    .color(theme::colors(ui).danger),
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
    fn a_small_composer_keeps_actions_visible_and_fields_scrollable() {
        let context = egui::Context::default();
        theme::install(&context);
        let (mut app, _) = crate::app::App::boot(true, crate::settings::Settings::default());
        app.update(Message::OpenCompose);
        app.update(Message::ToggleComposeCopies);
        let window = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(410.0, 240.0));
        let mut messages = Vec::new();
        for _ in 0..3 {
            let output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(window),
                    ..Default::default()
                },
                |ui| show(ui, app.compose().unwrap(), &mut messages),
            );
            for label in ["Send", "Discard", "Attach…"] {
                let (rect, clip) = output
                    .shapes
                    .iter()
                    .find_map(|clipped| {
                        if let egui::epaint::Shape::Text(text) = &clipped.shape
                            && text.galley.text() == label
                        {
                            return Some((
                                egui::Rect::from_min_size(text.pos, text.galley.size()),
                                clipped.clip_rect,
                            ));
                        }
                        None
                    })
                    .unwrap();
                assert!(
                    window.contains_rect(rect) && clip.contains_rect(rect),
                    "{label} is clipped: {rect:?} in {clip:?}"
                );
            }
            output.drop_without_applying_deltas();
        }
        assert!(!messages.iter().any(|message| matches!(
            message,
            Message::Send | Message::CloseCompose | Message::ComposeChanged(_, _)
        )));
    }

    #[test]
    fn the_composer_names_recipients_whose_copy_is_not_encrypted() {
        use crate::app::Effect;
        use futures::executor::block_on;

        let context = egui::Context::default();
        theme::install(&context);
        let (mut app, _) = crate::app::App::boot(true, crate::settings::Settings::default());
        app.update(Message::OpenCompose);
        app.update(Message::ComposeChanged(
            ComposeField::To,
            "ada@proton.me, plain@example.com".into(),
        ));
        let shown = |app: &crate::app::App| {
            let mut messages = Vec::new();
            let output = context.run_ui(egui::RawInput::default(), |ui| {
                show(ui, app.compose().unwrap(), &mut messages);
            });
            let found = output.shapes.iter().any(|clipped| {
                matches!(&clipped.shape, egui::epaint::Shape::Text(text)
                    if text.galley.text().contains("unencrypted to: plain@example.com"))
            });
            output.drop_without_applying_deltas();
            found
        };
        assert!(!shown(&app));

        let effect = app
            .update(Message::ComposeRecipientsLeft)
            .into_iter()
            .next()
            .unwrap();
        let Effect::Future(lookup) = effect else {
            panic!("the lookup runs off the UI thread");
        };
        app.update(block_on(lookup));

        assert!(shown(&app));
    }

    #[test]
    fn leaving_a_recipient_field_asks_how_the_message_will_be_protected() {
        let context = egui::Context::default();
        theme::install(&context);
        let (mut app, _) = crate::app::App::boot(true, crate::settings::Settings::default());
        app.update(Message::OpenCompose);
        let frame = |events: Vec<egui::Event>| {
            let mut messages = Vec::new();
            context
                .run_ui(
                    egui::RawInput {
                        events,
                        ..Default::default()
                    },
                    |ui| show(ui, app.compose().unwrap(), &mut messages),
                )
                .drop_without_applying_deltas();
            messages
                .iter()
                .any(|message| matches!(message, Message::ComposeRecipientsLeft))
        };

        frame(Vec::new());
        context.memory_mut(|memory| memory.request_focus(egui::Id::new(("compose", "To"))));
        assert!(!frame(Vec::new()), "entering the field looks nothing up");
        assert!(!frame(Vec::new()), "staying in the field looks nothing up");

        let tab = egui::Event::Key {
            key: egui::Key::Tab,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        // Focus moves during the frame that sees the key; the field reports it by the next one.
        let asked = [frame(vec![tab]), frame(Vec::new())];
        assert!(asked.contains(&true));
        assert!(!frame(Vec::new()), "the lookup is asked for once");
    }

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
