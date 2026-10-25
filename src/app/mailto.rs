//! Email links wait in memory until the shell can safely show a new composer.

use super::{App, ComposeField, Effects};
use crate::mailto::{MAX_PENDING, Request};

impl App {
    pub(super) fn receive_mailto(&mut self, request: Request) {
        // A demo process never signs in, so once its fictional mailbox is
        // closed a link would wait for a sign-in that cannot happen.
        if self.demo_process && !self.auth_state.is_authenticated() {
            self.mailto_error = Some(
                "This window was started as a demo and cannot open email links. Close it and start Ruston Mail without RUSTON_DEMO.",
            );
            return;
        }
        if self.pending_mailto.len() == MAX_PENDING {
            self.mailto_error = Some("Too many email links are waiting. Please try again later.");
            return;
        }
        self.pending_mailto.push_back(request);
    }

    pub fn pending_mailto_count(&self) -> usize {
        self.pending_mailto.len()
    }

    pub fn mailto_error(&self) -> Option<&str> {
        self.mailto_error
    }

    pub fn can_open_pending_mailto(&self) -> bool {
        self.auth_state.is_authenticated()
            && !self.pending_mailto.is_empty()
            && self.compose.is_none()
            && !self.showing_settings
            && self.pending_link.is_none()
    }

    pub(super) fn open_pending_mailto(&mut self) -> Effects {
        if !self.can_open_pending_mailto() {
            return Effects::none();
        }
        let Some(request) = self.pending_mailto.pop_front() else {
            return Effects::none();
        };
        let (to, subject, body) = request.into_fields();
        let opened = self.open_compose();
        self.change_compose(ComposeField::To, to);
        self.change_compose(ComposeField::Subject, subject);
        self.change_compose(ComposeField::Body, body);
        opened
    }
}
