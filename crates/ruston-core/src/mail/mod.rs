//! High-level mail client assembling transport + crypto + API.

pub mod attachments;
pub mod contacts;
pub mod drafts;
pub mod export;
pub mod filters;
pub mod organize;
pub mod read;
pub mod send;
pub mod sync;
#[cfg(test)]
mod test_support;

use crate::DEFAULT_BASE_URL;
use crate::api;
use crate::auth::{self, TotpPrompt};
use crate::cache::CacheIdentity;
use crate::crypto::{self, keys::KeyStore};
use crate::error::{Error, Result};
use crate::model::enums::{key_flag, password_mode};
use crate::session::{KeyringStore, Paths, SecretStore, Session, Tokens, validate_profile_name};
use crate::transport::{Doer, HttpClient};
use secrecy::{ExposeSecret, SecretString};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

const DEFAULT_APP_VERSION: &str = "Other";
const SENDER_KEY_CACHE_CAPACITY: usize = 256;
/// How long a sender's keys are reused before Proton is asked again, so a key
/// marked compromised during a long session stops verifying.
const SENDER_KEY_CACHE_TTL: Duration = Duration::from_secs(10 * 60);
/// Bound the complete best-effort revocation, including retries and backoff.
const LOGOUT_REVOCATION_TIMEOUT: Duration = Duration::from_secs(30);

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

/// Options for an interactive login.
pub struct LoginOptions {
    /// Account username (email address).
    pub username: String,
    /// Account password.
    pub password: SecretString,
    /// Optional TOTP code for two-factor authentication.
    pub totp: Option<SecretString>,
    /// Separate mailbox password for two-password (PasswordMode 2) accounts.
    pub mailbox_password: Option<SecretString>,
    /// Local profile name used to store the session.
    pub profile: String,
    /// Optional API base URL override.
    pub base_url: Option<String>,
    /// Optional app-version string to present to the API.
    pub app_version: Option<String>,
    /// Optional `User-Agent` to present as a real client.
    pub user_agent: Option<String>,
    /// Optional human-verification (CAPTCHA) resolver, invoked on API code 9001.
    pub hv: Option<crate::transport::HvResolver>,
}

/// Asks for the separate mailbox password of a two-password account during
/// login.
///
/// Called only when the account has one and none was supplied up front,
/// after the account password and any TOTP code are accepted, so neither is
/// asked for again. Obtain it interactively and do not persist it.
pub type MailboxPasswordPrompt =
    Arc<dyn Fn() -> futures::future::BoxFuture<'static, Result<SecretString>> + Send + Sync>;

/// What a login may ask the person signing in for, when the account needs
/// it and [`LoginOptions`] does not carry it.
#[derive(Clone, Default)]
pub struct LoginPrompts {
    /// Asks for the TOTP code; see [`TotpPrompt`].
    pub totp: Option<TotpPrompt>,
    /// Asks for the mailbox password; see [`MailboxPasswordPrompt`].
    pub mailbox_password: Option<MailboxPasswordPrompt>,
}

/// The mailbox password of an account that has a separate one: the one
/// `given`, or else what `prompt` asks for now, kept in `asked`. Without
/// either, [`Error::MailboxPasswordRequired`].
async fn mailbox_password<'a>(
    given: Option<&'a SecretString>,
    prompt: Option<&MailboxPasswordPrompt>,
    asked: &'a mut Option<SecretString>,
) -> Result<&'a SecretString> {
    if let Some(password) = given {
        return Ok(password);
    }
    let Some(prompt) = prompt else {
        return Err(Error::MailboxPasswordRequired);
    };
    Ok(asked.insert(prompt().await?))
}

/// The Proton Mail client.
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

    /// Shares the saved session with other processes: a refresh starts from
    /// the tokens saved last, under the profile lock, and saves the new ones.
    pub(crate) fn wire_refresh(
        http: &mut HttpClient,
        store: Arc<dyn SecretStore>,
        paths: Paths,
        profile: &str,
    ) {
        let load_store = store.clone();
        let load_profile = profile.to_owned();
        http.set_refresh_load(Arc::new(move || {
            Session::lock_and_load_tokens(&paths, &load_profile, load_store.as_ref())
        }));
        http.set_refresh_persist_fallible(Arc::new(move |tokens| {
            Session::save_tokens(
                store.as_ref(),
                tokens.access.expose_secret(),
                tokens.refresh.expose_secret(),
            )
        }));
    }

    /// Interactive login: SRP + 2FA, unlock keys, persist session.
    pub async fn login(opts: LoginOptions) -> Result<Client> {
        Self::login_inner(opts, LoginPrompts::default()).await
    }

    /// Like [`Client::login`], but calls `totp_prompt` for the code when the
    /// account requires TOTP and `opts.totp` is `None`. The prompt runs after
    /// any human verification (so the code is fresh) and never runs for
    /// accounts without TOTP.
    pub async fn login_with_totp_prompt(
        opts: LoginOptions,
        totp_prompt: TotpPrompt,
    ) -> Result<Client> {
        let prompts = LoginPrompts {
            totp: Some(totp_prompt),
            ..LoginPrompts::default()
        };
        Self::login_inner(opts, prompts).await
    }

    /// Like [`Client::login`], asking through `prompts` for what the account
    /// needs and `opts` does not carry: a TOTP code, as
    /// [`Client::login_with_totp_prompt`] does, and a separate mailbox
    /// password once the account password is accepted.
    pub async fn login_with_prompts(opts: LoginOptions, prompts: LoginPrompts) -> Result<Client> {
        Self::login_inner(opts, prompts).await
    }

    async fn login_inner(opts: LoginOptions, prompts: LoginPrompts) -> Result<Client> {
        validate_profile_name(&opts.profile)?;
        let base_url = opts
            .base_url
            .clone()
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        let app_version = opts
            .app_version
            .clone()
            .unwrap_or_else(|| DEFAULT_APP_VERSION.to_string());
        let store: Arc<dyn SecretStore> = Arc::new(KeyringStore::new(opts.profile.clone()));
        let mut http = HttpClient::try_new(base_url.clone(), app_version.clone())?;
        if let Some(ua) = &opts.user_agent {
            http.set_user_agent(ua.clone()).await;
        }
        if let Some(resolver) = &opts.hv {
            http.set_hv_resolver(resolver.clone());
        }

        let login = auth::login_with_prompt(
            &http,
            &opts.username,
            &opts.password,
            opts.totp.as_ref().map(ExposeSecret::expose_secret),
            prompts.totp.as_ref(),
        )
        .await?;
        let opened = Self::open_logged_in(
            &http,
            &login,
            &opts,
            &prompts,
            &base_url,
            app_version,
            store.clone(),
        );
        let (keys, paths, cache_identity) = auth::or_end_session(&http, opened).await?;

        Self::wire_refresh(&mut http, store.clone(), paths.clone(), &opts.profile);
        Ok(Client {
            http,
            keys,
            paths,
            profile: opts.profile,
            session_uid: login.tokens.uid,
            cache_identity,
            store,
            sender_cache: Mutex::new(SenderKeyCache::default()),
        })
    }

    /// The rest of a login once Proton has accepted the password: unlocks the
    /// account's keys and saves the session.
    async fn open_logged_in(
        http: &HttpClient,
        login: &auth::LoginResult,
        opts: &LoginOptions,
        prompts: &LoginPrompts,
        base_url: &str,
        app_version: String,
        store: Arc<dyn SecretStore>,
    ) -> Result<(KeyStore, Paths, CacheIdentity)> {
        let (salts, (user, addresses)) =
            tokio::try_join!(api::keys::get_key_salts(http), account(http))?;
        let cache_identity = CacheIdentity::new(base_url, &user.id)?;
        let user_id = user.id.clone();

        let mut asked = None;
        let mailbox_pw = if login.password_mode == password_mode::TWO_PASSWORDS {
            mailbox_password(
                opts.mailbox_password.as_ref(),
                prompts.mailbox_password.as_ref(),
                &mut asked,
            )
            .await?
        } else {
            &opts.password
        };

        tracing::debug!(target: "ruston_core::mail", addresses = addresses.len(), password_mode = login.password_mode, "login: fetched salts + user + addresses; unlocking keys");
        // Each key's passphrase is derived with bcrypt, slow by design, so the
        // unlocking runs where it holds no async worker; resuming does too.
        let mailbox_pw = mailbox_pw.clone();
        let (keys, skp) = off_thread(move || {
            crypto::keys::unlock(&crypto::provider(), &user, &addresses, &mailbox_pw, &salts)
        })
        .await?;

        let session = Session {
            uid: login.tokens.uid.clone(),
            app_version,
            base_url: base_url.to_owned(),
            password_mode: login.password_mode,
            user_agent: opts.user_agent.clone(),
            user_id: Some(user_id),
        };
        let paths = Paths::system()?;
        let (saved_in, profile, tokens) =
            (paths.clone(), opts.profile.clone(), login.tokens.clone());
        off_thread(move || session.save(&saved_in, &profile, store.as_ref(), &tokens, &skp))
            .await?;
        Ok((keys, paths, cache_identity))
    }

    /// Resume a saved session and unlock keys using the stored `skp`.
    pub async fn resume(profile: &str) -> Result<Client> {
        validate_profile_name(profile)?;
        let store: Arc<dyn SecretStore> = Arc::new(KeyringStore::new(profile.to_string()));
        Self::resume_from(profile, Paths::system()?, store).await
    }

    /// `resume`, with the session's files and secrets where the caller says.
    async fn resume_from(
        profile: &str,
        paths: Paths,
        store: Arc<dyn SecretStore>,
    ) -> Result<Client> {
        tracing::info!(target: "ruston_core::mail", profile, "resume: loading saved session");
        // The credential store may wait for the person to unlock it, and the
        // profile lock for another process.
        let loaded = {
            let (paths, profile, store) = (paths.clone(), profile.to_owned(), store.clone());
            off_thread(move || Session::load(&paths, &profile, store.as_ref())).await?
        }
        .ok_or(Error::Unauthorized)?;
        tracing::debug!(target: "ruston_core::mail", uid = %loaded.session.uid, "resume: session found; fetching user + addresses, unlocking keys");

        let mut http = HttpClient::try_new(
            loaded.session.base_url.clone(),
            loaded.session.app_version.clone(),
        )?;
        if let Some(ua) = &loaded.session.user_agent {
            http.set_user_agent(ua.clone()).await;
        }
        let Tokens {
            uid,
            access,
            refresh,
        } = loaded.tokens;
        http.set_tokens(uid, access, refresh).await;
        Self::wire_refresh(&mut http, store.clone(), paths.clone(), profile);

        let (user, addresses) = account(&http).await?;
        let cache_identity = CacheIdentity::new(&loaded.session.base_url, &user.id)?;
        let user_id = user.id.clone();
        let skp = loaded.skp;
        let keys = off_thread(move || {
            crypto::keys::unlock_with_skp(&crypto::provider(), &user, &addresses, &skp)
        })
        .await?;
        if loaded.session.user_id.as_deref() != Some(user_id.as_str()) {
            // A session saved without its account ID records it now, so its
            // cache opens offline next time. Nothing else depends on it.
            let (paths, profile) = (paths.clone(), profile.to_owned());
            let uid = loaded.session.uid.clone();
            let recorded =
                off_thread(move || Session::record_user_id(&paths, &profile, &uid, &user_id)).await;
            if let Err(error) = recorded {
                tracing::debug!(target: "ruston_core::mail", %error, "resume: account ID not recorded");
            }
        }

        Ok(Client {
            http,
            keys,
            paths,
            profile: profile.to_string(),
            session_uid: loaded.session.uid,
            cache_identity,
            store,
            sender_cache: Mutex::new(SenderKeyCache::default()),
        })
    }

    /// Clear the saved session, then attempt remote revocation for up to 30 seconds.
    /// Local cleanup runs on a blocking worker and continues if this future is
    /// cancelled after it starts. Remote failures do not undo cleanup; local
    /// storage failures are returned to the caller. Returns whether Proton
    /// confirmed that the session ended.
    pub async fn logout(&self) -> Result<bool> {
        let paths = self.paths.clone();
        let profile = self.profile.clone();
        let session_uid = self.session_uid.clone();
        let store = self.store.clone();
        let cleanup = tokio::task::spawn_blocking(move || {
            Session::clear_if_current(&paths, &profile, store.as_ref(), &session_uid)
        })
        .await
        .map_err(|error| Error::Session(format!("session cleanup task failed: {error}")))?;

        // Only revocation has a deadline. A timeout must never cancel local cleanup.
        let revoked = matches!(
            tokio::time::timeout(LOGOUT_REVOCATION_TIMEOUT, auth::logout(&self.http)).await,
            Ok(Ok(()))
        );
        cleanup.map(|()| revoked)
    }

    /// Remove a profile's saved session without resuming it, for when it can
    /// no longer be resumed: offline, revoked, after a password change, or
    /// bound to another API address. Proton is not contacted, so a server
    /// session that still exists is not revoked.
    pub async fn forget_session(profile: &str) -> Result<()> {
        validate_profile_name(profile)?;
        let profile = profile.to_owned();
        tokio::task::spawn_blocking(move || {
            let store = KeyringStore::new(profile.clone());
            Session::clear(&Paths::system()?, &profile, &store)
        })
        .await
        .map_err(|error| Error::Session(format!("session cleanup task failed: {error}")))?
    }

    /// The primary sending address email.
    pub fn primary_email(&self) -> Option<&str> {
        self.keys.primary_address().map(|a| a.email.as_str())
    }

    /// The ID of the Proton account this client is signed in to. It stays the
    /// same from one session to the next, so it tells a later sign-in to the
    /// same account from one to another.
    pub fn user_id(&self) -> &str {
        self.cache_identity.user_id()
    }

    /// Fetch (and cache for a while) a sender's armored public keys for
    /// verification, leaving out keys Proton marks as compromised.
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

/// The account and its addresses, asked for together: neither request needs
/// the other, and every command that resumes a session waits for both.
async fn account<D: Doer>(http: &D) -> Result<(crypto::keys::User, Vec<crypto::keys::Address>)> {
    tokio::try_join!(api::keys::get_user(http), api::keys::get_addresses(http))
}

#[cfg(test)]
mod tests;
