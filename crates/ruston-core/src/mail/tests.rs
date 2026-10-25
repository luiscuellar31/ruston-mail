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
    async fn do_raw(&self, _req: crate::transport::Request) -> Result<crate::transport::Response> {
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
async fn a_client_names_the_account_it_is_signed_in_to() {
    let (client, base) =
        saved_logout_client("http://127.0.0.1:9", Arc::new(MemoryStore::new())).await;
    assert_eq!(client.user_id(), "logout-account");
    std::fs::remove_dir_all(base).unwrap();
}

/// A credential store that answers late, as one waiting for the person
/// to unlock it does.
struct SlowStore {
    inner: Arc<dyn SecretStore>,
    delay: Duration,
}

impl SecretStore for SlowStore {
    fn set(&self, key: &str, value: &str) -> Result<()> {
        std::thread::sleep(self.delay);
        self.inner.set(key, value)
    }

    fn get(&self, key: &str) -> Result<Option<String>> {
        std::thread::sleep(self.delay);
        self.inner.get(key)
    }

    fn delete(&self, key: &str) -> Result<()> {
        self.inner.delete(key)
    }
}

#[tokio::test]
async fn a_saved_session_resumes_with_its_keys_unlocked() {
    use proton_crypto::crypto::{
        DataEncoding, KeyGenerator, KeyGeneratorAlgorithm, KeyGeneratorSync, PGPProviderSync,
    };

    // Keys locked with the session's key passphrase, as Proton keeps them.
    let provider = crypto::provider();
    let key = |id: &str, email: &str| {
        let generated = provider
            .new_key_generator()
            .with_user_id("Test", email)
            .with_algorithm(KeyGeneratorAlgorithm::ECC)
            .generate()
            .unwrap();
        let armored = provider
            .private_key_export(&generated, "logout-skp", DataEncoding::Armor)
            .unwrap();
        serde_json::json!({
            "ID": id,
            "PrivateKey": String::from_utf8(armored.as_ref().to_vec()).unwrap(),
            "Primary": 1,
        })
    };
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/core/v4/users"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "Code": 1000,
            "User": {"ID": "user", "Keys": [key("user-key", "user@proton.me")]},
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/core/v4/addresses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "Code": 1000,
            "Addresses": [{
                "ID": "address",
                "Email": "me@proton.me",
                "Order": 1,
                "Keys": [key("address-key", "me@proton.me")],
            }],
        })))
        .mount(&server)
        .await;
    let store: Arc<dyn SecretStore> = Arc::new(MemoryStore::new());
    let (saved, base) = saved_logout_client(&server.uri(), store.clone()).await;

    let resumed = Client::resume_from("test", saved.paths.clone(), store)
        .await
        .unwrap();

    let address = &resumed.keys().addresses[0];
    assert_eq!(address.email, "me@proton.me");
    assert!(address.keys[0].unlocked.is_some(), "its key opened");
    let _ = std::fs::remove_dir_all(base);
}

#[tokio::test(flavor = "current_thread")]
async fn a_slow_credential_store_leaves_the_async_runtime_free() {
    let store: Arc<dyn SecretStore> = Arc::new(MemoryStore::new());
    let (client, base) = saved_logout_client("http://127.0.0.1:9", store.clone()).await;
    let slow = Arc::new(SlowStore {
        inner: store,
        delay: Duration::from_millis(400),
    });

    // On one thread, a store read inline would hold the timer as well.
    let started = Instant::now();
    let resumed = tokio::time::timeout(
        Duration::from_millis(50),
        Client::resume_from("test", client.paths.clone(), slow),
    )
    .await;
    assert!(resumed.is_err(), "the resume should still be waiting");
    assert!(
        started.elapsed() < Duration::from_millis(300),
        "the timer waited {:?}",
        started.elapsed()
    );

    // The read goes on in the background; the files go once it is done.
    let deadline = Instant::now() + Duration::from_secs(10);
    while std::fs::remove_dir_all(&base).is_err() {
        assert!(Instant::now() < deadline, "the session files stayed in use");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
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
                ResponseTemplate::new(status).set_body_json(serde_json::json!({"Code": status})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let (client, base) = saved_logout_client(&server.uri(), Arc::new(MemoryStore::new())).await;
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
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({"Code": 401})))
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
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"Code": 1000})))
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
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"Code": 1000})))
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
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"Code": 1000})))
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
            self.release
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .expect("the test never let the delete go on");
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
