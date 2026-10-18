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

use crate::DEFAULT_BASE_URL;
use crate::api;
use crate::auth::{self, TotpPrompt};
use crate::cache::CacheIdentity;
use crate::crypto::{self, keys::KeyStore};
use crate::error::{Error, Result};
use crate::model::enums::key_flag;
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
/// either, the error says how to give it, in words the desktop recognizes.
async fn mailbox_password<'a>(
    given: Option<&'a SecretString>,
    prompt: Option<&MailboxPasswordPrompt>,
    asked: &'a mut Option<SecretString>,
) -> Result<&'a SecretString> {
    if let Some(password) = given {
        return Ok(password);
    }
    let Some(prompt) = prompt else {
        return Err(Error::Other(
            "this account uses a separate mailbox password (PasswordMode 2); supply --mailbox-password".into(),
        ));
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
        http.set_refresh_persist_fallible(Arc::new(move |_uid, access, refresh| {
            Session::save_tokens(store.as_ref(), access, refresh)
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
            store.as_ref(),
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
        store: &dyn SecretStore,
    ) -> Result<(KeyStore, Paths, CacheIdentity)> {
        let provider = crypto::provider();
        let (salts, (user, addresses)) =
            tokio::try_join!(api::keys::get_key_salts(http), account(http))?;
        let cache_identity = CacheIdentity::new(base_url, &user.id)?;

        let mut asked = None;
        let mailbox_pw = if login.password_mode == 2 {
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
        let (keys, skp) = crypto::keys::unlock(&provider, &user, &addresses, mailbox_pw, &salts)?;

        let session = Session {
            uid: login.tokens.uid.clone(),
            app_version,
            base_url: base_url.to_owned(),
            password_mode: login.password_mode,
            user_agent: opts.user_agent.clone(),
            user_id: Some(user.id.clone()),
        };
        let paths = Paths::system()?;
        session.save(&paths, &opts.profile, store, &login.tokens, &skp)?;
        Ok((keys, paths, cache_identity))
    }

    /// Resume a saved session and unlock keys using the stored `skp`.
    pub async fn resume(profile: &str) -> Result<Client> {
        validate_profile_name(profile)?;
        tracing::info!(target: "ruston_core::mail", profile, "resume: loading saved session");
        let store: Arc<dyn SecretStore> = Arc::new(KeyringStore::new(profile.to_string()));
        let paths = Paths::system()?;
        let loaded = Session::load(&paths, profile, store.as_ref())?.ok_or(Error::Unauthorized)?;
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

        let provider = crypto::provider();
        let (user, addresses) = account(&http).await?;
        let cache_identity = CacheIdentity::new(&loaded.session.base_url, &user.id)?;
        let keys = crypto::keys::unlock_with_skp(&provider, &user, &addresses, &loaded.skp)?;
        if loaded.session.user_id.as_deref() != Some(user.id.as_str()) {
            // A session saved without its account ID records it now, so its
            // cache opens offline next time. Nothing else depends on it.
            if let Err(error) =
                Session::record_user_id(&paths, profile, &loaded.session.uid, &user.id)
            {
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

/// Runs CPU work, such as decrypting, on Tokio's blocking pool so it holds
/// no async worker; without a runtime it runs where it is.
pub(crate) async fn off_thread<T: Send + 'static>(
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    if tokio::runtime::Handle::try_current().is_ok() {
        tokio::task::spawn_blocking(work)
            .await
            .map_err(|error| Error::Other(format!("decryption worker failed: {error}")))?
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
mod tests {
    use super::*;
    use crate::session::MemoryStore;
    use std::path::PathBuf;
    use std::time::Duration;
    use tokio::sync::Notify;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Answers only once both account requests are waiting for an answer.
    struct BothAtOnce(tokio::sync::Barrier);

    #[async_trait::async_trait]
    impl Doer for BothAtOnce {
        async fn do_raw(
            &self,
            _req: crate::transport::Request,
        ) -> Result<crate::transport::Response> {
            unreachable!("the account requests decode JSON")
        }

        async fn decode<T: serde::de::DeserializeOwned>(
            &self,
            req: crate::transport::Request,
        ) -> Result<T> {
            self.0.wait().await;
            Ok(serde_json::from_value(match req.path.as_str() {
                "/core/v4/users" => serde_json::json!({"User": {"ID": "user-1", "Keys": []}}),
                "/core/v4/addresses" => serde_json::json!({"Addresses": []}),
                other => panic!("unexpected request to {other}"),
            })?)
        }
    }

    #[tokio::test]
    async fn the_account_and_its_addresses_are_asked_for_at_once() {
        let http = BothAtOnce(tokio::sync::Barrier::new(2));
        // Asked one after the other, the first request would wait forever.
        let (user, addresses) = tokio::time::timeout(Duration::from_secs(10), account(&http))
            .await
            .expect("the requests did not overlap")
            .unwrap();
        assert_eq!(user.id, "user-1");
        assert!(addresses.is_empty());
    }

    async fn saved_logout_client(base_url: &str, store: Arc<dyn SecretStore>) -> (Client, PathBuf) {
        // Tests run in parallel and the clock can read the same for two of
        // them, so a counter keeps each directory to itself.
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!(
            "ruston-logout-{}-{unique}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let paths = Paths::with_base(&base);
        let tokens = Tokens {
            uid: "logout-uid".into(),
            access: SecretString::from("logout-access"),
            refresh: SecretString::from("logout-refresh"),
        };
        Session {
            uid: tokens.uid.clone(),
            app_version: "Other".into(),
            base_url: base_url.into(),
            password_mode: 1,
            user_agent: None,
            user_id: None,
        }
        .save(
            &paths,
            "test",
            store.as_ref(),
            &tokens,
            &SecretString::from("logout-skp"),
        )
        .unwrap();
        let mut http = HttpClient::new(base_url, "Other");
        http.set_tokens(tokens.uid, tokens.access, tokens.refresh)
            .await;
        Client::wire_refresh(&mut http, store.clone(), paths.clone(), "test");
        (
            Client {
                http,
                keys: KeyStore {
                    user_keys: Vec::new(),
                    addresses: Vec::new(),
                },
                paths,
                profile: "test".into(),
                session_uid: "logout-uid".into(),
                cache_identity: CacheIdentity::new(base_url, "logout-account").unwrap(),
                store,
                sender_cache: Mutex::new(SenderKeyCache::default()),
            },
            base,
        )
    }

    fn assert_session_cleared(client: &Client, base: &std::path::Path) {
        assert!(!base.join("sessions/test.json").exists());
        assert!(client.store.get("auth_tokens_v1").unwrap().is_none());
        assert!(client.store.get("skp").unwrap().is_none());
        assert!(
            Session::load(&client.paths, "test", client.store.as_ref())
                .unwrap()
                .is_none()
        );
    }

    async fn delayed_logout_server() -> (MockServer, Arc<Notify>) {
        let server = MockServer::start().await;
        let requested = Arc::new(Notify::new());
        let received = requested.clone();
        Mock::given(method("DELETE"))
            .and(path("/core/v4/auth"))
            .and(header("authorization", "Bearer logout-access"))
            .respond_with(move |_: &wiremock::Request| {
                received.notify_one();
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"Code": 1000}))
                    .set_delay(Duration::from_secs(3600))
            })
            .expect(1)
            .mount(&server)
            .await;
        (server, requested)
    }

    #[tokio::test]
    async fn cancelling_remote_logout_cannot_leave_saved_credentials() {
        let (server, requested) = delayed_logout_server().await;
        let (client, base) = saved_logout_client(&server.uri(), Arc::new(MemoryStore::new())).await;

        {
            let logout = client.logout();
            tokio::pin!(logout);
            tokio::select! {
                result = &mut logout => panic!("revocation should still be pending: {result:?}"),
                _ = requested.notified() => {}
            }
            assert_session_cleared(&client, &base);
            // Dropping the future models cancellation by a caller's timeout.
        }
        assert_session_cleared(&client, &base);
        server.verify().await;
        std::fs::remove_dir_all(base).unwrap();
    }

    #[tokio::test]
    async fn remote_logout_timeout_keeps_local_cleanup_complete() {
        let (server, requested) = delayed_logout_server().await;
        let (client, base) = saved_logout_client(&server.uri(), Arc::new(MemoryStore::new())).await;
        let logout = client.logout();
        tokio::pin!(logout);
        tokio::select! {
            result = &mut logout => panic!("revocation should still be pending: {result:?}"),
            _ = requested.notified() => {}
        }
        assert_session_cleared(&client, &base);
        tokio::time::pause();
        // Tokio rounds timer deadlines up to the next millisecond.
        tokio::time::advance(LOGOUT_REVOCATION_TIMEOUT + Duration::from_millis(1)).await;
        assert!(matches!(
            futures::poll!(&mut logout),
            std::task::Poll::Ready(Ok(false))
        ));
        assert_session_cleared(&client, &base);
        server.verify().await;
        std::fs::remove_dir_all(base).unwrap();
    }

    #[tokio::test]
    async fn remote_logout_errors_do_not_leave_saved_credentials() {
        for status in [401, 500] {
            let server = MockServer::start().await;
            Mock::given(method("DELETE"))
                .and(path("/core/v4/auth"))
                .respond_with(
                    ResponseTemplate::new(status)
                        .set_body_json(serde_json::json!({"Code": status})),
                )
                .expect(1)
                .mount(&server)
                .await;
            let (client, base) =
                saved_logout_client(&server.uri(), Arc::new(MemoryStore::new())).await;
            assert!(
                !client.logout().await.unwrap(),
                "{status} was not a revocation"
            );
            assert_session_cleared(&client, &base);
            server.verify().await;
            std::fs::remove_dir_all(base).unwrap();
        }
    }

    #[tokio::test]
    async fn logout_with_an_expired_token_still_revokes_and_saves_nothing() {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path("/core/v4/auth"))
            .and(header("authorization", "Bearer logout-access"))
            .respond_with(
                ResponseTemplate::new(401).set_body_json(serde_json::json!({"Code": 401})),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/auth/v4/refresh"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Code": 1000,
                "AccessToken": "fresh-access",
                "RefreshToken": "fresh-refresh",
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/core/v4/auth"))
            .and(header("authorization", "Bearer fresh-access"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"Code": 1000})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let (client, base) = saved_logout_client(&server.uri(), Arc::new(MemoryStore::new())).await;

        assert!(
            client.logout().await.unwrap(),
            "Proton confirmed the revocation"
        );
        // The tokens refreshed to revoke the session were never saved.
        assert_session_cleared(&client, &base);
        server.verify().await;
        std::fs::remove_dir_all(base).unwrap();
    }

    #[tokio::test]
    async fn old_logout_preserves_a_replacement_session_in_the_same_profile() {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path("/core/v4/auth"))
            .and(header("authorization", "Bearer logout-access"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"Code": 1000})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let (client, base) = saved_logout_client(&server.uri(), Arc::new(MemoryStore::new())).await;
        Session {
            uid: "replacement-uid".into(),
            app_version: "Other".into(),
            base_url: server.uri(),
            password_mode: 1,
            user_agent: None,
            user_id: None,
        }
        .save(
            &client.paths,
            "test",
            client.store.as_ref(),
            &Tokens {
                uid: "replacement-uid".into(),
                access: SecretString::from("replacement-access"),
                refresh: SecretString::from("replacement-refresh"),
            },
            &SecretString::from("replacement-skp"),
        )
        .unwrap();

        assert!(client.logout().await.unwrap());
        let saved = Session::load(&client.paths, "test", client.store.as_ref())
            .unwrap()
            .unwrap();
        assert_eq!(saved.session.uid, "replacement-uid");
        assert_eq!(saved.tokens.access.expose_secret(), "replacement-access");
        assert_eq!(saved.tokens.refresh.expose_secret(), "replacement-refresh");
        assert_eq!(saved.skp.expose_secret(), "replacement-skp");
        server.verify().await;
        std::fs::remove_dir_all(base).unwrap();
    }

    struct RejectTokenDeletes(MemoryStore);

    impl SecretStore for RejectTokenDeletes {
        fn set(&self, key: &str, value: &str) -> Result<()> {
            self.0.set(key, value)
        }

        fn get(&self, key: &str) -> Result<Option<String>> {
            self.0.get(key)
        }

        fn delete(&self, key: &str) -> Result<()> {
            // Saving a session clears its entries first, while they are
            // still empty; only removing saved tokens fails.
            if key == "auth_tokens_v1" && self.0.get(key)?.is_some() {
                return Err(Error::Session("simulated keyring delete failure".into()));
            }
            self.0.delete(key)
        }
    }

    #[tokio::test]
    async fn local_logout_failure_still_removes_other_secrets_and_attempts_revocation() {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path("/core/v4/auth"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"Code": 1000})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let (client, base) = saved_logout_client(
            &server.uri(),
            Arc::new(RejectTokenDeletes(MemoryStore::new())),
        )
        .await;

        assert!(matches!(client.logout().await, Err(Error::Session(_))));
        assert!(!base.join("sessions/test.json").exists());
        assert!(client.store.get("auth_tokens_v1").unwrap().is_some());
        assert!(client.store.get("skp").unwrap().is_none());
        server.verify().await;
        std::fs::remove_dir_all(base).unwrap();
    }

    struct PausedDeletes {
        inner: MemoryStore,
        entered: Notify,
        release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
        finished: Notify,
    }

    impl SecretStore for PausedDeletes {
        fn set(&self, key: &str, value: &str) -> Result<()> {
            self.inner.set(key, value)
        }

        fn get(&self, key: &str) -> Result<Option<String>> {
            self.inner.get(key)
        }

        fn delete(&self, key: &str) -> Result<()> {
            // Saving the session clears its entries first, while they are
            // still empty; only removing what was saved pauses and counts.
            let saved = self.inner.get(key)?.is_some();
            if key == "auth_tokens_v1" && saved {
                self.entered.notify_one();
                self.release.lock().unwrap().recv().unwrap();
            }
            self.inner.delete(key)?;
            if key == "skp" && saved {
                self.finished.notify_one();
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn cancelling_logout_during_local_cleanup_does_not_stop_the_worker() {
        let server = MockServer::start().await;
        let (release, waiting) = std::sync::mpsc::channel();
        let store = Arc::new(PausedDeletes {
            inner: MemoryStore::new(),
            entered: Notify::new(),
            release: std::sync::Mutex::new(waiting),
            finished: Notify::new(),
        });
        let (client, base) = saved_logout_client(&server.uri(), store.clone()).await;
        {
            let logout = client.logout();
            tokio::pin!(logout);
            tokio::select! {
                result = &mut logout => panic!("cleanup should still be pending: {result:?}"),
                _ = store.entered.notified() => {}
            }
            // The caller is cancelled while the blocking worker is in the keyring.
        }
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), store.finished.notified())
            .await
            .unwrap();
        assert_session_cleared(&client, &base);
        assert!(server.received_requests().await.unwrap().is_empty());
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn sender_key_cache_is_lru_bounded_and_skips_negative_results() {
        let now = Instant::now();
        let mut cache = SenderKeyCache::default();
        cache.insert("missing@example.com".into(), Vec::new(), now);
        assert!(cache.get("missing@example.com", now).is_none());

        for index in 0..SENDER_KEY_CACHE_CAPACITY {
            cache.insert(
                format!("sender-{index}@example.com"),
                vec![index.to_string()],
                now,
            );
        }
        assert_eq!(cache.entries.len(), SENDER_KEY_CACHE_CAPACITY);

        assert!(cache.get("sender-0@example.com", now).is_some());
        cache.insert("new@example.com".into(), vec!["key".into()], now);

        assert_eq!(cache.entries.len(), SENDER_KEY_CACHE_CAPACITY);
        assert!(cache.get("sender-0@example.com", now).is_some());
        assert!(cache.get("sender-1@example.com", now).is_none());
    }

    #[test]
    fn cached_sender_keys_expire() {
        let fetched = Instant::now();
        let mut cache = SenderKeyCache::default();
        cache.insert("sender@example.com".into(), vec!["key".into()], fetched);

        let almost = fetched + SENDER_KEY_CACHE_TTL - Duration::from_secs(1);
        assert!(cache.get("sender@example.com", almost).is_some());
        // Later lookups ask Proton again, which may have marked the key
        // compromised meanwhile.
        let expired = fetched + SENDER_KEY_CACHE_TTL;
        assert!(cache.get("sender@example.com", expired).is_none());
        assert!(cache.entries.is_empty() && cache.recency.is_empty());
    }

    /// Both login entry points accept credentials wrapped as secrets.
    /// The futures are never polled, so nothing touches the network.
    #[test]
    fn login_entry_points_accept_secret_options() {
        let opts = || LoginOptions {
            username: "user@example.com".into(),
            password: SecretString::from("not-a-real-password"),
            totp: None,
            mailbox_password: None,
            profile: "test".into(),
            base_url: None,
            app_version: None,
            user_agent: None,
            hv: None,
        };
        let prompt: TotpPrompt = Arc::new(|| Box::pin(async { Ok(SecretString::from("000000")) }));
        drop(Client::login(opts()));
        drop(Client::login_with_totp_prompt(opts(), prompt.clone()));
        let mailbox_password: MailboxPasswordPrompt =
            Arc::new(|| Box::pin(async { Ok(SecretString::from("not-a-real-password")) }));
        let prompts = LoginPrompts {
            totp: Some(prompt),
            mailbox_password: Some(mailbox_password),
        };
        drop(Client::login_with_prompts(opts(), prompts));
    }

    #[tokio::test]
    async fn a_separate_mailbox_password_is_asked_for_only_when_not_given() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let asks = Arc::new(AtomicUsize::new(0));
        let counted = asks.clone();
        let prompt: MailboxPasswordPrompt = Arc::new(move || {
            counted.fetch_add(1, Ordering::Relaxed);
            Box::pin(async { Ok(SecretString::from("asked")) })
        });
        let given = SecretString::from("given");

        let mut asked = None;
        let used = mailbox_password(Some(&given), Some(&prompt), &mut asked)
            .await
            .unwrap();
        assert_eq!(used.expose_secret(), "given");
        assert_eq!(asks.load(Ordering::Relaxed), 0);

        let mut asked = None;
        let used = mailbox_password(None, Some(&prompt), &mut asked)
            .await
            .unwrap();
        assert_eq!(used.expose_secret(), "asked");
        assert_eq!(asks.load(Ordering::Relaxed), 1);

        // With no way to ask, the error is the one the desktop recognizes.
        let mut asked = None;
        let error = mailbox_password(None, None, &mut asked).await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("this account uses a separate mailbox password")
        );
    }
}
