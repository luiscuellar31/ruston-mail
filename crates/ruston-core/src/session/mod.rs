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
    /// be opened without asking Proton.
    pub user_id: String,
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
        if store.get(K_BASE_URL)?.as_deref() != Some(session.base_url.as_str()) {
            return Err(Error::SessionAddressMismatch);
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
mod tests;
