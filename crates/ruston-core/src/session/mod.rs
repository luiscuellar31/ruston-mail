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
use std::sync::Arc;
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
#[derive(Debug, Clone)]
pub struct Tokens {
    /// The session's ID.
    pub uid: String,
    /// The token each request presents.
    pub access: SecretString,
    /// The token that renews `access`.
    pub refresh: SecretString,
}

impl Tokens {
    /// Whether these are `other`'s: the same session with the same pair.
    pub fn same_as(&self, other: &Tokens) -> bool {
        self.uid == other.uid
            && self.access.expose_secret() == other.access.expose_secret()
            && self.refresh.expose_secret() == other.refresh.expose_secret()
    }
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

/// Where one profile's session is saved: its files under `paths` and its
/// secrets in `store`. Held together, the file of one profile is never read
/// or written beside the secrets of another.
#[derive(Clone)]
pub struct ProfileStorage {
    paths: Paths,
    profile: String,
    store: Arc<dyn SecretStore>,
}

impl ProfileStorage {
    /// `profile`'s session where the platform keeps it: this system's
    /// configuration folder and credential store.
    pub fn system(profile: &str) -> Result<Self> {
        validate_profile_name(profile)?;
        Ok(Self::new(
            Paths::system()?,
            profile,
            Arc::new(KeyringStore::new(profile)),
        ))
    }

    /// `profile`'s session under `paths` and in `store` instead, as tests
    /// keep it.
    pub fn new(paths: Paths, profile: impl Into<String>, store: Arc<dyn SecretStore>) -> Self {
        Self {
            paths,
            profile: profile.into(),
            store,
        }
    }

    /// The profile's name.
    pub fn profile(&self) -> &str {
        &self.profile
    }

    /// The store of its secrets, for tests that look into it.
    #[cfg(test)]
    pub(crate) fn store(&self) -> &dyn SecretStore {
        self.store.as_ref()
    }

    /// The same profile's files with its secrets in `store` instead.
    #[cfg(test)]
    pub(crate) fn with_store(&self, store: Arc<dyn SecretStore>) -> Self {
        Self {
            store,
            ..self.clone()
        }
    }

    fn lock(&self) -> Result<File> {
        self.lock_within(PROFILE_LOCK_TIMEOUT)
    }

    /// Takes the profile lock, giving up after `timeout`: a process that
    /// holds it, stopped with Ctrl-Z or waiting on an unanswered keychain
    /// prompt, must not stall every other one for good.
    fn lock_within(&self, timeout: Duration) -> Result<File> {
        let profile = self.profile.as_str();
        validate_profile_name(profile)?;
        let dir = self.paths.sessions_dir();
        std::fs::create_dir_all(&dir)?;
        set_unix_mode(&dir, PRIVATE_DIR_MODE)?;
        let path = self.paths.lock_file(profile);
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
    pub(crate) fn is_saved(&self) -> bool {
        validate_profile_name(&self.profile).is_ok()
            && self.paths.metadata_file(&self.profile).is_file()
    }

    /// Waits for whoever holds the profile lock, and lets it go at once. A
    /// token refresh holds it from sending the old refresh token until the
    /// new pair is saved. A profile never signed in to has no lock file,
    /// and nothing to wait for.
    pub(crate) fn wait_unlocked(&self) -> Result<()> {
        validate_profile_name(&self.profile)?;
        if !self.paths.lock_file(&self.profile).exists() {
            return Ok(());
        }
        self.lock().map(drop)
    }

    pub(crate) fn lock_and_load(&self) -> Result<(File, Option<LoadedSession>)> {
        let lock = self.lock()?;
        let loaded = self.load_unlocked()?;
        Ok((lock, loaded))
    }

    /// Saves `session` under the profile: its metadata in its file, and in
    /// the store the `tokens`, the key password `skp` and the API address the
    /// tokens belong to.
    pub fn save(&self, session: &Session, tokens: &Tokens, skp: &SecretString) -> Result<()> {
        let _lock = self.lock()?;
        // The previous login's secrets go before any of this one is written,
        // so a save that fails part way leaves no earlier tokens beside this
        // session's metadata and API address, where they would be sent.
        self.store.delete(K_TOKENS)?;
        self.store.delete(K_SKP)?;
        self.write_metadata(session)?;
        self.store.set(K_BASE_URL, &session.base_url)?;
        self.save_tokens(tokens)?;
        self.store.set(K_SKP, skp.expose_secret())?;
        Ok(())
    }

    /// Replaces the metadata file in one rename, so a write that fails part
    /// way leaves the previous file whole. The caller holds the profile lock.
    fn write_metadata(&self, session: &Session) -> Result<()> {
        let dir = self.paths.sessions_dir();
        std::fs::create_dir_all(&dir)?;
        set_unix_mode(&dir, PRIVATE_DIR_MODE)?;
        let file = self.paths.metadata_file(&self.profile);
        let temporary = self.paths.metadata_temp_file(&self.profile);
        let json = serde_json::to_vec_pretty(session)?;
        let written = std::fs::write(&temporary, json)
            .map_err(Error::from)
            .and_then(|()| set_unix_mode(&temporary, PRIVATE_FILE_MODE))
            .and_then(|()| std::fs::rename(&temporary, &file).map_err(Error::from));
        if written.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        written
    }

    /// Saves the pair of `tokens` a refresh issued, and nothing else of the
    /// session.
    pub fn save_tokens(&self, tokens: &Tokens) -> Result<()> {
        let pair = Zeroizing::new(serde_json::to_string(&StoredTokens {
            access: tokens.access.expose_secret(),
            refresh: tokens.refresh.expose_secret(),
        })?);
        self.store.set(K_TOKENS, &pair)?;
        Ok(())
    }

    /// The session saved under the profile. `None` when any part of it is
    /// missing, or its file holds no session.
    pub fn load(&self) -> Result<Option<LoadedSession>> {
        let (_lock, loaded) = self.lock_and_load()?;
        Ok(loaded)
    }

    /// The saved tokens under the profile lock, for a token refresh: it needs
    /// neither the key passphrase nor anything else from the store.
    pub(crate) fn lock_and_load_tokens(&self) -> Result<(File, Option<Tokens>)> {
        let lock = self.lock()?;
        let tokens = self.load_tokens_unlocked()?.map(|(_, tokens)| tokens);
        Ok((lock, tokens))
    }

    fn load_unlocked(&self) -> Result<Option<LoadedSession>> {
        let Some((session, tokens)) = self.load_tokens_unlocked()? else {
            return Ok(None);
        };
        let Some(skp) = self.store.get(K_SKP)? else {
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
    fn load_tokens_unlocked(&self) -> Result<Option<(Session, Tokens)>> {
        let Some(session) = self.read_metadata()? else {
            return Ok(None);
        };
        let Some(pair) = self.store.get(K_TOKENS)? else {
            return Ok(None);
        };
        let pair = Zeroizing::new(pair);
        let pair: LoadedTokens = serde_json::from_str(&pair)
            .map_err(|e| Error::Session(format!("invalid stored auth tokens: {e}")))?;
        // The metadata file is not secret, yet it names where the saved tokens
        // are sent. Trust only an address the secret store vouches for.
        if self.store.get(K_BASE_URL)?.as_deref() != Some(session.base_url.as_str()) {
            return Err(Error::SessionAddressMismatch);
        }
        let tokens = Tokens {
            uid: session.uid.clone(),
            access: SecretString::from(pair.access),
            refresh: SecretString::from(pair.refresh),
        };
        Ok(Some((session, tokens)))
    }

    /// Removes the session saved under the profile, its file and its secrets.
    pub fn clear(&self) -> Result<()> {
        let _lock = self.lock()?;
        self.clear_unlocked()
    }

    /// A pending logout must not remove a later login in the same profile.
    pub(crate) fn clear_if_current(&self, uid: &str) -> Result<()> {
        let _lock = self.lock()?;
        // Incomplete metadata cannot resume a session; still remove its secrets.
        if self
            .read_metadata()?
            .is_some_and(|session| session.uid != uid)
        {
            return Ok(());
        }
        self.clear_unlocked()
    }

    /// The profile's saved metadata. `None` when there is none, or when it
    /// is not a session: either way no session can resume from it. Any other
    /// failure to read it is an error.
    fn read_metadata(&self) -> Result<Option<Session>> {
        let bytes = match std::fs::read(self.paths.metadata_file(&self.profile)) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        Ok(serde_json::from_slice(&bytes).ok())
    }

    fn clear_unlocked(&self) -> Result<()> {
        let file = self.paths.metadata_file(&self.profile);
        // What an interrupted metadata write left behind goes too.
        let _ = std::fs::remove_file(self.paths.metadata_temp_file(&self.profile));
        let metadata = match std::fs::remove_file(&file) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Error::from(e)),
        };
        // Attempt every removal under the lock even if an earlier one fails.
        let tokens = self.store.delete(K_TOKENS);
        let skp = self.store.delete(K_SKP);
        let base_url = self.store.delete(K_BASE_URL);
        metadata.and(tokens).and(skp).and(base_url)
    }
}

#[cfg(test)]
mod tests;
