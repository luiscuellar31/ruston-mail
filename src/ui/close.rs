//! Root-window confirmation; draft ownership stays in App.

use eframe::egui;

use crate::app::Message;

pub(super) fn confirm(context: &egui::Context) -> Option<Message> {
    let mut decision = None;
    let width = (context.content_rect().width() - 48.0).clamp(0.0, 440.0);
    let modal = egui::Modal::new(egui::Id::new("close-with-unsaved-message"))
        .frame(egui::Frame::popup(&context.style_of(context.theme())).inner_margin(20))
        .show(context, |ui| {
            ui.set_width(width);
            ui.heading("Close Ruston Mail?");
            ui.label("Your unsaved message will be discarded.");
            ui.horizontal_wrapped(|ui| {
                if ui.button("Keep writing").clicked() {
                    decision = Some(Message::CancelDiscard);
                }
                if ui
                    .button(
                        egui::RichText::new("Discard and close")
                            .color(super::theme::colors(ui).danger),
                    )
                    .clicked()
                {
                    decision = Some(Message::DiscardCompose);
                }
            });
        });
    if decision.is_none() && modal.should_close() {
        decision = Some(Message::CancelDiscard);
    }
    decision
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_dismisses_close_confirmation_without_discarding() {
        let context = egui::Context::default();
        context
            .run_ui(egui::RawInput::default(), |_| {
                assert!(confirm(&context).is_none());
            })
            .drop_without_applying_deltas();
        let raw = egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };
        context
            .run_ui(raw, |_| {
                assert!(matches!(confirm(&context), Some(Message::CancelDiscard)));
            })
            .drop_without_applying_deltas();
    }
}
