//! Signing in, resuming a saved session and signing out: how a [`Client`]
//! starts and ends, and the options and prompts that carry what it asks.

use super::{Client, SenderKeyCache, off_thread};
use crate::DEFAULT_BASE_URL;
use crate::api;
use crate::auth::{self, TotpPrompt};
use crate::cache::CacheIdentity;
use crate::crypto::{self, keys::KeyStore};
use crate::error::{Error, Result};
use crate::model::enums::password_mode;
use crate::session::{ProfileStorage, Session, Tokens};
use crate::transport::{Doer, HttpClient};
use secrecy::{ExposeSecret, SecretString};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

const DEFAULT_APP_VERSION: &str = "Other";

/// Bound the complete best-effort revocation, including retries and backoff.
pub(super) const LOGOUT_REVOCATION_TIMEOUT: Duration = Duration::from_secs(30);

/// What signing in to an account takes.
pub struct LoginOptions {
    /// The account's email address.
    pub username: String,
    /// The account's password.
    pub password: SecretString,
    /// The current code from the account's authenticator app, when it has
    /// one and it is known up front.
    pub totp: Option<SecretString>,
    /// The mailbox password of an account that keeps a separate one
    /// (`PasswordMode` 2).
    pub mailbox_password: Option<SecretString>,
    /// The local profile the session is saved under.
    pub profile: String,
    /// Another API address than Proton's own.
    pub base_url: Option<String>,
    /// The app version to present to the API, in place of the default.
    pub app_version: Option<String>,
    /// The `User-Agent` to present, so requests look like a given client's.
    pub user_agent: Option<String>,
    /// What solves a human-verification challenge (a CAPTCHA) when Proton
    /// answers with code 9001.
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
pub(super) async fn mailbox_password<'a>(
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

/// The account and its addresses, asked for together: neither request needs
/// the other, and every command that resumes a session waits for both.
pub(super) async fn account<D: Doer>(
    http: &D,
) -> Result<(crypto::keys::User, Vec<crypto::keys::Address>)> {
    tokio::try_join!(api::keys::get_user(http), api::keys::get_addresses(http))
}

/// A client for the API at `base_url` that presents itself as `app_version`
/// and, when there is one, `user_agent`: what signing in sets up, and what a
/// resumed session sets up again from what it saved.
async fn api_client(
    base_url: &str,
    app_version: &str,
    user_agent: Option<&str>,
) -> Result<HttpClient> {
    let http = HttpClient::try_new(base_url, app_version)?;
    if let Some(agent) = user_agent {
        http.set_user_agent(agent.to_owned()).await;
    }
    Ok(http)
}

impl Client {
    /// Shares the saved session with other processes: a refresh starts from
    /// the tokens saved last, under the profile lock, and saves the new ones.
    pub(crate) fn wire_refresh(http: &mut HttpClient, storage: &ProfileStorage) {
        let saved = storage.clone();
        http.set_refresh_load(Arc::new(move || saved.lock_and_load_tokens()));
        let saved = storage.clone();
        http.set_refresh_persist_fallible(Arc::new(move |tokens| saved.save_tokens(tokens)));
    }

    /// Signs in with what `opts` carries, second factor included, unlocks
    /// the account's keys, and saves the session under `opts.profile`.
    pub async fn login(opts: LoginOptions) -> Result<Client> {
        Self::login_inner(opts, LoginPrompts::default()).await
    }

    /// Like [`Client::login`], asking through `prompts` for what the account
    /// needs and `opts` does not carry: a TOTP code, asked for after any
    /// human verification so that it is fresh, and never for an account
    /// without TOTP; and a separate mailbox password once the account
    /// password is accepted.
    pub async fn login_with_prompts(opts: LoginOptions, prompts: LoginPrompts) -> Result<Client> {
        Self::login_inner(opts, prompts).await
    }

    pub(super) async fn login_inner(opts: LoginOptions, prompts: LoginPrompts) -> Result<Client> {
        let storage = ProfileStorage::system(&opts.profile)?;
        let base_url = opts
            .base_url
            .as_deref()
            .unwrap_or(DEFAULT_BASE_URL)
            .to_owned();
        let app_version = opts
            .app_version
            .as_deref()
            .unwrap_or(DEFAULT_APP_VERSION)
            .to_owned();
        let mut http = api_client(&base_url, &app_version, opts.user_agent.as_deref()).await?;
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
            storage.clone(),
        );
        let (keys, cache_identity) = auth::or_end_session(&http, opened).await?;

        Self::wire_refresh(&mut http, &storage);
        Ok(Client {
            http,
            keys,
            storage,
            session_uid: login.tokens.uid,
            cache_identity,
            sender_cache: Mutex::new(SenderKeyCache::default()),
        })
    }

    /// The rest of a login once Proton has accepted the password: unlocks the
    /// account's keys and saves the session.
    pub(super) async fn open_logged_in(
        http: &HttpClient,
        login: &auth::LoginResult,
        opts: &LoginOptions,
        prompts: &LoginPrompts,
        base_url: &str,
        app_version: String,
        storage: ProfileStorage,
    ) -> Result<(KeyStore, CacheIdentity)> {
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
            user_id,
        };
        let tokens = login.tokens.clone();
        off_thread(move || storage.save(&session, &tokens, &skp)).await?;
        Ok((keys, cache_identity))
    }

    /// Opens the session saved under `profile`, unlocking the account's keys
    /// with the key passphrase saved beside it.
    pub async fn resume(profile: &str) -> Result<Client> {
        Self::resume_from(ProfileStorage::system(profile)?).await
    }

    /// `resume`, with the session's files and secrets where the caller says.
    pub(super) async fn resume_from(storage: ProfileStorage) -> Result<Client> {
        tracing::info!(target: "ruston_core::mail", profile = storage.profile(), "resume: loading saved session");
        // The credential store may wait for the person to unlock it, and the
        // profile lock for another process.
        let saved = storage.clone();
        let loaded = off_thread(move || saved.load())
            .await?
            .ok_or(Error::Unauthorized)?;
        tracing::debug!(target: "ruston_core::mail", uid = %loaded.session.uid, "resume: session found; fetching user + addresses, unlocking keys");

        let mut http = api_client(
            &loaded.session.base_url,
            &loaded.session.app_version,
            loaded.session.user_agent.as_deref(),
        )
        .await?;
        let Tokens {
            uid,
            access,
            refresh,
        } = loaded.tokens;
        http.set_tokens(uid, access, refresh).await;
        Self::wire_refresh(&mut http, &storage);

        let (user, addresses) = account(&http).await?;
        let cache_identity = CacheIdentity::new(&loaded.session.base_url, &user.id)?;
        let skp = loaded.skp;
        let keys = off_thread(move || {
            crypto::keys::unlock_with_skp(&crypto::provider(), &user, &addresses, &skp)
        })
        .await?;

        Ok(Client {
            http,
            keys,
            storage,
            session_uid: loaded.session.uid,
            cache_identity,
            sender_cache: Mutex::new(SenderKeyCache::default()),
        })
    }

    /// Clear the saved session, then attempt remote revocation for up to 30 seconds.
    /// Local cleanup runs on a blocking worker and continues if this future is
    /// cancelled after it starts. Remote failures do not undo cleanup; local
    /// storage failures are returned to the caller. Returns whether Proton
    /// confirmed that the session ended.
    pub async fn logout(&self) -> Result<bool> {
        let storage = self.storage.clone();
        let session_uid = self.session_uid.clone();
        let cleanup = tokio::task::spawn_blocking(move || storage.clear_if_current(&session_uid))
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
        let storage = ProfileStorage::system(profile)?;
        tokio::task::spawn_blocking(move || storage.clear())
            .await
            .map_err(|error| Error::Session(format!("session cleanup task failed: {error}")))?
    }

    /// Whether `profile` has a saved session to resume, judged by its
    /// metadata file alone: the keychain is not asked, so the answer comes
    /// at once, and a session whose secrets are gone still counts.
    pub fn has_saved_session(profile: &str) -> bool {
        ProfileStorage::system(profile).is_ok_and(|storage| storage.is_saved())
    }

    /// Waits until no token refresh is under way, so a program about to end
    /// lets one finish saving the new tokens instead of losing the session.
    pub async fn settle(&self) {
        self.http.settled().await;
    }

    /// Waits until no token refresh of `profile`'s saved session is under
    /// way, in this program or another: one holds the profile lock from
    /// sending the old refresh token until the new pair is saved. A program
    /// stopped from outside, as by Ctrl+C, can then end at once, with no
    /// client at hand, and keep the session. Gives up with an error after as
    /// long as any other wait for that lock.
    pub async fn settle_session(profile: &str) -> Result<()> {
        let storage = ProfileStorage::system(profile)?;
        tokio::task::spawn_blocking(move || storage.wait_unlocked())
            .await
            .map_err(|error| Error::Session(format!("session wait task failed: {error}")))?
    }
}
