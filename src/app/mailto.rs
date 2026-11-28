//! Email links wait in memory until the shell can safely show a new composer.

use super::{App, ComposeField, Effects};
use crate::mailto::{MAX_PENDING, Request};

/// Why an email link did not open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailtoError {
    /// It was no link the composer can take.
    Unreadable,
    /// The window was started as a demo, which never signs in.
    Demo,
    /// As many links as may wait were waiting already.
    TooManyWaiting,
}

impl MailtoError {
    pub fn message(self) -> &'static str {
        match self {
            Self::Unreadable => "This email link could not be opened.",
            Self::Demo => {
                "This window was started as a demo and cannot open email links. Close it and start Ruston Mail without RUSTON_DEMO."
            }
            Self::TooManyWaiting => "Too many email links are waiting. Please try again later.",
        }
    }
}

impl App {
    pub(super) fn receive_mailto(&mut self, request: Request) {
        // A demo process never signs in, so once its fictional mailbox is
        // closed a link would wait for a sign-in that cannot happen.
        if self.demo_process && !self.auth_state.is_authenticated() {
            self.session.mailto_error = Some(MailtoError::Demo);
            return;
        }
        if self.session.pending_mailto.len() == MAX_PENDING {
            self.session.mailto_error = Some(MailtoError::TooManyWaiting);
            return;
        }
        self.session.pending_mailto.push_back(request);
    }

    pub fn pending_mailto_count(&self) -> usize {
        self.session.pending_mailto.len()
    }

    pub fn mailto_error(&self) -> Option<MailtoError> {
        self.session.mailto_error
    }

    pub fn can_open_pending_mailto(&self) -> bool {
        self.auth_state.is_authenticated()
            && !self.session.pending_mailto.is_empty()
            && self.compose.is_none()
            && !self.showing_settings
            && self.session.pending_link.is_none()
    }

    pub(super) fn open_pending_mailto(&mut self) -> Effects {
        if !self.can_open_pending_mailto() {
            return Effects::none();
        }
        let Some(request) = self.session.pending_mailto.pop_front() else {
            return Effects::none();
        };
        let opened = self.open_compose();
        self.change_compose(ComposeField::To, request.to());
        self.change_compose(ComposeField::Subject, request.subject().to_owned());
        self.change_compose(ComposeField::Body, request.body().to_owned());
        opened
    }
}
