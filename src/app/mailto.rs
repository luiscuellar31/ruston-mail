//! Email links wait in memory until the shell can safely show a new composer.

use super::{App, AuthState, ComposeField};
use crate::mailto::{MAX_PENDING, Request};

impl App {
    pub(super) fn receive_mailto(&mut self, request: Request) {
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
        matches!(self.auth_state, AuthState::Authenticated { .. })
            && !self.pending_mailto.is_empty()
            && self.compose.is_none()
            && !self.showing_settings
            && self.pending_link.is_none()
    }

    pub(super) fn open_pending_mailto(&mut self) {
        if !self.can_open_pending_mailto() {
            return;
        }
        let Some(request) = self.pending_mailto.pop_front() else {
            return;
        };
        let (to, subject, body) = request.into_fields();
        self.open_compose();
        self.change_compose(ComposeField::To, to);
        self.change_compose(ComposeField::Subject, subject);
        self.change_compose(ComposeField::Body, body);
    }
}
