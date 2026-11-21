use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Default)]
struct RejectTokenWrites {
    inner: MemoryStore,
    reject: AtomicBool,
}

impl SecretStore for RejectTokenWrites {
    fn set(&self, key: &str, value: &str) -> Result<()> {
        if self.reject.load(Ordering::Relaxed) && key == K_TOKENS {
            return Err(Error::Session("simulated keyring write failure".into()));
        }
        self.inner.set(key, value)
    }

    fn get(&self, key: &str) -> Result<Option<String>> {
        self.inner.get(key)
    }

    fn delete(&self, key: &str) -> Result<()> {
        self.inner.delete(key)
    }
}

fn unique_base() -> PathBuf {
    // Tests run in parallel and the clock can read the same for two of
    // them, so a counter keeps each directory to itself.
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "ruston-session-test-{}-{}-{}",
        std::process::id(),
        nanos,
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

#[test]
fn a_profile_held_elsewhere_is_given_up_on_after_a_while() {
    let base = unique_base();
    let paths = Paths::with_base(&base);
    let held = Session::lock(&paths, "default").unwrap();

    let started = Instant::now();
    let busy = Session::lock_within(&paths, "default", Duration::from_millis(200));
    assert!(
        matches!(busy, Err(Error::Session(ref message)) if message.contains("busy")),
        "{busy:?}"
    );
    assert!(started.elapsed() >= Duration::from_millis(200));

    drop(held);
    Session::lock_within(&paths, "default", Duration::from_millis(200)).unwrap();
    let _ = std::fs::remove_dir_all(base);
}

#[test]
fn a_session_counts_as_saved_by_its_metadata_alone() {
    let base = unique_base();
    let paths = Paths::with_base(&base);
    assert!(!Session::is_saved(&paths, "default"));
    let store = MemoryStore::new();
    Session {
        uid: "uid".into(),
        app_version: "Other".into(),
        base_url: crate::DEFAULT_BASE_URL.into(),
        password_mode: 1,
        user_agent: None,
        user_id: "user-1".into(),
    }
    .save(
        &paths,
        "default",
        &store,
        &Tokens {
            uid: "uid".into(),
            access: SecretString::from("access"),
            refresh: SecretString::from("refresh"),
        },
        &SecretString::from("skp"),
    )
    .unwrap();
    assert!(Session::is_saved(&paths, "default"));
    assert!(!Session::is_saved(&paths, "other"));
    assert!(!Session::is_saved(&paths, "../default"));
    // Its secrets gone, it still counts: resuming it says what is wrong.
    store.delete(K_TOKENS).unwrap();
    assert!(Session::is_saved(&paths, "default"));
    let _ = std::fs::remove_dir_all(base);
}

#[test]
fn waiting_for_a_profile_lasts_while_it_is_held_and_lets_it_go() {
    let base = unique_base();
    let paths = Paths::with_base(&base);
    // Never signed in to: nothing to wait for, and nothing is made.
    Session::wait_unlocked(&paths, "default").unwrap();
    assert!(!base.exists());

    let held = Session::lock(&paths, "default").unwrap();
    let waiter = {
        let paths = paths.clone();
        std::thread::spawn(move || Session::wait_unlocked(&paths, "default"))
    };
    std::thread::sleep(Duration::from_millis(200));
    assert!(!waiter.is_finished(), "stopped waiting while it was held");
    drop(held);
    waiter.join().unwrap().unwrap();
    Session::lock_within(&paths, "default", Duration::ZERO).unwrap();
    let _ = std::fs::remove_dir_all(base);
}

#[test]
fn clear_removes_secrets_even_when_metadata_cannot_be_removed() {
    let base = unique_base();
    let paths = Paths::with_base(&base);
    // A directory at the metadata path makes remove_file fail deterministically.
    std::fs::create_dir_all(paths.metadata_file("test")).unwrap();
    let store = MemoryStore::new();
    store.set(K_TOKENS, "tokens").unwrap();
    store.set(K_SKP, "skp").unwrap();

    assert!(matches!(
        Session::clear(&paths, "test", &store),
        Err(Error::Io(_))
    ));
    assert!(store.get(K_TOKENS).unwrap().is_none());
    assert!(store.get(K_SKP).unwrap().is_none());
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn invalid_profile_cannot_write_outside_sessions_directory() {
    let base = unique_base();
    std::fs::create_dir_all(&base).unwrap();
    let outside = base.join("outside.json");
    std::fs::write(&outside, b"keep this file").unwrap();
    let paths = Paths::with_base(&base);
    let store = MemoryStore::new();
    let session = metadata("UID1", crate::DEFAULT_BASE_URL);
    let saved_tokens = tokens("UID1", "acc", "ref");
    assert!(matches!(
        session.save(
            &paths,
            "../outside",
            &store,
            &saved_tokens,
            &SecretString::from("skp"),
        ),
        Err(Error::Session(_))
    ));
    assert_eq!(std::fs::read(&outside).unwrap(), b"keep this file");
    assert!(!base.join("outside.lock").exists());
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn a_saved_session_loads_whole_until_it_is_cleared() {
    let base = unique_base();
    let paths = Paths::with_base(&base);
    let store = MemoryStore::new();
    let session = Session {
        user_agent: Some("ruston-cli/0.2.0 (macos)".into()),
        ..metadata("UID1", crate::DEFAULT_BASE_URL)
    };
    let saved_tokens = tokens("UID1", "acc", "ref");
    let skp = SecretString::from("skp-secret");
    session
        .save(&paths, "default", &store, &saved_tokens, &skp)
        .unwrap();

    let loaded = Session::load(&paths, "default", &store).unwrap().unwrap();
    let described = &loaded.session;
    let agent = described.user_agent.as_deref();
    assert_eq!(
        (
            described.uid.as_str(),
            described.app_version.as_str(),
            agent
        ),
        ("UID1", "Other", Some("ruston-cli/0.2.0 (macos)"))
    );
    let secrets = [&loaded.tokens.access, &loaded.tokens.refresh, &loaded.skp];
    let secrets = secrets.map(|secret| secret.expose_secret());
    assert_eq!(secrets, ["acc", "ref", "skp-secret"]);
    assert_eq!(loaded.tokens.uid, "UID1");
    // Only its owner may read the metadata.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let file = std::fs::metadata(paths.metadata_file("default")).unwrap();
        assert_eq!(file.permissions().mode() & 0o777, 0o600);
    }

    Session::clear(&paths, "default", &store).unwrap();
    assert!(Session::load(&paths, "default", &store).unwrap().is_none());
    assert!(!paths.metadata_file("default").exists());
    for key in [K_TOKENS, K_SKP, K_BASE_URL] {
        assert_eq!(store.get(key).unwrap(), None, "{key}");
    }
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn session_without_user_agent_round_trips_unchanged() {
    let base = unique_base();
    let paths = Paths::with_base(&base);
    let store = MemoryStore::new();
    let saved = saved_session("https://mail.proton.me/api", &paths, &store);
    assert_eq!(saved.user_agent, None);

    let loaded = Session::load(&paths, "default", &store).unwrap().unwrap();
    assert_eq!(loaded.session.user_agent, None);
    assert_eq!(loaded.session.uid, saved.uid);
    std::fs::remove_dir_all(base).unwrap();
}

/// What earlier versions saved, where and under which names, so that
/// their sessions still resume.
#[test]
fn stored_names_and_formats_stay_as_they_are() {
    let base = unique_base();
    let paths = Paths::with_base(&base);
    let store = MemoryStore::new();
    saved_session("https://mail.proton.me/api", &paths, &store);

    let sessions = base.join("sessions");
    assert_eq!(
        paths.metadata_file("default"),
        sessions.join("default.json")
    );
    assert_eq!(paths.lock_file("default"), sessions.join("default.lock"));
    let temporary = paths.metadata_temp_file("default");
    assert_eq!(temporary, sessions.join("default.json.tmp"));
    let written: serde_json::Value =
        serde_json::from_slice(&std::fs::read(sessions.join("default.json")).unwrap()).unwrap();
    assert_eq!(
        written,
        serde_json::json!({
            "uid": "UID1", "app_version": "Other", "base_url": "https://mail.proton.me/api",
            "password_mode": 1, "user_agent": null, "user_id": "user-1"
        })
    );
    for (key, value) in [
        ("auth_tokens_v1", r#"{"access":"acc","refresh":"ref"}"#),
        ("skp", "skp"),
        ("api_base_url_v1", "https://mail.proton.me/api"),
    ] {
        assert_eq!(store.get(key).unwrap().as_deref(), Some(value), "{key}");
    }
    // On disk, sessions live in the platform's folder for the application.
    if let Ok(system) = Paths::system() {
        let named = |part: std::path::Component| part.as_os_str() == "ruston-mail";
        assert!(system.base.components().any(named), "{:?}", system.base);
    }
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn a_profile_never_signed_in_to_has_no_session_to_load() {
    let base = unique_base();
    let paths = Paths::with_base(&base);
    let store = MemoryStore::new();
    assert!(
        Session::load(&paths, "never-used", &store)
            .unwrap()
            .is_none()
    );
    assert!(!Session::is_saved(&paths, "never-used"));
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn failed_refresh_write_keeps_the_previous_token_pair() {
    let base = unique_base();
    let paths = Paths::with_base(&base);
    let store = RejectTokenWrites::default();
    let first = tokens("UID1", "old-access", "old-refresh");
    let session = metadata("UID1", crate::DEFAULT_BASE_URL);
    let skp = SecretString::from("skp");
    session
        .save(&paths, "default", &store, &first, &skp)
        .unwrap();

    store.reject.store(true, Ordering::Relaxed);
    assert!(Session::save_tokens(&store, "new-access", "new-refresh").is_err());
    let kept = Session::load(&paths, "default", &store)
        .unwrap()
        .unwrap()
        .tokens;
    let pair = [kept.access.expose_secret(), kept.refresh.expose_secret()];
    assert_eq!(pair, ["old-access", "old-refresh"]);
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn a_login_whose_token_write_fails_leaves_no_earlier_tokens_behind() {
    let paths = Paths::with_base(unique_base());
    let store = RejectTokenWrites::default();
    let session = |uid: &str, base_url: &str| Session {
        uid: uid.into(),
        app_version: "Other".into(),
        base_url: base_url.into(),
        password_mode: 1,
        user_agent: None,
        user_id: "user-1".into(),
    };
    let tokens = |uid: &str| Tokens {
        uid: uid.into(),
        access: SecretString::from(format!("{uid}-access")),
        refresh: SecretString::from(format!("{uid}-refresh")),
    };
    session("UID1", "https://mail.proton.me/api")
        .save(
            &paths,
            "default",
            &store,
            &tokens("UID1"),
            &SecretString::from("skp1"),
        )
        .unwrap();

    // A later login to another API address fails at the token write.
    store.reject.store(true, Ordering::Relaxed);
    assert!(
        session("UID2", "https://other.example/api")
            .save(
                &paths,
                "default",
                &store,
                &tokens("UID2"),
                &SecretString::from("skp2")
            )
            .is_err()
    );
    // UID1's tokens must not be resumed for, and sent to, the new address.
    assert!(Session::load(&paths, "default", &store).unwrap().is_none());
    assert!(store.get(K_TOKENS).unwrap().is_none());
    assert!(store.get(K_SKP).unwrap().is_none());
}

#[test]
fn a_session_whose_tokens_are_gone_does_not_resume() {
    let base = unique_base();
    let paths = Paths::with_base(&base);
    let store = MemoryStore::new();
    saved_session(crate::DEFAULT_BASE_URL, &paths, &store);
    store.delete(K_TOKENS).unwrap();

    assert!(Session::load(&paths, "default", &store).unwrap().is_none());
    std::fs::remove_dir_all(base).unwrap();
}

/// The metadata a login at `base_url` saves for session `uid`.
fn metadata(uid: &str, base_url: &str) -> Session {
    Session {
        uid: uid.into(),
        app_version: "Other".into(),
        base_url: base_url.into(),
        password_mode: 1,
        user_agent: None,
        user_id: "user-1".into(),
    }
}

/// The access and refresh tokens of session `uid`.
fn tokens(uid: &str, access: &str, refresh: &str) -> Tokens {
    Tokens {
        uid: uid.into(),
        access: SecretString::from(access),
        refresh: SecretString::from(refresh),
    }
}

/// Session `UID1` at `base_url`, saved as the profile `default` with the
/// tokens `acc` and `ref` and the key password `skp`.
fn saved_session(base_url: &str, paths: &Paths, store: &MemoryStore) -> Session {
    let session = metadata("UID1", base_url);
    let saved_tokens = tokens("UID1", "acc", "ref");
    let skp = SecretString::from("skp");
    session
        .save(paths, "default", store, &saved_tokens, &skp)
        .unwrap();
    session
}

fn rewrite_base_url(paths: &Paths, session: &Session, base_url: &str) {
    let edited = Session {
        base_url: base_url.into(),
        ..session.clone()
    };
    std::fs::write(
        paths.metadata_file("default"),
        serde_json::to_vec(&edited).unwrap(),
    )
    .unwrap();
}

#[test]
fn edited_api_address_does_not_resume_a_session() {
    let base = unique_base();
    let paths = Paths::with_base(&base);
    let store = MemoryStore::new();
    let session = saved_session("https://mail.proton.me/api", &paths, &store);

    for edited in ["https://example.test/api", "http://mail.proton.me/api"] {
        rewrite_base_url(&paths, &session, edited);
        assert!(matches!(
            Session::load(&paths, "default", &store),
            Err(Error::SessionAddressMismatch)
        ));
    }

    rewrite_base_url(&paths, &session, "https://mail.proton.me/api");
    assert!(Session::load(&paths, "default", &store).unwrap().is_some());
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn selected_api_address_resumes_only_while_bound() {
    let base = unique_base();
    let paths = Paths::with_base(&base);
    let store = MemoryStore::new();
    saved_session("http://127.0.0.1:8080", &paths, &store);
    assert!(Session::load(&paths, "default", &store).unwrap().is_some());

    store.delete(K_BASE_URL).unwrap();
    assert!(matches!(
        Session::load(&paths, "default", &store),
        Err(Error::SessionAddressMismatch)
    ));
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn a_session_bound_to_no_address_does_not_resume() {
    let base = unique_base();
    let paths = Paths::with_base(&base);
    let store = MemoryStore::new();
    saved_session("https://mail.proton.me/api", &paths, &store);
    store.delete(K_BASE_URL).unwrap();
    assert!(matches!(
        Session::load(&paths, "default", &store),
        Err(Error::SessionAddressMismatch)
    ));
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn session_metadata_requires_password_mode() {
    let session = Session {
        uid: "UID1".into(),
        app_version: "Other".into(),
        base_url: "https://mail.proton.me/api".into(),
        password_mode: 1,
        user_agent: None,
        user_id: "user-1".into(),
    };
    let current = serde_json::to_value(session).unwrap();
    let mut incomplete = current;
    incomplete.as_object_mut().unwrap().remove("password_mode");
    assert!(serde_json::from_value::<Session>(incomplete).is_err());
}

#[test]
fn a_later_version_s_session_loads_and_one_without_its_account_does_not() {
    let base = unique_base();
    let paths = Paths::with_base(&base);
    let store = MemoryStore::new();
    let session = saved_session("https://mail.proton.me/api", &paths, &store);

    // Written by a later version, with a field this one does not know.
    let mut newer = serde_json::to_value(&session).unwrap();
    newer["recorded_by_a_later_version"] = true.into();
    std::fs::write(paths.metadata_file("default"), newer.to_string()).unwrap();
    let loaded = Session::load(&paths, "default", &store).unwrap().unwrap();
    assert_eq!(loaded.session.uid, "UID1");
    assert_eq!(loaded.session.user_id, "user-1");

    // Without the account it belongs to, as before 0.8.0, it is no session.
    let mut older = serde_json::to_value(&session).unwrap();
    older.as_object_mut().unwrap().remove("user_id");
    std::fs::write(paths.metadata_file("default"), older.to_string()).unwrap();
    assert!(Session::load(&paths, "default", &store).unwrap().is_none());
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn metadata_that_is_no_session_is_ignored_but_a_failed_read_is_an_error() {
    let base = unique_base();
    let paths = Paths::with_base(&base);
    let store = MemoryStore::new();
    saved_session("https://mail.proton.me/api", &paths, &store);

    // Metadata that is not a session: there is none to resume, and its
    // secrets are still cleared.
    std::fs::write(paths.metadata_file("default"), b"not json").unwrap();
    assert!(Session::load(&paths, "default", &store).unwrap().is_none());
    Session::clear_if_current(&paths, "default", &store, "UID1").unwrap();
    assert!(store.get(K_TOKENS).unwrap().is_none());

    // A file that cannot be read at all is an error, never no session.
    let _ = std::fs::remove_file(paths.metadata_file("default"));
    std::fs::create_dir(paths.metadata_file("default")).unwrap();
    assert!(Session::load(&paths, "default", &store).is_err());
    assert!(Session::clear_if_current(&paths, "default", &store, "UID1").is_err());
    std::fs::remove_dir_all(base).unwrap();
}

/// Counts reads of the key passphrase.
#[derive(Default)]
struct CountingPassphraseReads {
    inner: MemoryStore,
    reads: std::sync::atomic::AtomicUsize,
}

impl SecretStore for CountingPassphraseReads {
    fn set(&self, key: &str, value: &str) -> Result<()> {
        self.inner.set(key, value)
    }

    fn get(&self, key: &str) -> Result<Option<String>> {
        if key == K_SKP {
            self.reads.fetch_add(1, Ordering::Relaxed);
        }
        self.inner.get(key)
    }

    fn delete(&self, key: &str) -> Result<()> {
        self.inner.delete(key)
    }
}

#[test]
fn a_token_refresh_needs_only_the_tokens() {
    let base = unique_base();
    let paths = Paths::with_base(&base);
    let store = CountingPassphraseReads::default();
    let session = metadata("UID1", crate::DEFAULT_BASE_URL);
    let saved_tokens = tokens("UID1", "acc", "ref");
    session
        .save(
            &paths,
            "default",
            &store,
            &saved_tokens,
            &SecretString::from("skp"),
        )
        .unwrap();
    let refresh_tokens = || {
        let (_lock, tokens) = Session::lock_and_load_tokens(&paths, "default", &store)?;
        Ok::<_, Error>(tokens.map(|tokens| tokens.refresh.expose_secret().to_owned()))
    };

    assert_eq!(refresh_tokens().unwrap().as_deref(), Some("ref"));
    assert_eq!(store.reads.load(Ordering::Relaxed), 0);

    // Without the passphrase the session cannot be resumed, but a client
    // already running can still renew its tokens.
    store.inner.delete(K_SKP).unwrap();
    assert!(Session::load(&paths, "default", &store).unwrap().is_none());
    assert_eq!(refresh_tokens().unwrap().as_deref(), Some("ref"));

    // An address the store does not vouch for is still refused.
    rewrite_base_url(&paths, &session, "https://elsewhere.example/api");
    assert!(matches!(
        refresh_tokens(),
        Err(Error::SessionAddressMismatch)
    ));
    std::fs::remove_dir_all(base).unwrap();
}
