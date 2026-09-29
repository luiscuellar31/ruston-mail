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

use crate::api;
use crate::auth::{self, TotpPrompt};
use crate::cache::CacheIdentity;
use crate::crypto::{self, keys::KeyStore};
use crate::error::{Error, Result};
use crate::session::{KeyringStore, Paths, SecretStore, Session, Tokens, validate_profile_name};
use crate::transport::HttpClient;
use secrecy::{ExposeSecret, SecretString};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

const DEFAULT_BASE_URL: &str = "https://mail.proton.me/api";
const DEFAULT_APP_VERSION: &str = "Other";
const SENDER_KEY_CACHE_CAPACITY: usize = 256;
/// Bound the complete best-effort revocation, including retries and backoff.
const LOGOUT_REVOCATION_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Default)]
struct SenderKeyCache {
    entries: HashMap<String, Vec<String>>,
    recency: VecDeque<String>,
}

impl SenderKeyCache {
    fn get(&mut self, key: &str) -> Option<Vec<String>> {
        let value = self.entries.get(key)?.clone();
        if let Some(index) = self.recency.iter().position(|cached| cached == key) {
            self.recency.remove(index);
        }
        self.recency.push_back(key.to_owned());
        Some(value)
    }

    fn insert(&mut self, key: String, value: Vec<String>) {
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
        self.entries.insert(key, value);
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

    fn wire_refresh(
        http: &mut HttpClient,
        store: Arc<dyn SecretStore>,
        paths: Paths,
        profile: &str,
    ) {
        let load_store = store.clone();
        let load_profile = profile.to_owned();
        http.set_refresh_load(Arc::new(move || {
            let (lock, loaded) =
                Session::lock_and_load(&paths, &load_profile, load_store.as_ref())?;
            Ok((lock, loaded.map(|session| session.tokens)))
        }));
        http.set_refresh_persist_fallible(Arc::new(move |_uid, access, refresh| {
            Session::save_tokens(store.as_ref(), access, refresh)
        }));
    }

    /// Interactive login: SRP + 2FA, unlock keys, persist session.
    pub async fn login(opts: LoginOptions) -> Result<Client> {
        Self::login_inner(opts, None).await
    }

    /// Like [`Client::login`], but calls `totp_prompt` for the code when the
    /// account requires TOTP and `opts.totp` is `None`. The prompt runs after
    /// any human verification (so the code is fresh) and never runs for
    /// accounts without TOTP.
    pub async fn login_with_totp_prompt(
        opts: LoginOptions,
        totp_prompt: TotpPrompt,
    ) -> Result<Client> {
        Self::login_inner(opts, Some(totp_prompt)).await
    }

    async fn login_inner(opts: LoginOptions, totp_prompt: Option<TotpPrompt>) -> Result<Client> {
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
        let mut http = HttpClient::new(base_url.clone(), app_version.clone());
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
            totp_prompt.as_ref(),
        )
        .await?;

        let provider = crypto::provider();
        let salts = api::keys::get_key_salts(&http).await?;
        let user = api::keys::get_user(&http).await?;
        let cache_identity = CacheIdentity::new(&base_url, &user.id)?;
        let addresses = api::keys::get_addresses(&http).await?;

        let mailbox_pw = if login.password_mode == 2 {
            opts.mailbox_password.as_ref().ok_or_else(|| {
                Error::Other(
                    "this account uses a separate mailbox password (PasswordMode 2); supply --mailbox-password".into(),
                )
            })?
        } else {
            &opts.password
        };

        tracing::debug!(target: "ruston_core::mail", addresses = addresses.len(), password_mode = login.password_mode, "login: fetched salts + user + addresses; unlocking keys");
        let (keys, skp) = crypto::keys::unlock(&provider, &user, &addresses, mailbox_pw, &salts)?;

        let session = Session {
            uid: login.tokens.uid.clone(),
            app_version,
            base_url: base_url.clone(),
            password_mode: login.password_mode,
            user_agent: opts.user_agent.clone(),
        };
        let paths = Paths::system()?;
        session.save(&paths, &opts.profile, store.as_ref(), &login.tokens, &skp)?;

        Self::wire_refresh(&mut http, store.clone(), paths.clone(), &opts.profile);
        Ok(Client {
            http,
            keys,
            paths,
            profile: opts.profile,
            session_uid: session.uid,
            cache_identity,
            store,
            sender_cache: Mutex::new(SenderKeyCache::default()),
        })
    }

    /// Resume a saved session and unlock keys using the stored `skp`.
    pub async fn resume(profile: &str) -> Result<Client> {
        validate_profile_name(profile)?;
        tracing::info!(target: "ruston_core::mail", profile, "resume: loading saved session");
        let store: Arc<dyn SecretStore> = Arc::new(KeyringStore::new(profile.to_string()));
        let paths = Paths::system()?;
        let loaded = Session::load(&paths, profile, store.as_ref())?.ok_or(Error::Unauthorized)?;
        tracing::debug!(target: "ruston_core::mail", uid = %loaded.session.uid, "resume: session found; fetching user + addresses, unlocking keys");

        let mut http = HttpClient::new(
            loaded.session.base_url.clone(),
            loaded.session.app_version.clone(),
        );
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
        let user = api::keys::get_user(&http).await?;
        let cache_identity = CacheIdentity::new(&loaded.session.base_url, &user.id)?;
        let addresses = api::keys::get_addresses(&http).await?;
        let keys = crypto::keys::unlock_with_skp(&provider, &user, &addresses, &loaded.skp)?;

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
    /// storage failures are returned to the caller.
    pub async fn logout(&self) -> Result<()> {
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
        let _ = tokio::time::timeout(LOGOUT_REVOCATION_TIMEOUT, auth::logout(&self.http)).await;
        cleanup
    }

    /// The primary sending address email.
    pub fn primary_email(&self) -> Option<&str> {
        self.keys.primary_address().map(|a| a.email.as_str())
    }

    /// Fetch (and cache) a sender's armored public keys for verification.
    pub(crate) async fn sender_pubkeys(&self, email: &str) -> Vec<String> {
        let key = email.to_ascii_lowercase();
        if let Some(keys) = self.sender_cache.lock().await.get(&key) {
            return keys;
        }
        let pubs = match api::keys::get_all_public_keys(&self.http, email).await {
            Ok(r) => r.keys.into_iter().map(|k| k.public_key).collect(),
            Err(_) => Vec::new(),
        };
        self.sender_cache.lock().await.insert(key, pubs.clone());
        pubs
    }
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

    async fn saved_logout_client(base_url: &str, store: Arc<dyn SecretStore>) -> (Client, PathBuf) {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base =
            std::env::temp_dir().join(format!("ruston-logout-{}-{unique}", std::process::id()));
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
            std::task::Poll::Ready(Ok(()))
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
            client.logout().await.unwrap();
            assert_session_cleared(&client, &base);
            server.verify().await;
            std::fs::remove_dir_all(base).unwrap();
        }
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

        client.logout().await.unwrap();
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
            if key == "auth_tokens_v1" {
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
            if key == "auth_tokens_v1" {
                self.entered.notify_one();
                self.release.lock().unwrap().recv().unwrap();
            }
            self.inner.delete(key)?;
            if key == "skp" {
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
        let mut cache = SenderKeyCache::default();
        cache.insert("missing@example.com".into(), Vec::new());
        assert!(cache.get("missing@example.com").is_none());

        for index in 0..SENDER_KEY_CACHE_CAPACITY {
            cache.insert(
                format!("sender-{index}@example.com"),
                vec![index.to_string()],
            );
        }
        assert_eq!(cache.entries.len(), SENDER_KEY_CACHE_CAPACITY);

        assert!(cache.get("sender-0@example.com").is_some());
        cache.insert("new@example.com".into(), vec!["key".into()]);

        assert_eq!(cache.entries.len(), SENDER_KEY_CACHE_CAPACITY);
        assert!(cache.get("sender-0@example.com").is_some());
        assert!(cache.get("sender-1@example.com").is_none());
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
        drop(Client::login_with_totp_prompt(opts(), prompt));
    }
}
