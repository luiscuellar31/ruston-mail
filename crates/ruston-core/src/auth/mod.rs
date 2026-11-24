//! SRP login, the second factor, and logout. Renewing tokens is the
//! transport's.

pub mod types;

use crate::error::{Error, Result};
use crate::model::enums::two_factor;
use crate::session::Tokens;
use crate::transport::{Doer, HttpClient, Request};
use futures::future::BoxFuture;
use proton_srp::{RPGPVerifier, SRPAuth, SRPProofB64, SrpHashVersion};
use secrecy::{ExposeSecret, SecretString};
use serde::de::IgnoredAny;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

/// How long a failed login waits for Proton to end the session it opened,
/// before reporting its own error.
const UNUSED_SESSION_REVOCATION_TIMEOUT: Duration = Duration::from_secs(10);
use types::{AuthInfo, AuthResponse, SessionResp};

/// Asks the user for a TOTP code during login.
///
/// Called only when the account requires TOTP and no code was supplied up
/// front, after any human verification, so the code is still fresh. Obtain the
/// code interactively and do not persist it.
pub type TotpPrompt = Arc<dyn Fn() -> BoxFuture<'static, Result<SecretString>> + Send + Sync>;

/// What a sign-in that went through leaves: the session, and how the
/// account's passwords are set up.
pub struct LoginResult {
    /// The signed-in session's ID and tokens.
    pub tokens: Tokens,
    /// Password mode, one of [`password_mode`](crate::model::enums::password_mode).
    pub password_mode: u8,
}

/// Signs in with SRP, then with the second factor when the account has
/// one, using `totp` as the code.
pub async fn login(
    http: &HttpClient,
    username: &str,
    password: &SecretString,
    totp: Option<&str>,
) -> Result<LoginResult> {
    login_with_prompt(http, username, password, totp, None).await
}

/// Like [`login`], but asks `totp_prompt` for the code when the account
/// requires TOTP and `totp` is `None`.
pub async fn login_with_prompt(
    http: &HttpClient,
    username: &str,
    password: &SecretString,
    totp: Option<&str>,
    totp_prompt: Option<&TotpPrompt>,
) -> Result<LoginResult> {
    tracing::info!(target: "ruston_core::auth", username, "login: starting SRP flow");
    open_unauthenticated_session(http).await?;
    let challenge = srp_challenge(http, username).await?;
    // Hashing the password takes a tenth of a second or more: work for the
    // blocking pool, as unlocking the keys is, not for an async worker.
    let (proofs, challenge) = {
        let (username, password) = (username.to_owned(), password.clone());
        crate::mail::off_thread(move || {
            let proofs = answer_challenge(&username, &password, &challenge)?;
            Ok((proofs, challenge))
        })
        .await?
    };
    let signed_in = exchange_proofs(http, username, &proofs, &challenge.srp_session).await?;

    // The tokens become secrets at once, and the client holds copies of those.
    let tokens = Tokens {
        uid: signed_in.uid,
        access: SecretString::from(signed_in.access_token),
        refresh: SecretString::from(signed_in.refresh_token),
    };
    http.set_tokens(
        tokens.uid.clone(),
        tokens.access.clone(),
        tokens.refresh.clone(),
    )
    .await;

    tracing::debug!(target: "ruston_core::auth", "login step 6/6: second factor, if the account has one");
    let second = second_factor(http, signed_in.two_fa.enabled, totp, totp_prompt);
    or_end_session(http, second).await?;
    tracing::info!(target: "ruston_core::auth", "login: complete");
    Ok(LoginResult {
        tokens,
        password_mode: signed_in.password_mode,
    })
}

/// Opens a session that belongs to no one yet, and has the client use it:
/// Proton answers the sign-in requests that follow only within a session.
async fn open_unauthenticated_session(http: &HttpClient) -> Result<()> {
    tracing::debug!(target: "ruston_core::auth", "login step 1/6: creating unauthenticated session (POST /auth/v4/sessions)");
    let request = Request::post("/auth/v4/sessions")
        .json(serde_json::json!({}))
        .enforce_unauth()
        .no_refresh();
    let opened: SessionResp = http.decode(request).await?;
    tracing::debug!(target: "ruston_core::auth", uid = %opened.uid, "login: got unauth session");
    let access = SecretString::from(opened.access_token);
    let refresh = SecretString::from(opened.refresh_token);
    http.set_tokens(opened.uid, access, refresh).await;
    Ok(())
}

/// The SRP challenge Proton sets for `username`: the signed modulus, the
/// salt, and the server's ephemeral value.
async fn srp_challenge(http: &HttpClient, username: &str) -> Result<AuthInfo> {
    tracing::debug!(target: "ruston_core::auth", "login step 2/6: fetching SRP challenge (POST /core/v4/auth/info)");
    let request = Request::post("/core/v4/auth/info")
        .json(serde_json::json!({ "Username": username }))
        .no_refresh();
    let challenge: AuthInfo = http.decode(request).await?;
    tracing::debug!(target: "ruston_core::auth", srp_version = challenge.version, salt_len = challenge.salt.len(), modulus_len = challenge.modulus.len(), "login: received modulus, salt, server ephemeral");
    Ok(challenge)
}

/// The client's proofs that it knows `password`, worked out for
/// `challenge` once its modulus is found to be signed with Proton's key.
fn answer_challenge(
    username: &str,
    password: &SecretString,
    challenge: &AuthInfo,
) -> Result<SRPProofB64> {
    let version = SrpHashVersion::try_from(challenge.version).map_err(|e| {
        Error::Srp(format!(
            "unsupported SRP version {}: {e}",
            challenge.version
        ))
    })?;
    tracing::debug!(target: "ruston_core::auth", "login step 3/6: verifying signed modulus + generating client proof (proton-srp)");
    let proofs = SRPAuth::new(
        &RPGPVerifier::default(),
        Some(username),
        password.expose_secret(),
        version,
        &challenge.salt,
        &challenge.modulus,
        &challenge.server_ephemeral,
    )
    .map_err(|e| Error::Srp(format!("SRP setup failed: {e}")))?
    .generate_proofs()
    .map_err(|e| Error::Srp(format!("SRP proof failed: {e}")))?;
    tracing::debug!(target: "ruston_core::auth", "login: client proof + ephemeral generated; modulus signature verified");
    Ok(SRPProofB64::from(proofs))
}

/// Sends Proton the client's `proofs`, and checks the server's proof that
/// comes back: only a server that holds the password's verifier can make
/// it, so a wrong one ends the sign-in. Returns the session the proofs
/// opened.
async fn exchange_proofs(
    http: &HttpClient,
    username: &str,
    proofs: &SRPProofB64,
    srp_session: &str,
) -> Result<AuthResponse> {
    tracing::debug!(target: "ruston_core::auth", "login step 4/6: submitting client proof (POST /core/v4/auth)");
    let request = Request::post("/core/v4/auth")
        .json(serde_json::json!({
            "Username": username,
            "ClientProof": proofs.client_proof,
            "ClientEphemeral": proofs.client_ephemeral,
            "SRPSession": srp_session,
        }))
        .no_refresh();
    let signed_in: AuthResponse = http.decode(request).await?;
    if proofs.compare_server_proof(&signed_in.server_proof) {
        tracing::info!(target: "ruston_core::auth", uid = %signed_in.uid, password_mode = signed_in.password_mode, two_fa = signed_in.two_fa.enabled, "login step 5/6: server proof verified; authenticated");
        Ok(signed_in)
    } else {
        tracing::error!(target: "ruston_core::auth", "login: SERVER PROOF MISMATCH — aborting (possible MITM)");
        Err(Error::Srp("server proof verification failed".into()))
    }
}

/// Submit the second factor if the account requires one. `enabled` is the 2FA
/// bitmask of [`two_factor`] bits.
async fn second_factor(
    http: &HttpClient,
    enabled: u32,
    totp: Option<&str>,
    totp_prompt: Option<&TotpPrompt>,
) -> Result<()> {
    if enabled & two_factor::TOTP == 0 && enabled & two_factor::SECURITY_KEY != 0 {
        return Err(Error::SecurityKeyRequired);
    }
    if enabled & two_factor::TOTP == 0 {
        return Ok(());
    }
    let prompted_code;
    let code = match (totp, totp_prompt) {
        (Some(code), _) => code,
        (None, Some(prompt)) => {
            prompted_code = prompt().await?;
            prompted_code.expose_secret()
        }
        (None, None) => {
            return Err(Error::Other(
                "account requires 2FA but no TOTP code was provided".into(),
            ));
        }
    };
    tracing::debug!(target: "ruston_core::auth", "login: 2FA required — submitting TOTP (POST /core/v4/auth/2fa)");
    let _: IgnoredAny = http
        .decode(
            Request::post("/core/v4/auth/2fa")
                .json(serde_json::json!({ "TwoFactorCode": code }))
                .no_refresh(),
        )
        .await?;
    tracing::debug!(target: "ruston_core::auth", "login: 2FA accepted");
    Ok(())
}

/// Has Proton end the client's session. An expired access token is
/// refreshed first, in memory only: revoking never saves tokens again.
pub async fn logout(http: &HttpClient) -> Result<()> {
    let revoking = http.detached().await;
    let _: IgnoredAny = revoking.decode(Request::delete("/core/v4/auth")).await?;
    Ok(())
}

/// Runs `work`, the rest of a login once Proton has accepted the password.
/// If it fails, the session Proton opened is ended, best effort and briefly:
/// nothing would use or save it, yet it would stay valid on the account. A
/// login cancelled meanwhile leaves the session to expire.
pub(crate) async fn or_end_session<T>(
    http: &HttpClient,
    work: impl Future<Output = Result<T>>,
) -> Result<T> {
    let result = work.await;
    if result.is_err() {
        let ended = tokio::time::timeout(UNUSED_SESSION_REVOCATION_TIMEOUT, logout(http)).await;
        tracing::debug!(target: "ruston_core::auth", ended = matches!(ended, Ok(Ok(()))), "login failed after the password was accepted; ending its session");
    }
    result
}

/// What the tests of sign-in and of other SRP users share.
#[cfg(test)]
pub(crate) mod test_support {
    /// An SRP modulus signed with Proton's key, as Proton hands one out,
    /// from proton-srp's own tests.
    pub(crate) const MODULUS: &str = "-----BEGIN PGP SIGNED MESSAGE-----\nHash: SHA256\n\nW2z5HBi8RvsfYzZTS7qBaUxxPhsfHJFZpu3Kd6s1JafNrCCH9rfvPLrfuqocxWPgWDH2R8neK7PkNvjxto9TStuY5z7jAzWRvFWN9cQhAKkdWgy0JY6ywVn22+HFpF4cYesHrqFIKUPDMSSIlWjBVmEJZ/MusD44ZT29xcPrOqeZvwtCffKtGAIjLYPZIEbZKnDM1Dm3q2K/xS5h+xdhjnndhsrkwm9U9oyA2wxzSXFL+pdfj2fOdRwuR5nW0J2NFrq3kJjkRmpO/Genq1UW+TEknIWAb6VzJJJA244K/H8cnSx2+nSNZO3bbo6Ys228ruV9A8m6DhxmS+bihN3ttQ==\n-----BEGIN PGP SIGNATURE-----\nVersion: ProtonMail\nComment: https://protonmail.com\n\nwl4EARYIABAFAlwB1j0JEDUFhcTpUY8mAAD8CgEAnsFnF4cF0uSHKkXa1GIa\nGO86yMV4zDZEZcDSJo0fgr8A/AlupGN9EdHlsrZLmTA1vhIx+rOgxdEff28N\nkvNM7qIK\n=q6vu\n-----END PGP SIGNATURE-----";
}

#[cfg(test)]
mod tests;
