//! Session model + on-disk persistence (non-secret metadata file + secrets in
//! the `SecretStore`).

pub mod secret_store;

pub use secret_store::{KeyringStore, MemoryStore, SecretStore};

use crate::error::{Error, Result};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

const K_TOKENS: &str = "auth_tokens_v1";
const K_SKP: &str = "skp";

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
    /// Password mode (1 = single-password, 2 = two-password login).
    pub password_mode: u8,
    /// Optional `User-Agent` recorded for this session.
    pub user_agent: Option<String>,
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
        let p = if profile.is_empty() {
            "default"
        } else {
            profile
        };
        self.sessions_dir().join(format!("{p}.json"))
    }
}

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
    /// Persist metadata to disk and secrets to the store.
    pub fn save(
        &self,
        paths: &Paths,
        profile: &str,
        store: &dyn SecretStore,
        tokens: &Tokens,
        skp: &SecretString,
    ) -> Result<()> {
        let dir = paths.sessions_dir();
        std::fs::create_dir_all(&dir)?;
        set_mode(&dir, 0o700)?;
        let file = paths.session_file(profile);
        let json = serde_json::to_vec_pretty(self)?;
        std::fs::write(&file, json)?;
        set_mode(&file, 0o600)?;

        Self::save_tokens(
            store,
            tokens.access.expose_secret(),
            tokens.refresh.expose_secret(),
        )?;
        store.set(K_SKP, skp.expose_secret())?;
        Ok(())
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
        let file = paths.session_file(profile);
        let bytes = match std::fs::read(&file) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let session: Session = match serde_json::from_slice(&bytes) {
            Ok(s) => s,
            Err(_) => return Ok(None),
        };
        let Some(pair) = store.get(K_TOKENS)? else {
            return Ok(None);
        };
        let pair = Zeroizing::new(pair);
        let pair: LoadedTokens = serde_json::from_str(&pair)
            .map_err(|e| Error::Session(format!("invalid stored auth tokens: {e}")))?;
        let skp = match store.get(K_SKP)? {
            Some(skp) => skp,
            None => return Ok(None),
        };
        let tokens = Tokens {
            uid: session.uid.clone(),
            access: SecretString::from(pair.access),
            refresh: SecretString::from(pair.refresh),
        };
        Ok(Some(LoadedSession {
            session,
            tokens,
            skp: SecretString::from(skp),
        }))
    }

    /// Remove the on-disk file and stored secrets.
    pub fn clear(paths: &Paths, profile: &str, store: &dyn SecretStore) -> Result<()> {
        let file = paths.session_file(profile);
        match std::fs::remove_file(&file) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        store.delete(K_TOKENS)?;
        store.delete(K_SKP)?;
        Ok(())
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
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "ruston-session-test-{}-{}",
            std::process::id(),
            nanos
        ))
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
    fn split_tokens_do_not_resume_a_session() {
        let paths = Paths::with_base(unique_base());
        let store = MemoryStore::new();
        let session = Session {
            uid: "UID1".into(),
            app_version: "Other".into(),
            base_url: "https://mail.proton.me/api".into(),
            password_mode: 1,
            user_agent: None,
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

    #[test]
    fn session_metadata_requires_password_mode() {
        let session = Session {
            uid: "UID1".into(),
            app_version: "Other".into(),
            base_url: "https://mail.proton.me/api".into(),
            password_mode: 1,
            user_agent: None,
        };
        let current = serde_json::to_value(session).unwrap();
        let mut incomplete = current;
        incomplete.as_object_mut().unwrap().remove("password_mode");
        assert!(serde_json::from_value::<Session>(incomplete).is_err());
    }
}
