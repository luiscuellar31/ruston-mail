//! Where a profile's secrets are kept: the system's credential store, or
//! memory alone in tests.

use crate::error::{Error, Result};
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

/// Storage for a profile's secrets: its tokens, its key passphrase, and the
/// API address the tokens were issued for, which a saved session must match
/// before they are sent anywhere.
pub trait SecretStore: Send + Sync {
    /// Saves `value` as `key`'s, in place of any value it had.
    fn set(&self, key: &str, value: &str) -> Result<()>;
    /// The value saved as `key`'s; `None` when there is none.
    fn get(&self, key: &str) -> Result<Option<String>>;
    /// Forgets `key`'s value. One that is not there is no error.
    fn delete(&self, key: &str) -> Result<()>;
}

/// A profile's secrets in the system's credential store: the macOS
/// Keychain, the Windows Credential Manager, or the Secret Service.
pub struct KeyringStore {
    profile: String,
}

impl KeyringStore {
    /// The store of `profile`'s secrets.
    pub fn new(profile: impl Into<String>) -> Self {
        KeyringStore {
            profile: profile.into(),
        }
    }

    /// The service and account names of the credential that holds `key`.
    /// Saved sessions are found by them, so they must not change.
    fn credential_names(&self, key: &str) -> (&'static str, String) {
        (crate::STORAGE_NAME, format!("{}:{key}", self.profile))
    }

    /// The credential that holds `key`, whether or not it has a value yet.
    fn credential(&self, key: &str) -> Result<keyring::Entry> {
        let (service, account) = self.credential_names(key);
        keyring::Entry::new(service, &account).map_err(keyring_error("open"))
    }
}

/// Turns a credential store failure into a session error that names the
/// `step` that failed.
fn keyring_error(step: &'static str) -> impl FnOnce(keyring::Error) -> Error {
    move |error| Error::Session(format!("keyring {step}: {error}"))
}

/// What a credential store `outcome` gives, with a credential that is not
/// there as `None` rather than an error.
fn unless_absent<T>(outcome: keyring::Result<T>) -> keyring::Result<Option<T>> {
    match outcome {
        Err(keyring::Error::NoEntry) => Ok(None),
        found => found.map(Some),
    }
}

impl SecretStore for KeyringStore {
    fn set(&self, key: &str, value: &str) -> Result<()> {
        let credential = self.credential(key)?;
        credential.set_password(value).map_err(keyring_error("set"))
    }

    fn get(&self, key: &str) -> Result<Option<String>> {
        let credential = self.credential(key)?;
        unless_absent(credential.get_password()).map_err(keyring_error("get"))
    }

    fn delete(&self, key: &str) -> Result<()> {
        let credential = self.credential(key)?;
        let deleted = unless_absent(credential.delete_credential());
        deleted.map(|_| ()).map_err(keyring_error("delete"))
    }
}

/// Secrets held in memory alone, for tests.
#[derive(Default)]
pub struct MemoryStore {
    values: Mutex<HashMap<String, String>>,
}

impl MemoryStore {
    /// A store with nothing in it.
    pub fn new() -> Self {
        Self::default()
    }

    /// The values, locked for one operation.
    fn values(&self) -> MutexGuard<'_, HashMap<String, String>> {
        self.values.lock().unwrap()
    }
}

impl SecretStore for MemoryStore {
    fn set(&self, key: &str, value: &str) -> Result<()> {
        self.values().insert(key.to_owned(), value.to_owned());
        Ok(())
    }

    fn get(&self, key: &str) -> Result<Option<String>> {
        Ok(self.values().get(key).cloned())
    }

    fn delete(&self, key: &str) -> Result<()> {
        self.values().remove(key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keychain_entries_keep_their_service_and_account_names() {
        let store = KeyringStore::new("ruston");
        let names = store.credential_names("auth_tokens_v1");
        assert_eq!(names, ("ruston-mail", "ruston:auth_tokens_v1".to_owned()));
    }

    #[test]
    fn the_memory_store_keeps_the_last_value_until_it_is_deleted() {
        let store = MemoryStore::new();
        assert_eq!(store.get("tokens").unwrap(), None);
        store.set("tokens", "first").unwrap();
        store.set("tokens", "second").unwrap();
        store.set("other", "kept").unwrap();
        assert_eq!(store.get("tokens").unwrap().as_deref(), Some("second"));

        store.delete("tokens").unwrap();
        // Deleting what is not there is no error.
        store.delete("tokens").unwrap();
        assert_eq!(store.get("tokens").unwrap(), None);
        assert_eq!(store.get("other").unwrap().as_deref(), Some("kept"));
    }
}
