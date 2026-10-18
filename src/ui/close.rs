//! Root-window confirmations; the draft and the changes on their way to
//! Proton stay in App.

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

/// Holds the main window open, once asked to close, while changes are still
/// on their way to Proton, since closing would lose them. It closes once they
/// arrive, or at once when asked again.
#[derive(Debug, Default)]
pub(super) struct OnceSent {
    waiting: bool,
}

impl OnceSent {
    /// Whether a request to close, with `unsent` changes on their way, waits
    /// for them. A second request while waiting does not.
    pub(super) fn holds(&mut self, unsent: usize) -> bool {
        self.waiting = unsent > 0 && !self.waiting;
        self.waiting
    }

    /// Whether the close that waited can go ahead, everything having arrived.
    pub(super) fn arrived(&mut self, unsent: usize) -> bool {
        let arrived = self.waiting && unsent == 0;
        if arrived {
            self.waiting = false;
        }
        arrived
    }

    pub(super) fn waiting(&self) -> bool {
        self.waiting
    }

    /// Keeps the window open after all.
    pub(super) fn cancel(&mut self) {
        self.waiting = false;
    }
}

/// What to do while the window waits for changes to reach Proton.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Unsent {
    KeepOpen,
    CloseNow,
}

pub(super) fn confirm_unsent(context: &egui::Context, unsent: usize) -> Option<Unsent> {
    let detail = if unsent == 1 {
        "Ruston Mail closes once Proton has your last change. Closing now would lose it.".to_owned()
    } else {
        format!(
            "Ruston Mail closes once Proton has your last {unsent} changes. Closing now would \
             lose them."
        )
    };
    theme::confirmation(
        context,
        "close-with-unsent-changes",
        "Finishing your changes",
        &detail,
        &[
            ("Keep open", Emphasis::Secondary, Unsent::KeepOpen),
            ("Close now", Emphasis::Danger, Unsent::CloseNow),
        ],
        Unsent::KeepOpen,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_waits_once_for_changes_on_their_way() {
        let mut close = OnceSent::default();
        assert!(!close.holds(0), "nothing to wait for");

        assert!(close.holds(2));
        assert!(!close.arrived(1));
        assert!(close.arrived(0), "everything arrived, so it closes");
        assert!(!close.waiting());

        // Asking again while it waits closes at once.
        assert!(close.holds(1));
        assert!(!close.holds(1));
        assert!(!close.waiting());

        // Keeping the window open forgets the close it waited for.
        assert!(close.holds(1));
        close.cancel();
        assert!(!close.arrived(0));
    }

    #[test]
    fn escape_keeps_the_window_open_while_changes_are_on_their_way() {
        let context = egui::Context::default();
        context
            .run_ui(egui::RawInput::default(), |_| {
                assert!(confirm_unsent(&context, 2).is_none());
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
                assert_eq!(confirm_unsent(&context, 2), Some(Unsent::KeepOpen));
            })
            .drop_without_applying_deltas();
    }

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
