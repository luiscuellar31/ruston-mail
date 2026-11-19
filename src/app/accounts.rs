//! Several accounts: switching to another, from the mailbox or the sign-in
//! screen, and adding one. One is open at a time; the others keep their
//! sessions saved, each under a profile of its own, and nothing of one
//! reaches the next, as when signing out.

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

    /// The accounts the sign-in screen offers to open instead of signing in:
    /// all but the one signing in, so one whose session ended, even at
    /// startup, holds none of the others back. None while a sign-in runs.
    pub fn other_accounts(&self) -> impl Iterator<Item = &Account> {
        let signing_in = self.session_profile();
        let idle = !self.demo_process && self.auth_state == AuthState::SignedOut;
        self.settings
            .accounts
            .all()
            .iter()
            .filter(move |account| idle && account.profile != signing_in)
    }

    /// Whether an account being added is, at `address`, one added already:
    /// as typed, before signing in, or as Proton names it once signed in,
    /// which also finds one typed as its username or another address.
    pub(super) fn adds_known_address(&self, address: &str) -> bool {
        let address = address.trim();
        self.adding_account.is_some()
            && self.settings.accounts.all().iter().any(|account| {
                account
                    .address
                    .as_deref()
                    .is_some_and(|known| known.eq_ignore_ascii_case(address))
            })
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

    /// Opens the account whose session is saved under `profile`: from the
    /// mailbox, leaving the open one's saved too, or from the sign-in screen,
    /// instead of the account signing in there.
    pub(super) fn switch_account(&mut self, profile: String) -> Effects {
        if self
            .other_accounts()
            .any(|account| account.profile == profile)
        {
            self.adding_account = None;
            self.login_form.clear_all();
        } else if self.can_change_account()
            && profile != self.settings.accounts.active_profile()
            && self.settings.accounts.get(&profile).is_some()
        {
            self.close_mailbox(None);
        } else {
            return Effects::none();
        }
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
        self.close_mailbox(None);
        self.adding_account = Some(profile);
        Effects::none()
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
    pub(super) fn open_saved_session(&mut self) -> Effects {
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
