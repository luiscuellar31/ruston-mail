//! Session model + on-disk persistence (non-secret metadata file + secrets in
//! the `SecretStore`).

pub mod secret_store;

pub use secret_store::{KeyringStore, MemoryStore, SecretStore};

use crate::error::{Error, Result};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

const K_TOKENS: &str = "auth_tokens_v1";
const K_SKP: &str = "skp";
const K_BASE_URL: &str = "api_base_url_v1";
/// How long to wait for another process to let go of a profile, as long as
/// SQLite waits for the cache.
const PROFILE_LOCK_TIMEOUT: Duration = Duration::from_secs(30);
/// How often a waiting process tries the profile lock again.
const PROFILE_LOCK_RETRY: Duration = Duration::from_millis(50);

/// Reject empty profile names, path syntax, and control characters.
pub fn validate_profile_name(profile: &str) -> Result<()> {
    let mut components = Path::new(profile).components();
    if profile.contains(['/', '\\', ':'])
        || profile.chars().any(char::is_control)
        || !matches!(components.next(), Some(Component::Normal(_)))
        || components.next().is_some()
    {
        return Err(Error::Session(
            "profile must be a non-empty filename without path syntax or control characters".into(),
        ));
    }
    Ok(())
}

#[derive(Serialize)]
struct StoredTokens<'a> {
    access: &'a str,
    refresh: &'a str,
}

#[derive(Deserialize)]
struct LoadedTokens {
    access: String,
    refresh: String,
}

/// Auth tokens held in memory.
#[derive(Clone)]
pub struct Tokens {
    /// Session UID.
    pub uid: String,
    /// Access token.
    pub access: SecretString,
    /// Refresh token.
    pub refresh: SecretString,
}

/// Non-secret session metadata persisted to disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Session {
    /// Session UID.
    pub uid: String,
    /// App-version string sent on every request.
    pub app_version: String,
    /// API base URL for this session.
    pub base_url: String,
    /// Password mode, one of
    /// [`password_mode`](crate::model::enums::password_mode).
    pub password_mode: u8,
    /// Optional `User-Agent` recorded for this session.
    pub user_agent: Option<String>,
    /// The account's user ID, which names its local cache, so the cache can
    /// be opened without asking Proton. Sessions saved before it was recorded
    /// have none until they are next resumed.
    #[serde(default)]
    pub user_id: Option<String>,
}

/// A fully-loaded session (metadata + secrets).
pub struct LoadedSession {
    /// Non-secret session metadata.
    pub session: Session,
    /// Auth tokens loaded from the secret store.
    pub tokens: Tokens,
    /// Secret key passphrase loaded from the secret store.
    pub skp: SecretString,
}

/// Resolves on-disk paths for sessions (overridable in tests).
#[derive(Clone)]
pub struct Paths {
    base: PathBuf,
}

impl Paths {
    /// Platform config directory for `ruston-mail`.
    pub fn system() -> Result<Self> {
        let dirs = directories::ProjectDirs::from("", "", crate::STORAGE_NAME)
            .ok_or_else(|| Error::Session("cannot resolve config dir".into()))?;
        Ok(Paths {
            base: dirs.config_dir().to_path_buf(),
        })
    }
    /// Resolve paths under an explicit base directory (used in tests).
    pub fn with_base(base: impl Into<PathBuf>) -> Self {
        Paths { base: base.into() }
    }
    fn sessions_dir(&self) -> PathBuf {
        self.base.join("sessions")
    }
    fn session_file(&self, profile: &str) -> PathBuf {
        self.sessions_dir().join(format!("{profile}.json"))
    }
    /// The file whose lock a profile's readers and writers take.
    fn lock_file(&self, profile: &str) -> PathBuf {
        self.session_file(profile).with_extension("lock")
    }
    /// Where new metadata is written before it replaces the old in one
    /// rename.
    fn metadata_temp_file(&self, profile: &str) -> PathBuf {
        self.session_file(profile).with_extension("json.tmp")
    }
}

/// Permissions of the sessions folder and of the files in it, on Unix.
const PRIVATE_DIR_MODE: u32 = 0o700;
const PRIVATE_FILE_MODE: u32 = 0o600;

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    Ok(())
}
#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

impl Session {
    fn lock(paths: &Paths, profile: &str) -> Result<File> {
        Self::lock_within(paths, profile, PROFILE_LOCK_TIMEOUT)
    }

    /// Takes the profile lock, giving up after `timeout`: a process that
    /// holds it, stopped with Ctrl-Z or waiting on an unanswered keychain
    /// prompt, must not stall every other one for good.
    fn lock_within(paths: &Paths, profile: &str, timeout: Duration) -> Result<File> {
        validate_profile_name(profile)?;
        let dir = paths.sessions_dir();
        std::fs::create_dir_all(&dir)?;
        set_mode(&dir, PRIVATE_DIR_MODE)?;
        let path = paths.lock_file(profile);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
        set_mode(&path, PRIVATE_FILE_MODE)?;
        let deadline = Instant::now() + timeout;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(file),
                Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                    std::thread::sleep(PROFILE_LOCK_RETRY);
                }
                Err(TryLockError::WouldBlock) => {
                    return Err(Error::Session(format!(
                        "profile {profile} is busy in another Ruston process; try again"
                    )));
                }
                Err(TryLockError::Error(error)) => return Err(error.into()),
            }
        }
    }

    pub(crate) fn lock_and_load(
        paths: &Paths,
        profile: &str,
        store: &dyn SecretStore,
    ) -> Result<(File, Option<LoadedSession>)> {
        let lock = Self::lock(paths, profile)?;
        let loaded = Self::load_unlocked(paths, profile, store)?;
        Ok((lock, loaded))
    }

    /// Persist metadata to disk and secrets to the store.
    pub fn save(
        &self,
        paths: &Paths,
        profile: &str,
        store: &dyn SecretStore,
        tokens: &Tokens,
        skp: &SecretString,
    ) -> Result<()> {
        let _lock = Self::lock(paths, profile)?;
        // The previous login's secrets go before any of this one is written,
        // so a save that fails part way leaves no earlier tokens beside this
        // session's metadata and API address, where they would be sent.
        store.delete(K_TOKENS)?;
        store.delete(K_SKP)?;
        self.write_metadata(paths, profile)?;
        store.set(K_BASE_URL, &self.base_url)?;
        Self::save_tokens(
            store,
            tokens.access.expose_secret(),
            tokens.refresh.expose_secret(),
        )?;
        store.set(K_SKP, skp.expose_secret())?;
        Ok(())
    }

    /// Replaces the metadata file in one rename, so a write that fails part
    /// way leaves the previous file whole. The caller holds the profile lock.
    fn write_metadata(&self, paths: &Paths, profile: &str) -> Result<()> {
        let dir = paths.sessions_dir();
        std::fs::create_dir_all(&dir)?;
        set_mode(&dir, PRIVATE_DIR_MODE)?;
        let file = paths.session_file(profile);
        let temporary = paths.metadata_temp_file(profile);
        let json = serde_json::to_vec_pretty(self)?;
        let written = std::fs::write(&temporary, json)
            .map_err(Error::from)
            .and_then(|()| set_mode(&temporary, PRIVATE_FILE_MODE))
            .and_then(|()| std::fs::rename(&temporary, &file).map_err(Error::from));
        if written.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        written
    }

    /// Records the account's user ID in the saved metadata of session `uid`,
    /// once it is known. Leaves a later login in the same profile alone.
    pub(crate) fn record_user_id(
        paths: &Paths,
        profile: &str,
        uid: &str,
        user_id: &str,
    ) -> Result<()> {
        let _lock = Self::lock(paths, profile)?;
        let Some(mut session) = Self::read_metadata(paths, profile)? else {
            return Ok(());
        };
        if session.uid != uid || session.user_id.as_deref() == Some(user_id) {
            return Ok(());
        }
        session.user_id = Some(user_id.to_owned());
        session.write_metadata(paths, profile)
    }

    /// Persist only the rotated tokens (after a refresh).
    pub fn save_tokens(store: &dyn SecretStore, access: &str, refresh: &str) -> Result<()> {
        let pair = Zeroizing::new(serde_json::to_string(&StoredTokens { access, refresh })?);
        store.set(K_TOKENS, &pair)?;
        Ok(())
    }

    /// Load a session if present and complete.
    pub fn load(
        paths: &Paths,
        profile: &str,
        store: &dyn SecretStore,
    ) -> Result<Option<LoadedSession>> {
        let (_lock, loaded) = Self::lock_and_load(paths, profile, store)?;
        Ok(loaded)
    }

    /// The saved tokens under the profile lock, for a token refresh: it needs
    /// neither the key passphrase nor anything else from the store.
    pub(crate) fn lock_and_load_tokens(
        paths: &Paths,
        profile: &str,
        store: &dyn SecretStore,
    ) -> Result<(File, Option<Tokens>)> {
        let lock = Self::lock(paths, profile)?;
        let tokens = Self::load_tokens_unlocked(paths, profile, store)?.map(|(_, tokens)| tokens);
        Ok((lock, tokens))
    }

    fn load_unlocked(
        paths: &Paths,
        profile: &str,
        store: &dyn SecretStore,
    ) -> Result<Option<LoadedSession>> {
        let Some((session, tokens)) = Self::load_tokens_unlocked(paths, profile, store)? else {
            return Ok(None);
        };
        let Some(skp) = store.get(K_SKP)? else {
            return Ok(None);
        };
        Ok(Some(LoadedSession {
            session,
            tokens,
            skp: SecretString::from(skp),
        }))
    }

    /// The saved metadata and tokens, when both are present and the tokens'
    /// API address is the one the metadata names.
    fn load_tokens_unlocked(
        paths: &Paths,
        profile: &str,
        store: &dyn SecretStore,
    ) -> Result<Option<(Session, Tokens)>> {
        let Some(session) = Self::read_metadata(paths, profile)? else {
            return Ok(None);
        };
        let Some(pair) = store.get(K_TOKENS)? else {
            return Ok(None);
        };
        let pair = Zeroizing::new(pair);
        let pair: LoadedTokens = serde_json::from_str(&pair)
            .map_err(|e| Error::Session(format!("invalid stored auth tokens: {e}")))?;
        // The metadata file is not secret, yet it names where the saved tokens
        // are sent. Trust only an address the secret store vouches for.
        match store.get(K_BASE_URL)? {
            Some(bound) if bound == session.base_url => {}
            // Sessions saved before the address was bound used the default API
            // unless a login chose another one.
            None if session.base_url == crate::DEFAULT_BASE_URL => {}
            _ => {
                return Err(Error::Session(
                    "saved session does not match the API address it was created for; sign in again"
                        .into(),
                ));
            }
        }
        let tokens = Tokens {
            uid: session.uid.clone(),
            access: SecretString::from(pair.access),
            refresh: SecretString::from(pair.refresh),
        };
        Ok(Some((session, tokens)))
    }

    /// Remove the on-disk file and stored secrets.
    pub fn clear(paths: &Paths, profile: &str, store: &dyn SecretStore) -> Result<()> {
        let _lock = Self::lock(paths, profile)?;
        Self::clear_unlocked(paths, profile, store)
    }

    /// A pending logout must not remove a later login in the same profile.
    pub(crate) fn clear_if_current(
        paths: &Paths,
        profile: &str,
        store: &dyn SecretStore,
        uid: &str,
    ) -> Result<()> {
        let _lock = Self::lock(paths, profile)?;
        // Incomplete metadata cannot resume a session; still remove its secrets.
        if Self::read_metadata(paths, profile)?.is_some_and(|session| session.uid != uid) {
            return Ok(());
        }
        Self::clear_unlocked(paths, profile, store)
    }

    /// The saved metadata of `profile`. `None` when there is none, or when it
    /// is not a session: either way no session can resume from it. Any other
    /// failure to read it is an error.
    fn read_metadata(paths: &Paths, profile: &str) -> Result<Option<Session>> {
        let bytes = match std::fs::read(paths.session_file(profile)) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        Ok(serde_json::from_slice(&bytes).ok())
    }

    fn clear_unlocked(paths: &Paths, profile: &str, store: &dyn SecretStore) -> Result<()> {
        let file = paths.session_file(profile);
        // What an interrupted metadata write left behind goes too.
        let _ = std::fs::remove_file(paths.metadata_temp_file(profile));
        let metadata = match std::fs::remove_file(&file) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Error::from(e)),
        };
        // Attempt every removal under the lock even if an earlier one fails.
        let tokens = store.delete(K_TOKENS);
        let skp = store.delete(K_SKP);
        let base_url = store.delete(K_BASE_URL);
        metadata.and(tokens).and(skp).and(base_url)
    }
}

#[cfg(test)]
mod tests {
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
    fn clear_removes_secrets_even_when_metadata_cannot_be_removed() {
        let base = unique_base();
        let paths = Paths::with_base(&base);
        // A directory at the metadata path makes remove_file fail deterministically.
        std::fs::create_dir_all(paths.session_file("test")).unwrap();
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
        let session = Session {
            uid: "UID1".into(),
            app_version: "Other".into(),
            base_url: "https://mail.proton.me/api".into(),
            password_mode: 1,
            user_agent: None,
            user_id: None,
        };
        let tokens = Tokens {
            uid: "UID1".into(),
            access: SecretString::from("acc"),
            refresh: SecretString::from("ref"),
        };
        assert!(matches!(
            session.save(
                &paths,
                "../outside",
                &store,
                &tokens,
                &SecretString::from("skp"),
            ),
            Err(Error::Session(_))
        ));
        assert_eq!(std::fs::read(&outside).unwrap(), b"keep this file");
        assert!(!base.join("outside.lock").exists());
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn save_load_clear_roundtrip() {
        let paths = Paths::with_base(unique_base());
        let store = MemoryStore::new();
        let s = Session {
            uid: "UID1".into(),
            app_version: "Other".into(),
            base_url: "https://mail.proton.me/api".into(),
            password_mode: 1,
            user_agent: Some("ruston-cli/0.2.0 (macos)".into()),
            user_id: None,
        };
        let tokens = Tokens {
            uid: "UID1".into(),
            access: SecretString::from("acc"),
            refresh: SecretString::from("ref"),
        };
        s.save(
            &paths,
            "default",
            &store,
            &tokens,
            &SecretString::from("skp-secret"),
        )
        .unwrap();

        let loaded = Session::load(&paths, "default", &store).unwrap().unwrap();
        assert_eq!(loaded.session.uid, "UID1");
        assert_eq!(loaded.session.app_version, "Other");
        assert_eq!(
            loaded.session.user_agent.as_deref(),
            Some("ruston-cli/0.2.0 (macos)")
        );
        assert_eq!(loaded.tokens.access.expose_secret(), "acc");
        assert_eq!(loaded.skp.expose_secret(), "skp-secret");
        assert!(store.get(K_TOKENS).unwrap().is_some());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(paths.session_file("default"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        Session::clear(&paths, "default", &store).unwrap();
        assert!(Session::load(&paths, "default", &store).unwrap().is_none());
        assert!(store.get(K_TOKENS).unwrap().is_none());
    }

    #[test]
    fn session_without_user_agent_round_trips_unchanged() {
        let paths = Paths::with_base(unique_base());
        let store = MemoryStore::new();
        let s = Session {
            uid: "UID2".into(),
            app_version: "Other".into(),
            base_url: "https://mail.proton.me/api".into(),
            password_mode: 1,
            user_agent: None,
            user_id: None,
        };
        let tokens = Tokens {
            uid: "UID2".into(),
            access: SecretString::from("acc"),
            refresh: SecretString::from("ref"),
        };
        s.save(
            &paths,
            "default",
            &store,
            &tokens,
            &SecretString::from("skp"),
        )
        .unwrap();

        let loaded = Session::load(&paths, "default", &store).unwrap().unwrap();
        assert_eq!(loaded.session.user_agent, None);
        Session::clear(&paths, "default", &store).unwrap();
    }

    #[test]
    fn load_missing_returns_none() {
        let paths = Paths::with_base(unique_base());
        let store = MemoryStore::new();
        assert!(Session::load(&paths, "nope", &store).unwrap().is_none());
    }

    #[test]
    fn failed_refresh_write_keeps_the_previous_token_pair() {
        let paths = Paths::with_base(unique_base());
        let store = RejectTokenWrites::default();
        let session = Session {
            uid: "UID1".into(),
            app_version: "Other".into(),
            base_url: "https://mail.proton.me/api".into(),
            password_mode: 1,
            user_agent: None,
            user_id: None,
        };
        let tokens = Tokens {
            uid: "UID1".into(),
            access: SecretString::from("old-access"),
            refresh: SecretString::from("old-refresh"),
        };
        session
            .save(
                &paths,
                "default",
                &store,
                &tokens,
                &SecretString::from("skp"),
            )
            .unwrap();

        store.reject.store(true, Ordering::Relaxed);
        assert!(Session::save_tokens(&store, "new-access", "new-refresh").is_err());
        let loaded = Session::load(&paths, "default", &store).unwrap().unwrap();
        assert_eq!(loaded.tokens.access.expose_secret(), "old-access");
        assert_eq!(loaded.tokens.refresh.expose_secret(), "old-refresh");
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
            user_id: None,
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
    fn split_tokens_do_not_resume_a_session() {
        let paths = Paths::with_base(unique_base());
        let store = MemoryStore::new();
        let session = Session {
            uid: "UID1".into(),
            app_version: "Other".into(),
            base_url: "https://mail.proton.me/api".into(),
            password_mode: 1,
            user_agent: None,
            user_id: None,
        };
        let tokens = Tokens {
            uid: "UID1".into(),
            access: SecretString::from("old-access"),
            refresh: SecretString::from("old-refresh"),
        };
        session
            .save(
                &paths,
                "default",
                &store,
                &tokens,
                &SecretString::from("skp"),
            )
            .unwrap();
        store.delete(K_TOKENS).unwrap();
        store.set("access_token", "old-access").unwrap();
        store.set("refresh_token", "old-refresh").unwrap();

        assert!(Session::load(&paths, "default", &store).unwrap().is_none());
        Session::clear(&paths, "default", &store).unwrap();
        assert!(store.get(K_TOKENS).unwrap().is_none());
    }

    fn saved_session(base_url: &str, paths: &Paths, store: &MemoryStore) -> Session {
        let session = Session {
            uid: "UID1".into(),
            app_version: "Other".into(),
            base_url: base_url.into(),
            password_mode: 1,
            user_agent: None,
            user_id: None,
        };
        let tokens = Tokens {
            uid: "UID1".into(),
            access: SecretString::from("acc"),
            refresh: SecretString::from("ref"),
        };
        session
            .save(paths, "default", store, &tokens, &SecretString::from("skp"))
            .unwrap();
        session
    }

    fn rewrite_base_url(paths: &Paths, session: &Session, base_url: &str) {
        let edited = Session {
            base_url: base_url.into(),
            ..session.clone()
        };
        std::fs::write(
            paths.session_file("default"),
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
                Err(Error::Session(_))
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
            Err(Error::Session(_))
        ));
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn session_saved_before_the_binding_resumes_on_the_default_api_only() {
        let base = unique_base();
        let paths = Paths::with_base(&base);
        let store = MemoryStore::new();
        let session = saved_session("https://mail.proton.me/api", &paths, &store);
        store.delete(K_BASE_URL).unwrap();
        assert!(Session::load(&paths, "default", &store).unwrap().is_some());

        rewrite_base_url(&paths, &session, "https://example.test/api");
        assert!(matches!(
            Session::load(&paths, "default", &store),
            Err(Error::Session(_))
        ));
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn clear_removes_the_bound_api_address() {
        let base = unique_base();
        let paths = Paths::with_base(&base);
        let store = MemoryStore::new();
        saved_session("https://mail.proton.me/api", &paths, &store);
        assert!(store.get(K_BASE_URL).unwrap().is_some());

        Session::clear(&paths, "default", &store).unwrap();
        assert!(store.get(K_BASE_URL).unwrap().is_none());
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
            user_id: None,
        };
        let current = serde_json::to_value(session).unwrap();
        let mut incomplete = current;
        incomplete.as_object_mut().unwrap().remove("password_mode");
        assert!(serde_json::from_value::<Session>(incomplete).is_err());
    }

    #[test]
    fn sessions_saved_by_other_versions_still_load() {
        let base = unique_base();
        let paths = Paths::with_base(&base);
        let store = MemoryStore::new();
        let session = saved_session("https://mail.proton.me/api", &paths, &store);

        // Written before the account ID was recorded.
        let mut older = serde_json::to_value(&session).unwrap();
        older.as_object_mut().unwrap().remove("user_id");
        std::fs::write(paths.session_file("default"), older.to_string()).unwrap();
        let loaded = Session::load(&paths, "default", &store).unwrap().unwrap();
        assert_eq!(loaded.session.uid, "UID1");
        assert_eq!(loaded.session.user_id, None);

        // Written by a later version, with a field this one does not know.
        let mut newer = serde_json::to_value(&session).unwrap();
        newer["user_id"] = "user-1".into();
        newer["recorded_by_a_later_version"] = true.into();
        std::fs::write(paths.session_file("default"), newer.to_string()).unwrap();
        let loaded = Session::load(&paths, "default", &store).unwrap().unwrap();
        assert_eq!(loaded.session.user_id.as_deref(), Some("user-1"));
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn metadata_that_is_no_session_is_ignored_but_a_failed_read_is_an_error() {
        let base = unique_base();
        let paths = Paths::with_base(&base);
        let store = MemoryStore::new();
        saved_session("https://mail.proton.me/api", &paths, &store);

        // Metadata that is not a session: there is none to resume, no ID is
        // recorded into it, and its secrets are still cleared.
        std::fs::write(paths.session_file("default"), b"not json").unwrap();
        assert!(Session::load(&paths, "default", &store).unwrap().is_none());
        Session::record_user_id(&paths, "default", "UID1", "user-1").unwrap();
        assert_eq!(
            std::fs::read(paths.session_file("default")).unwrap(),
            b"not json"
        );
        Session::clear_if_current(&paths, "default", &store, "UID1").unwrap();
        assert!(store.get(K_TOKENS).unwrap().is_none());

        // A file that cannot be read at all is an error, never no session.
        let _ = std::fs::remove_file(paths.session_file("default"));
        std::fs::create_dir(paths.session_file("default")).unwrap();
        assert!(Session::load(&paths, "default", &store).is_err());
        assert!(Session::record_user_id(&paths, "default", "UID1", "user-1").is_err());
        assert!(Session::clear_if_current(&paths, "default", &store, "UID1").is_err());
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn the_account_id_is_recorded_only_for_the_session_it_belongs_to() {
        let base = unique_base();
        let paths = Paths::with_base(&base);
        let store = MemoryStore::new();
        saved_session("https://mail.proton.me/api", &paths, &store);

        // A later login replaced the session: its file is left alone.
        Session::record_user_id(&paths, "default", "OTHER-UID", "user-2").unwrap();
        let loaded = Session::load(&paths, "default", &store).unwrap().unwrap();
        assert_eq!(loaded.session.user_id, None);

        Session::record_user_id(&paths, "default", "UID1", "user-1").unwrap();
        let loaded = Session::load(&paths, "default", &store).unwrap().unwrap();
        assert_eq!(loaded.session.user_id.as_deref(), Some("user-1"));
        // Everything else, and the secrets, are as they were.
        assert_eq!(loaded.session.uid, "UID1");
        assert_eq!(loaded.tokens.access.expose_secret(), "acc");
        assert_eq!(loaded.skp.expose_secret(), "skp");

        // The file is replaced whole, with its permissions kept.
        assert!(!paths.metadata_temp_file("default").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(paths.session_file("default"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        // Nothing to record without a saved session.
        Session::clear(&paths, "default", &store).unwrap();
        Session::record_user_id(&paths, "default", "UID1", "user-1").unwrap();
        assert!(!paths.session_file("default").exists());
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
        let session = Session {
            uid: "UID1".into(),
            app_version: "Other".into(),
            base_url: "https://mail.proton.me/api".into(),
            password_mode: 1,
            user_agent: None,
            user_id: None,
        };
        let tokens = Tokens {
            uid: "UID1".into(),
            access: SecretString::from("acc"),
            refresh: SecretString::from("ref"),
        };
        session
            .save(
                &paths,
                "default",
                &store,
                &tokens,
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
        assert!(matches!(refresh_tokens(), Err(Error::Session(_))));
        std::fs::remove_dir_all(base).unwrap();
    }
}
