//! Several accounts: switching to another, adding one, and going back from
//! the sign-in screen. One is open at a time; the others keep their sessions
//! saved, each under a profile of its own, and nothing of one reaches the
//! next, as when signing out.

use super::{App, AuthState, Effects, Message};
use crate::accounts::{Account, Accounts};
use crate::mail::ProtonMailService;

impl App {
    /// The accounts signed in to, and the one open now.
    pub fn accounts(&self) -> &Accounts {
        &self.settings.accounts
    }

    /// Whether this process was started for the demo, whose one fictional
    /// account never changes.
    pub fn is_demo_process(&self) -> bool {
        self.demo_process
    }

    /// Whether another account can be added: outside the demo, while there
    /// are fewer than the most there can be.
    pub fn can_add_account(&self) -> bool {
        !self.demo_process && self.settings.accounts.free_profile().is_some()
    }

    /// Whether the open account can be left for another: signed in, outside
    /// the demo, and with no message being sent or written, which the
    /// interface asks about first.
    pub fn can_change_account(&self) -> bool {
        !self.demo_process
            && self.auth_state.is_authenticated()
            && !self
                .compose
                .as_ref()
                .is_some_and(|draft| draft.in_flight() || !draft.is_untouched())
    }

    /// The account the sign-in screen offers to go back to, if any.
    pub fn returning_to(&self) -> Option<&Account> {
        let profile = self.returning_to.as_deref()?;
        if self.auth_state != AuthState::SignedOut {
            return None;
        }
        self.settings.accounts.get(profile)
    }

    /// The address the account being opened last signed in as; none for an
    /// account being added.
    pub(super) fn open_account_address(&self) -> Option<String> {
        if self.adding_account.is_some() {
            return None;
        }
        let accounts = &self.settings.accounts;
        accounts.get(accounts.active_profile())?.address.clone()
    }

    /// Opens the account whose session is saved under `profile`, leaving the
    /// open one's saved too.
    pub(super) fn switch_account(&mut self, profile: String) -> Effects {
        if !self.can_change_account()
            || profile == self.settings.accounts.active_profile()
            || self.settings.accounts.get(&profile).is_none()
        {
            return Effects::none();
        }
        let previous = self.settings.accounts.active_profile().to_owned();
        self.close_mailbox(None);
        self.returning_to = Some(previous);
        self.remember(|settings| {
            settings.accounts.open(&profile);
        });
        self.open_saved_session()
    }

    /// Leaves the open account, its session saved, for the sign-in screen,
    /// where another account signs in under a profile of its own.
    pub(super) fn add_account(&mut self) -> Effects {
        if !self.can_change_account() {
            return Effects::none();
        }
        let Some(profile) = self.settings.accounts.free_profile() else {
            return Effects::none();
        };
        let previous = self.settings.accounts.active_profile().to_owned();
        self.close_mailbox(None);
        self.returning_to = Some(previous);
        self.adding_account = Some(profile);
        Effects::none()
    }

    /// From the sign-in screen, opens again the account open before it.
    pub(super) fn return_to_account(&mut self) -> Effects {
        if self.returning_to().is_none() {
            return Effects::none();
        }
        let Some(profile) = self.returning_to.take() else {
            return Effects::none();
        };
        self.adding_account = None;
        self.login_form.clear_all();
        self.remember(|settings| {
            settings.accounts.open(&profile);
        });
        self.open_saved_session()
    }

    /// The accounts at `addresses` signed in to in turn, each added after the
    /// first, the last one open, with the demo mailbox standing in for their
    /// mail.
    #[cfg(test)]
    pub(crate) fn signed_in_to(addresses: &[&str]) -> Self {
        let mut app = Self::new(crate::settings::Settings::default());
        for (n, address) in addresses.iter().enumerate() {
            if n > 0 {
                let _ = app.add_account();
            }
            let _ = app.open_mailbox(
                super::MailBackend::demo(),
                Some((*address).to_owned()),
                Some(format!("user-{}", n + 1)),
            );
        }
        app
    }

    /// Opens the session saved for the account being opened, as Ruston does
    /// when it starts.
    fn open_saved_session(&mut self) -> Effects {
        self.auth_error = None;
        self.resume_may_retry = false;
        let profile = self.session_profile();
        self.session_saved = ProtonMailService::has_saved_session(&profile);
        self.auth_state = AuthState::CheckingSession;
        Effects::perform(ProtonMailService::resume(profile.clone()), move |outcome| {
            Message::SessionChecked(profile, outcome)
        })
    }
}
