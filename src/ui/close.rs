//! Root-window confirmations; the draft and the changes on their way to
//! Proton stay in App.

use eframe::egui;

use super::theme::{self, Emphasis};
use crate::app::Message;

/// A way of leaving: what discarding a written message makes way for, and
/// what a [`DeferredLeave`] holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Leaving {
    /// Closing the main window, which quits the app.
    Window,
    /// Signing out of the account.
    SignOut,
    /// Leaving the demo mailbox.
    Demo,
}

/// Asks whether to discard the written message in order to leave this way.
pub(super) fn confirm_discard(context: &egui::Context, leaving: Leaving) -> Option<Message> {
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

/// Holds a close or a sign-out, once asked for, while something it would cut
/// short is under way: changes on their way to Proton, which going ahead
/// would lose, or a sign-out that has not yet forgotten the session on this
/// device. It goes ahead once that ends, or at once when asked again.
#[derive(Debug, Default)]
pub(super) struct DeferredLeave {
    waiting: Option<Leaving>,
}

impl DeferredLeave {
    /// Whether `leaving` waits, which it does while something it would cut
    /// short is `blocking` it. Asking again while it waits goes ahead at once.
    pub(super) fn defer(&mut self, leaving: Leaving, blocking: bool) -> bool {
        let again = self.waiting == Some(leaving);
        self.waiting = (blocking && !again).then_some(leaving);
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

/// What a close or a sign-out waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Holding {
    /// This many changes on their way to Proton.
    Changes(usize),
    /// A sign-out under way, until this device has forgotten the session.
    SigningOut,
}

/// What to do while a close or a sign-out waits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WaitDecision {
    /// Keep the window open, or stay signed in.
    Stay,
    /// Go ahead now, cutting short what it waited for.
    GoNow,
}

/// Says what a close or a sign-out waits for, and asks whether to keep
/// waiting.
pub(super) fn confirm_wait(
    context: &egui::Context,
    leaving: Leaving,
    holding: Holding,
) -> Option<WaitDecision> {
    let (then, going, stay, go) = match leaving {
        Leaving::Window => ("closes", "Closing", "Keep open", "Close now"),
        Leaving::SignOut => ("signs out", "Signing out", "Stay signed in", "Sign out now"),
        Leaving::Demo => ("leaves the demo", "Leaving", "Stay", "Exit now"),
    };
    let (heading, detail) = match holding {
        Holding::SigningOut => (
            "Signing out",
            format!(
                "Ruston Mail {then} once this device has forgotten your session. {going} now \
                 could leave it saved here."
            ),
        ),
        Holding::Changes(1) => (
            "Finishing your changes",
            format!(
                "Ruston Mail {then} once Proton has your last change. {going} now would lose it."
            ),
        ),
        Holding::Changes(unsent) => (
            "Finishing your changes",
            format!(
                "Ruston Mail {then} once Proton has your last {unsent} changes. {going} now would \
                 lose them."
            ),
        ),
    };
    theme::confirmation(
        context,
        "close-with-unsent-changes",
        heading,
        &detail,
        &[
            (stay, Emphasis::Secondary, WaitDecision::Stay),
            (go, Emphasis::Danger, WaitDecision::GoNow),
        ],
        WaitDecision::Stay,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaving_waits_once_for_changes_on_their_way() {
        let mut deferred = DeferredLeave::default();
        assert!(
            !deferred.defer(Leaving::Window, false),
            "nothing to wait for"
        );

        assert!(deferred.defer(Leaving::Window, true));
        assert_eq!(deferred.waiting(), Some(Leaving::Window));
        // Asking again while it waits goes ahead at once.
        assert!(!deferred.defer(Leaving::Window, true));
        assert_eq!(deferred.waiting(), None);

        // Signing out waits the same way, and staying forgets it.
        assert!(deferred.defer(Leaving::SignOut, true));
        assert_eq!(deferred.waiting(), Some(Leaving::SignOut));
        deferred.cancel();
        assert_eq!(deferred.waiting(), None);
    }

    #[test]
    fn escape_keeps_the_window_open_while_changes_are_on_their_way() {
        let context = egui::Context::default();
        context
            .run_ui(egui::RawInput::default(), |_| {
                assert!(confirm_wait(&context, Leaving::Window, Holding::Changes(2)).is_none());
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
                    confirm_wait(&context, Leaving::Window, Holding::Changes(2)),
                    Some(WaitDecision::Stay)
                );
            })
            .drop_without_applying_deltas();
    }

    #[test]
    fn escape_dismisses_close_confirmation_without_discarding() {
        let context = egui::Context::default();
        context
            .run_ui(egui::RawInput::default(), |_| {
                assert!(confirm_discard(&context, Leaving::Window).is_none());
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
                    confirm_discard(&context, Leaving::Window),
                    Some(Message::CancelDiscard)
                ));
            })
            .drop_without_applying_deltas();
    }
}
