//! The mail client: it signs in or resumes a saved session, keeps the
//! account's unlocked keys, and offers the mail operations its submodules
//! define.

pub mod attachments;
pub mod contacts;
pub mod drafts;
pub mod export;
pub mod filters;
mod login;
pub mod organize;
pub mod read;
pub mod recipients;
pub mod send;
pub mod sync;
#[cfg(test)]
mod test_support;

pub use login::{LoginOptions, LoginPrompts, MailboxPasswordPrompt};

use crate::api;
use crate::cache::CacheIdentity;
use crate::crypto::keys::KeyStore;
use crate::error::{Error, Result};
use crate::model::enums::key_flag;
use crate::session::{Paths, SecretStore};
use crate::transport::HttpClient;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

const SENDER_KEY_CACHE_CAPACITY: usize = 256;
/// How long a sender's keys are reused before Proton is asked again, so a key
/// marked compromised during a long session stops verifying.
const SENDER_KEY_CACHE_TTL: Duration = Duration::from_secs(10 * 60);
#[derive(Default)]
struct SenderKeyCache {
    /// Each sender's keys and when they were fetched.
    entries: HashMap<String, (Instant, Vec<String>)>,
    recency: VecDeque<String>,
}

impl SenderKeyCache {
    fn get(&mut self, key: &str, now: Instant) -> Option<Vec<String>> {
        let (fetched, keys) = self.entries.get(key)?;
        let expired = now.saturating_duration_since(*fetched) >= SENDER_KEY_CACHE_TTL;
        let value = (!expired).then(|| keys.clone());
        if let Some(index) = self.recency.iter().position(|cached| cached == key) {
            self.recency.remove(index);
        }
        if expired {
            self.entries.remove(key);
        } else {
            self.recency.push_back(key.to_owned());
        }
        value
    }

    fn insert(&mut self, key: String, value: Vec<String>, now: Instant) {
        // A missing key may appear later; do not retain negative lookups.
        if value.is_empty() {
            return;
        }
        if let Some(index) = self.recency.iter().position(|cached| cached == &key) {
            self.recency.remove(index);
        } else if self.entries.len() == SENDER_KEY_CACHE_CAPACITY
            && let Some(oldest) = self.recency.pop_front()
        {
            self.entries.remove(&oldest);
        }
        self.recency.push_back(key.clone());
        self.entries.insert(key, (now, value));
    }
}

/// A signed-in Proton Mail account, with its unlocked keys, through which
/// every mail operation goes.
pub struct Client {
    http: HttpClient,
    keys: KeyStore,
    paths: Paths,
    profile: String,
    session_uid: String,
    cache_identity: CacheIdentity,
    store: Arc<dyn SecretStore>,
    sender_cache: Mutex<SenderKeyCache>,
}

impl Client {
    pub(crate) fn http(&self) -> &HttpClient {
        &self.http
    }
    pub(crate) fn keys(&self) -> &KeyStore {
        &self.keys
    }

    /// The email of the account's primary address, which new mail is sent
    /// from unless another is chosen.
    pub fn primary_email(&self) -> Option<&str> {
        self.keys.primary_address().map(|a| a.email.as_str())
    }

    /// The ID of the Proton account this client is signed in to. It stays the
    /// same from one session to the next, so it tells a later sign-in to the
    /// same account from one to another.
    pub fn user_id(&self) -> &str {
        self.cache_identity.user_id()
    }

    /// The armored public keys that check `email`'s signatures, kept for a
    /// while once fetched. Keys Proton marks as compromised are left out.
    pub(crate) async fn sender_pubkeys(&self, email: &str) -> Result<Vec<String>> {
        let key = email.to_ascii_lowercase();
        if let Some(keys) = self.sender_cache.lock().await.get(&key, Instant::now()) {
            return Ok(keys);
        }
        let pubs: Vec<String> = api::keys::get_all_public_keys(&self.http, email)
            .await?
            .keys
            .into_iter()
            .filter(|k| key_flag::can_verify(k.flags))
            .map(|k| k.public_key)
            .collect();
        self.sender_cache
            .lock()
            .await
            .insert(key, pubs.clone(), Instant::now());
        Ok(pubs)
    }
}

/// Runs blocking work, such as decrypting or reaching the credential store,
/// on Tokio's blocking pool so it holds no async worker; without a runtime it
/// runs where it is.
pub(crate) async fn off_thread<T: Send + 'static>(
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    if tokio::runtime::Handle::try_current().is_ok() {
        tokio::task::spawn_blocking(work)
            .await
            .map_err(|error| Error::Other(format!("blocking worker failed: {error}")))?
    } else {
        work()
    }
}

#[cfg(test)]
mod tests;
