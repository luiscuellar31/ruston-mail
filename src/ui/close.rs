//! Root-window confirmation; draft ownership stays in App.

use eframe::egui;

use super::theme::{self, Emphasis};
use crate::app::Message;

/// What discarding the written message makes way for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Leaving {
    /// Closing the main window, which quits the app.
    Window,
    /// Signing out of the account.
    SignOut,
    /// Leaving the demo mailbox.
    Demo,
}

pub(super) fn confirm(context: &egui::Context, leaving: Leaving) -> Option<Message> {
    let (heading, discard) = match leaving {
        Leaving::Window => ("Close Ruston Mail?", "Discard and close"),
        Leaving::SignOut => ("Sign out?", "Discard and sign out"),
        Leaving::Demo => ("Exit demo?", "Discard and exit"),
    };
    theme::confirmation(
        context,
        "close-with-unsaved-message",
        heading,
        "Your unsaved message will be discarded.",
        &[
            ("Keep writing", Emphasis::Secondary, Message::CancelDiscard),
            (discard, Emphasis::Danger, Message::DiscardCompose),
        ],
        Message::CancelDiscard,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_dismisses_close_confirmation_without_discarding() {
        let context = egui::Context::default();
        context
            .run_ui(egui::RawInput::default(), |_| {
                assert!(confirm(&context, Leaving::Window).is_none());
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
                assert!(matches!(
                    confirm(&context, Leaving::Window),
                    Some(Message::CancelDiscard)
                ));
            })
            .drop_without_applying_deltas();
    }
}
