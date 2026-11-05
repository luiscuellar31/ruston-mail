//! A signed-in session, saved for the next run under a profile's name: what
//! is not secret in a file, and the tokens and key password in a
//! [`SecretStore`]. A lock file for each profile keeps two processes from
//! saving or renewing the same session at once.

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

/// A session's ID and the pair of tokens its requests carry.
#[derive(Clone)]
pub struct Tokens {
    /// The session's ID.
    pub uid: String,
    /// The token each request presents.
    pub access: SecretString,
    /// The token that renews `access`.
    pub refresh: SecretString,
}

/// What a session's file holds: nothing secret.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Session {
    /// The session's ID.
    pub uid: String,
    /// The client name and version its requests give in `x-pm-appversion`.
    pub app_version: String,
    /// The API address the session was opened at, where its tokens go.
    pub base_url: String,
    /// Password mode, one of
    /// [`password_mode`](crate::model::enums::password_mode).
    pub password_mode: u8,
    /// The `User-Agent` its requests send, when sign-in was given one.
    pub user_agent: Option<String>,
    /// The account's user ID, which names its local cache, so the cache can
    /// be opened without asking Proton. Sessions saved before it was recorded
    /// have none until they are next resumed.
    #[serde(default)]
    pub user_id: Option<String>,
}

/// A saved session read back whole: its file and its secrets.
pub struct LoadedSession {
    /// What its file holds.
    pub session: Session,
    /// Its ID and tokens, from the secret store.
    pub tokens: Tokens,
    /// The key password that opens the account's keys, from the secret
    /// store.
    pub skp: SecretString,
}

/// Where sessions are saved on disk: under the platform's configuration
/// folder, or under another folder in tests.
#[derive(Clone)]
pub struct Paths {
    base: PathBuf,
}

impl Paths {
    /// Paths in this platform's configuration folder for `ruston-mail`.
    pub fn system() -> Result<Self> {
        directories::ProjectDirs::from("", "", crate::STORAGE_NAME)
            .map(|folders| Self::with_base(folders.config_dir()))
            .ok_or_else(|| Error::Session("cannot resolve config dir".into()))
    }
    /// Paths under `base` instead, as tests keep them.
    pub fn with_base(base: impl Into<PathBuf>) -> Self {
        Paths { base: base.into() }
    }
    /// The folder of every profile's session files.
    fn sessions_dir(&self) -> PathBuf {
        self.base.join("sessions")
    }
    /// The file of a profile's session metadata.
    fn metadata_file(&self, profile: &str) -> PathBuf {
        self.sessions_dir().join(format!("{profile}.json"))
    }
    /// The file whose lock a profile's readers and writers take.
    fn lock_file(&self, profile: &str) -> PathBuf {
        self.metadata_file(profile).with_extension("lock")
    }
    /// Where new metadata is written before it replaces the old in one
    /// rename.
    fn metadata_temp_file(&self, profile: &str) -> PathBuf {
        self.metadata_file(profile).with_extension("json.tmp")
    }
}

/// Permissions of the sessions folder and of the files in it, on Unix.
const PRIVATE_DIR_MODE: u32 = 0o700;
const PRIVATE_FILE_MODE: u32 = 0o600;

/// Gives `path` the Unix permissions `mode`; on other systems it does
/// nothing.
#[cfg(unix)]
fn set_unix_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    Ok(())
}
#[cfg(not(unix))]
fn set_unix_mode(_path: &Path, _mode: u32) -> Result<()> {
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
        set_unix_mode(&dir, PRIVATE_DIR_MODE)?;
        let path = paths.lock_file(profile);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
        set_unix_mode(&path, PRIVATE_FILE_MODE)?;
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

    /// Whether the profile's session metadata is on disk. Its secrets are
    /// not looked at, so no keychain is asked.
    pub(crate) fn is_saved(paths: &Paths, profile: &str) -> bool {
        validate_profile_name(profile).is_ok() && paths.metadata_file(profile).is_file()
    }

    /// Waits for whoever holds the profile lock, and lets it go at once. A
    /// token refresh holds it from sending the old refresh token until the
    /// new pair is saved. A profile never signed in to has no lock file,
    /// and nothing to wait for.
    pub(crate) fn wait_unlocked(paths: &Paths, profile: &str) -> Result<()> {
        validate_profile_name(profile)?;
        if !paths.lock_file(profile).exists() {
            return Ok(());
        }
        Self::lock(paths, profile).map(drop)
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

    /// Saves the session under `profile`: this metadata in its file, and in
    /// `store` the `tokens`, the key password `skp` and the API address the
    /// tokens belong to.
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
        set_unix_mode(&dir, PRIVATE_DIR_MODE)?;
        let file = paths.metadata_file(profile);
        let temporary = paths.metadata_temp_file(profile);
        let json = serde_json::to_vec_pretty(self)?;
        let written = std::fs::write(&temporary, json)
            .map_err(Error::from)
            .and_then(|()| set_unix_mode(&temporary, PRIVATE_FILE_MODE))
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

    /// Saves the pair of tokens a refresh issued, and nothing else of the
    /// session.
    pub fn save_tokens(store: &dyn SecretStore, access: &str, refresh: &str) -> Result<()> {
        let pair = Zeroizing::new(serde_json::to_string(&StoredTokens { access, refresh })?);
        store.set(K_TOKENS, &pair)?;
        Ok(())
    }

    /// The session saved under `profile`. `None` when any part of it is
    /// missing, or its file holds no session.
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
            _ => return Err(Error::SessionAddressMismatch),
        }
        let tokens = Tokens {
            uid: session.uid.clone(),
            access: SecretString::from(pair.access),
            refresh: SecretString::from(pair.refresh),
        };
        Ok(Some((session, tokens)))
    }

    /// Removes the session saved under `profile`, its file and its secrets.
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
        let bytes = match std::fs::read(paths.metadata_file(profile)) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        Ok(serde_json::from_slice(&bytes).ok())
    }

    fn clear_unlocked(paths: &Paths, profile: &str, store: &dyn SecretStore) -> Result<()> {
        let file = paths.metadata_file(profile);
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
            user_id: None,
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
                "password_mode": 1, "user_agent": null, "user_id": null
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
        let base = unique_base();
        let paths = Paths::with_base(&base);
        let store = MemoryStore::new();
        saved_session(crate::DEFAULT_BASE_URL, &paths, &store);
        // The token pair as two entries, the way it was once kept.
        store.delete(K_TOKENS).unwrap();
        for (key, token) in [("access_token", "acc"), ("refresh_token", "ref")] {
            store.set(key, token).unwrap();
        }

        assert!(Session::load(&paths, "default", &store).unwrap().is_none());
        Session::clear(&paths, "default", &store).unwrap();
        assert!(store.get(K_TOKENS).unwrap().is_none());
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
            user_id: None,
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
            Err(Error::SessionAddressMismatch)
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
        std::fs::write(paths.metadata_file("default"), older.to_string()).unwrap();
        let loaded = Session::load(&paths, "default", &store).unwrap().unwrap();
        assert_eq!(loaded.session.uid, "UID1");
        assert_eq!(loaded.session.user_id, None);

        // Written by a later version, with a field this one does not know.
        let mut newer = serde_json::to_value(&session).unwrap();
        newer["user_id"] = "user-1".into();
        newer["recorded_by_a_later_version"] = true.into();
        std::fs::write(paths.metadata_file("default"), newer.to_string()).unwrap();
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
        std::fs::write(paths.metadata_file("default"), b"not json").unwrap();
        assert!(Session::load(&paths, "default", &store).unwrap().is_none());
        Session::record_user_id(&paths, "default", "UID1", "user-1").unwrap();
        assert_eq!(
            std::fs::read(paths.metadata_file("default")).unwrap(),
            b"not json"
        );
        Session::clear_if_current(&paths, "default", &store, "UID1").unwrap();
        assert!(store.get(K_TOKENS).unwrap().is_none());

        // A file that cannot be read at all is an error, never no session.
        let _ = std::fs::remove_file(paths.metadata_file("default"));
        std::fs::create_dir(paths.metadata_file("default")).unwrap();
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
            let file = std::fs::metadata(paths.metadata_file("default")).unwrap();
            assert_eq!(file.permissions().mode() & 0o777, 0o600);
        }

        // Nothing to record without a saved session.
        Session::clear(&paths, "default", &store).unwrap();
        Session::record_user_id(&paths, "default", "UID1", "user-1").unwrap();
        assert!(!paths.metadata_file("default").exists());
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
}
