//! The accounts the desktop signs in to. Each keeps its session in core under
//! a profile of its own, and one is open at a time.

use ruston_core::DEFAULT_SESSION_PROFILE;
use serde::{Deserialize, Serialize};

/// The most accounts there can be.
pub const MAX_ACCOUNTS: usize = 5;

/// One account: the session profile it signs in under and, once it has
/// signed in, its address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub profile: String,
    #[serde(default)]
    pub address: Option<String>,
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
    /// The profile of the account open now.
    pub fn active_profile(&self) -> &str {
        &self.active
    }

    /// Notes the address the open account signed in as; with none given,
    /// the one it had stays.
    pub fn signed_in(&mut self, address: Option<&str>) {
        let open = self
            .list
            .iter_mut()
            .find(|account| account.profile == self.active);
        if let (Some(account), Some(address)) = (open, address) {
            account.address = Some(address.to_owned());
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
        accounts.signed_in(Some("alex@proton.me"));
        // A session that names none leaves the one known.
        accounts.signed_in(None);
        assert_eq!(accounts.list, [account("ruston", Some("alex@proton.me"))]);
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
