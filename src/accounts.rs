//! The accounts the desktop signs in to. Each keeps its session in core under
//! a profile of its own, and one is open at a time.

use ruston_core::DEFAULT_SESSION_PROFILE;
use serde::{Deserialize, Serialize};

use crate::mail::Folder;

/// The most accounts there can be.
pub const MAX_ACCOUNTS: usize = 5;

/// One account: the session profile it signs in under and, once it has
/// signed in, its address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub profile: String,
    #[serde(default)]
    pub address: Option<String>,
    /// The last folder read in this account; none for its Inbox.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    folder: Option<Folder>,
}

/// The accounts, in the order they were added, and the profile of the one
/// open now. The first account's profile is the CLI's default, `ruston`, so a
/// sign-in in one app is resumed by the other; the others' are `ruston-2` to
/// `ruston-5`, which the CLI reaches with `--profile`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Accounts {
    list: Vec<Account>,
    active: String,
}

impl Default for Accounts {
    fn default() -> Self {
        Self {
            list: vec![Account {
                profile: DEFAULT_SESSION_PROFILE.to_owned(),
                address: None,
                folder: None,
            }],
            active: DEFAULT_SESSION_PROFILE.to_owned(),
        }
    }
}

/// The profile of the account in `slot`, from 0.
fn profile_at(slot: usize) -> String {
    if slot == 0 {
        DEFAULT_SESSION_PROFILE.to_owned()
    } else {
        format!("{DEFAULT_SESSION_PROFILE}-{}", slot + 1)
    }
}

impl Accounts {
    /// The accounts, in the order they were added.
    pub fn all(&self) -> &[Account] {
        &self.list
    }

    /// The profile of the account open now.
    pub fn active_profile(&self) -> &str {
        &self.active
    }

    /// The account whose session is saved under `profile`, if one is.
    pub fn get(&self, profile: &str) -> Option<&Account> {
        self.list.iter().find(|account| account.profile == profile)
    }

    /// The account under `profile`, to change.
    fn get_mut(&mut self, profile: &str) -> Option<&mut Account> {
        self.list
            .iter_mut()
            .find(|account| account.profile == profile)
    }

    /// The last folder read in the open account, its Inbox until another is.
    pub fn folder(&self) -> Folder {
        self.get(&self.active)
            .and_then(|account| account.folder.clone())
            .unwrap_or(Folder::INBOX)
    }

    /// Notes `folder` as the last one read in the open account.
    pub fn set_folder(&mut self, folder: Folder) {
        let active = self.active.clone();
        if let Some(account) = self.get_mut(&active) {
            account.folder = (folder != Folder::INBOX).then_some(folder);
        }
    }

    /// The profile a new account signs in under: the first slot no account
    /// takes, none once there are [`MAX_ACCOUNTS`].
    pub fn free_profile(&self) -> Option<String> {
        (0..MAX_ACCOUNTS)
            .map(profile_at)
            .find(|profile| self.get(profile).is_none())
    }

    /// Opens the account under `profile`; false when there is none.
    pub fn open(&mut self, profile: &str) -> bool {
        let known = self.get(profile).is_some();
        if known {
            self.active = profile.to_owned();
        }
        known
    }

    /// Notes that `profile` signed in, as `address` when one is given, and
    /// opens it, adding it when it is a free slot's; with no address, the
    /// one it had stays. Another address there is another account, which
    /// starts in its own Inbox.
    pub fn signed_in(&mut self, profile: &str, address: Option<&str>) {
        if self.get(profile).is_none() {
            if self.free_profile().as_deref() != Some(profile) {
                return;
            }
            self.list.push(Account {
                profile: profile.to_owned(),
                address: None,
                folder: None,
            });
        }
        if let Some(account) = self.get_mut(profile)
            && let Some(address) = address
        {
            if account
                .address
                .as_deref()
                .is_some_and(|known| !known.eq_ignore_ascii_case(address))
            {
                account.folder = None;
            }
            account.address = Some(address.to_owned());
        }
        self.active = profile.to_owned();
    }

    /// Forgets the account under `profile`. When it was open, the first left
    /// is; with none left, the list starts again with the first slot's.
    pub fn remove(&mut self, profile: &str) {
        self.list.retain(|account| account.profile != profile);
        match self.list.first() {
            None => *self = Self::default(),
            Some(first) if self.active == profile => self.active = first.profile.clone(),
            Some(_) => {}
        }
    }

    /// What a damaged or hand-edited file could break: a profile none of
    /// the slots has, one listed twice, so more accounts than allowed, or an
    /// open one that is not listed.
    pub(crate) fn sanitized(self) -> Self {
        let slots: Vec<String> = (0..MAX_ACCOUNTS).map(profile_at).collect();
        let mut list: Vec<Account> = Vec::new();
        for account in self.list {
            if slots.contains(&account.profile)
                && list.iter().all(|kept| kept.profile != account.profile)
            {
                list.push(account);
            }
        }
        let Some(first) = list.first() else {
            return Self::default();
        };
        let active = if list.iter().any(|account| account.profile == self.active) {
            self.active
        } else {
            first.profile.clone()
        };
        Self { list, active }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(profile: &str, address: Option<&str>) -> Account {
        Account {
            profile: profile.to_owned(),
            address: address.map(str::to_owned),
            folder: None,
        }
    }

    #[test]
    fn the_first_account_shares_the_clis_default_profile() {
        let accounts = Accounts::default();
        assert_eq!(accounts.active_profile(), "ruston");
        assert_eq!(accounts.list, [account("ruston", None)]);
        assert_eq!(
            (0..MAX_ACCOUNTS).map(profile_at).collect::<Vec<_>>(),
            ["ruston", "ruston-2", "ruston-3", "ruston-4", "ruston-5"]
        );
    }

    #[test]
    fn the_open_account_keeps_the_address_it_signed_in_as() {
        let mut accounts = Accounts::default();
        accounts.signed_in("ruston", Some("alex@proton.me"));
        // A session that names none leaves the one known.
        accounts.signed_in("ruston", None);
        assert_eq!(accounts.list, [account("ruston", Some("alex@proton.me"))]);
    }

    #[test]
    fn new_accounts_take_the_free_slots_up_to_five_and_open() {
        let mut accounts = Accounts::default();
        accounts.signed_in("ruston", Some("alex@proton.me"));
        for (slot, address) in [
            "sam@proton.me",
            "kim@proton.me",
            "lee@proton.me",
            "max@proton.me",
        ]
        .into_iter()
        .enumerate()
        {
            let profile = accounts.free_profile().unwrap();
            assert_eq!(profile, format!("ruston-{}", slot + 2));
            accounts.signed_in(&profile, Some(address));
            assert_eq!(accounts.active_profile(), profile);
        }
        assert_eq!(accounts.all().len(), MAX_ACCOUNTS);
        assert_eq!(accounts.free_profile(), None);
        // A profile that no free slot gives is not added.
        accounts.signed_in("ruston-9", Some("nine@proton.me"));
        assert_eq!(accounts.all().len(), MAX_ACCOUNTS);
        assert_eq!(accounts.active_profile(), "ruston-5");

        assert!(accounts.open("ruston-2"));
        assert_eq!(accounts.active_profile(), "ruston-2");
        assert!(!accounts.open("ruston-9"));
        assert_eq!(accounts.active_profile(), "ruston-2");
    }

    #[test]
    fn each_account_keeps_its_last_folder_until_another_signs_in_there() {
        let archive = Folder::System(crate::mail::MailFolder::Archive);
        let invoices = Folder::custom("kZ9", "Invoices");
        let mut accounts = Accounts::default();
        accounts.signed_in("ruston", Some("alex@proton.me"));
        assert_eq!(accounts.folder(), Folder::INBOX);
        accounts.set_folder(invoices.clone());
        accounts.signed_in("ruston-2", Some("sam@proton.me"));
        assert_eq!(accounts.folder(), Folder::INBOX);
        accounts.set_folder(archive.clone());

        assert!(accounts.open("ruston"));
        assert_eq!(accounts.folder(), invoices);
        // Signing in again, as the same address, keeps it.
        accounts.signed_in("ruston", Some("ALEX@proton.me"));
        accounts.signed_in("ruston", None);
        assert_eq!(accounts.folder(), invoices);
        // Another account there must not have this one's folder asked for.
        accounts.signed_in("ruston", Some("kim@proton.me"));
        assert_eq!(accounts.folder(), Folder::INBOX);
        assert!(accounts.open("ruston-2"));
        assert_eq!(accounts.folder(), archive);

        // The Inbox is no folder of its own.
        accounts.set_folder(Folder::INBOX);
        assert_eq!(accounts.get("ruston-2").unwrap().folder, None);
    }

    #[test]
    fn removing_the_open_account_opens_the_first_left_or_starts_again() {
        let mut accounts = Accounts::default();
        accounts.signed_in("ruston", Some("alex@proton.me"));
        accounts.signed_in("ruston-2", Some("sam@proton.me"));
        accounts.signed_in("ruston-3", Some("kim@proton.me"));
        accounts.remove("ruston-2");
        assert_eq!(accounts.active_profile(), "ruston-3");
        accounts.remove("ruston-3");
        assert_eq!(accounts.active_profile(), "ruston");
        // A free slot is taken again by the next account added.
        assert_eq!(accounts.free_profile().as_deref(), Some("ruston-2"));
        accounts.remove("ruston");
        assert_eq!(accounts, Accounts::default());
    }

    #[test]
    fn a_damaged_list_keeps_only_known_profiles_once_and_an_open_one() {
        let damaged = Accounts {
            list: vec![
                account("ruston-3", Some("sam@proton.me")),
                account("../elsewhere", None),
                account("ruston-3", Some("again@proton.me")),
                account("ruston-9", None),
                account("ruston", Some("alex@proton.me")),
            ],
            active: "ruston-9".to_owned(),
        };
        let kept = damaged.sanitized();
        assert_eq!(
            kept.list,
            [
                account("ruston-3", Some("sam@proton.me")),
                account("ruston", Some("alex@proton.me")),
            ]
        );
        assert_eq!(kept.active_profile(), "ruston-3");

        let nothing_usable = Accounts {
            list: vec![account("default", None)],
            active: "default".to_owned(),
        };
        assert_eq!(nothing_usable.sanitized(), Accounts::default());
    }
}
