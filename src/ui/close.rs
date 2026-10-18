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

/// Holds a close or a sign-out, once asked for, while changes are still on
/// their way to Proton, since going ahead would lose them. It goes ahead once
/// they arrive, or at once when asked again.
#[derive(Debug, Default)]
pub(super) struct OnceSent {
    waiting: Option<Leaving>,
}

impl OnceSent {
    /// Whether `leaving`, asked for while something it would cut short is
    /// `pending`, waits for it. Asking again while it waits does not.
    pub(super) fn holds(&mut self, leaving: Leaving, pending: bool) -> bool {
        let again = self.waiting == Some(leaving);
        self.waiting = (pending && !again).then_some(leaving);
        self.waiting.is_some()
    }

    /// What waits, if anything.
    pub(super) fn waiting(&self) -> Option<Leaving> {
        self.waiting
    }

    /// Stops waiting, for what waited to go ahead or to be called off.
    pub(super) fn cancel(&mut self) {
        self.waiting = None;
    }
}

/// What to do while a close or a sign-out waits for changes to reach Proton.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Unsent {
    /// Keep the window open, or stay signed in.
    Stay,
    /// Go ahead now, losing the changes.
    GoNow,
}

pub(super) fn confirm_unsent(
    context: &egui::Context,
    leaving: Leaving,
    unsent: usize,
) -> Option<Unsent> {
    let (then, going, stay, go) = match leaving {
        Leaving::Window => ("closes", "Closing", "Keep open", "Close now"),
        Leaving::SignOut => ("signs out", "Signing out", "Stay signed in", "Sign out now"),
        Leaving::Demo => ("leaves the demo", "Leaving", "Stay", "Exit now"),
    };
    let detail = if unsent == 1 {
        format!("Ruston Mail {then} once Proton has your last change. {going} now would lose it.")
    } else {
        format!(
            "Ruston Mail {then} once Proton has your last {unsent} changes. {going} now would \
             lose them."
        )
    };
    theme::confirmation(
        context,
        "close-with-unsent-changes",
        "Finishing your changes",
        &detail,
        &[
            (stay, Emphasis::Secondary, Unsent::Stay),
            (go, Emphasis::Danger, Unsent::GoNow),
        ],
        Unsent::Stay,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaving_waits_once_for_changes_on_their_way() {
        let mut close = OnceSent::default();
        assert!(!close.holds(Leaving::Window, false), "nothing to wait for");

        assert!(close.holds(Leaving::Window, true));
        assert_eq!(close.waiting(), Some(Leaving::Window));
        // Asking again while it waits goes ahead at once.
        assert!(!close.holds(Leaving::Window, true));
        assert_eq!(close.waiting(), None);

        // Signing out waits the same way, and staying forgets it.
        assert!(close.holds(Leaving::SignOut, true));
        assert_eq!(close.waiting(), Some(Leaving::SignOut));
        close.cancel();
        assert_eq!(close.waiting(), None);
    }

    #[test]
    fn escape_keeps_the_window_open_while_changes_are_on_their_way() {
        let context = egui::Context::default();
        context
            .run_ui(egui::RawInput::default(), |_| {
                assert!(confirm_unsent(&context, Leaving::Window, 2).is_none());
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
                assert_eq!(
                    confirm_unsent(&context, Leaving::Window, 2),
                    Some(Unsent::Stay)
                );
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
