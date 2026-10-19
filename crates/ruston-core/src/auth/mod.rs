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

/// Result of a successful login.
pub struct LoginResult {
    /// Session tokens (UID, access, refresh) for the authenticated session.
    pub tokens: Tokens,
    /// Password mode, one of [`password_mode`](crate::model::enums::password_mode).
    pub password_mode: u8,
}

/// Perform the full SRP login (+ 2FA if required).
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

    // 1. Unauthenticated session.
    tracing::debug!(target: "ruston_core::auth", "login step 1/6: creating unauthenticated session (POST /auth/v4/sessions)");
    let session: SessionResp = http
        .decode(
            Request::post("/auth/v4/sessions")
                .json(serde_json::json!({}))
                .enforce_unauth()
                .no_refresh(),
        )
        .await?;
    tracing::debug!(target: "ruston_core::auth", uid = %session.uid, "login: got unauth session");
    http.set_tokens(
        session.uid,
        SecretString::from(session.access_token),
        SecretString::from(session.refresh_token),
    )
    .await;

    // 2. SRP challenge.
    tracing::debug!(target: "ruston_core::auth", "login step 2/6: fetching SRP challenge (POST /core/v4/auth/info)");
    let info: AuthInfo = http
        .decode(
            Request::post("/core/v4/auth/info")
                .json(serde_json::json!({ "Username": username }))
                .no_refresh(),
        )
        .await?;
    tracing::debug!(target: "ruston_core::auth", srp_version = info.version, salt_len = info.salt.len(), modulus_len = info.modulus.len(), "login: received modulus, salt, server ephemeral");

    // 3. Compute SRP proofs (modulus signature verified internally by RPGPVerifier).
    let version = SrpHashVersion::try_from(info.version)
        .map_err(|e| Error::Srp(format!("unsupported SRP version {}: {e}", info.version)))?;
    tracing::debug!(target: "ruston_core::auth", "login step 3/6: verifying signed modulus + generating client proof (proton-srp)");
    let verifier = RPGPVerifier::default();
    let srp = SRPAuth::new(
        &verifier,
        Some(username),
        password.expose_secret(),
        version,
        &info.salt,
        &info.modulus,
        &info.server_ephemeral,
    )
    .map_err(|e| Error::Srp(format!("SRP setup failed: {e}")))?;
    let proof = srp
        .generate_proofs()
        .map_err(|e| Error::Srp(format!("SRP proof failed: {e}")))?;
    tracing::debug!(target: "ruston_core::auth", "login: client proof + ephemeral generated; modulus signature verified");
    let proofs: SRPProofB64 = proof.into();

    // 4. Submit proof.
    tracing::debug!(target: "ruston_core::auth", "login step 4/6: submitting client proof (POST /core/v4/auth)");
    let resp: AuthResponse = http
        .decode(
            Request::post("/core/v4/auth")
                .json(serde_json::json!({
                    "Username": username,
                    "ClientProof": proofs.client_proof,
                    "ClientEphemeral": proofs.client_ephemeral,
                    "SRPSession": info.srp_session,
                }))
                .no_refresh(),
        )
        .await?;

    // 5. Verify the server proof (MITM guard), then use the authenticated
    // session.
    if !proofs.compare_server_proof(&resp.server_proof) {
        tracing::error!(target: "ruston_core::auth", "login: SERVER PROOF MISMATCH — aborting (possible MITM)");
        return Err(Error::Srp("server proof verification failed".into()));
    }
    tracing::info!(target: "ruston_core::auth", uid = %resp.uid, password_mode = resp.password_mode, two_fa = resp.two_fa.enabled, "login step 5/6: server proof verified; authenticated");

    http.set_tokens(
        resp.uid.clone(),
        SecretString::from(resp.access_token.clone()),
        SecretString::from(resp.refresh_token.clone()),
    )
    .await;

    // 6. The second factor, if the account has one.
    tracing::debug!(target: "ruston_core::auth", "login step 6/6: second factor, if the account has one");
    or_end_session(
        http,
        second_factor(http, resp.two_fa.enabled, totp, totp_prompt),
    )
    .await?;
    tracing::info!(target: "ruston_core::auth", "login: complete");

    Ok(LoginResult {
        tokens: Tokens {
            uid: resp.uid,
            access: SecretString::from(resp.access_token),
            refresh: SecretString::from(resp.refresh_token),
        },
        password_mode: resp.password_mode,
    })
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
        return Err(Error::Other(
            "this account requires a security key (FIDO2/WebAuthn) for 2FA, which is not yet \
             supported — enable a TOTP authenticator app, or use an app/bridge password"
                .into(),
        ));
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

/// Revoke the current session server-side. An expired access token is
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn a_login_that_fails_after_the_password_ends_its_session() {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path("/core/v4/auth"))
            .and(header("authorization", "Bearer opened-access"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"Code": 1000})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let http = HttpClient::new(server.uri(), "Other");
        http.set_tokens(
            "opened-uid".into(),
            SecretString::from("opened-access"),
            SecretString::from("opened-refresh"),
        )
        .await;

        // An account whose only second factor is a security key cannot go on.
        let failed = or_end_session(&http, second_factor(&http, 2, None, None))
            .await
            .unwrap_err();
        // The login's own error is what comes back.
        assert!(failed.to_string().contains("FIDO2"), "{failed}");

        // A login that goes on keeps its session.
        or_end_session(&http, async { Ok(()) }).await.unwrap();
        server.verify().await;
    }

    #[tokio::test]
    async fn logout_calls_revoke() {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path("/core/v4/auth"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"Code": 1000})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let http = HttpClient::new(server.uri(), "Other");
        logout(&http).await.unwrap();
    }

    #[tokio::test]
    async fn logout_refreshes_an_expired_token_only_in_memory() {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path("/core/v4/auth"))
            .and(header("authorization", "Bearer expired-access"))
            .respond_with(
                ResponseTemplate::new(401).set_body_json(serde_json::json!({"Code": 401})),
            )
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
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"Code": 1000})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let mut http = HttpClient::new(server.uri(), "Other");
        let saved = Arc::new(AtomicUsize::new(0));
        let saves = saved.clone();
        http.set_refresh_persist_fallible(Arc::new(move |_, _, _| {
            saves.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }));
        http.set_tokens(
            "uid".into(),
            SecretString::from("expired-access"),
            SecretString::from("refresh-token"),
        )
        .await;

        logout(&http).await.unwrap();
        server.verify().await;
        assert_eq!(
            saved.load(Ordering::SeqCst),
            0,
            "the revoked tokens were saved"
        );
    }

    /// A prompt returning `code` and counting its invocations.
    fn counting_prompt(code: &'static str) -> (TotpPrompt, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let prompt: TotpPrompt = Arc::new(move || {
            seen.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move { Ok(SecretString::from(code)) })
        });
        (prompt, calls)
    }

    async fn expect_2fa(server: &MockServer, code: &str, times: u64) {
        Mock::given(method("POST"))
            .and(path("/core/v4/auth/2fa"))
            .and(body_json(serde_json::json!({ "TwoFactorCode": code })))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"Code": 1000})),
            )
            .expect(times)
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn second_factor_prefers_supplied_code() {
        let server = MockServer::start().await;
        expect_2fa(&server, "123456", 1).await;
        let http = HttpClient::new(server.uri(), "Other");
        let (prompt, calls) = counting_prompt("000000");
        second_factor(&http, 1, Some("123456"), Some(&prompt))
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn second_factor_prompts_when_code_missing() {
        let server = MockServer::start().await;
        expect_2fa(&server, "654321", 1).await;
        let http = HttpClient::new(server.uri(), "Other");
        let (prompt, calls) = counting_prompt("654321");
        second_factor(&http, 1, None, Some(&prompt)).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn second_factor_errors_without_code_or_prompt() {
        let server = MockServer::start().await;
        expect_2fa(&server, "", 0).await;
        let http = HttpClient::new(server.uri(), "Other");
        let err = second_factor(&http, 1, None, None).await.unwrap_err();
        assert!(err.to_string().contains("no TOTP code"));
    }

    #[tokio::test]
    async fn second_factor_rejects_fido2_only_without_prompting() {
        let server = MockServer::start().await;
        let http = HttpClient::new(server.uri(), "Other");
        let (prompt, calls) = counting_prompt("222222");
        let err = second_factor(&http, 2, None, Some(&prompt))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("FIDO2"));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn second_factor_uses_totp_when_a_security_key_is_set_up_too() {
        let server = MockServer::start().await;
        expect_2fa(&server, "123456", 1).await;
        let http = HttpClient::new(server.uri(), "Other");
        second_factor(&http, 3, Some("123456"), None).await.unwrap();
    }

    #[tokio::test]
    async fn second_factor_skipped_when_not_enabled() {
        let server = MockServer::start().await;
        let http = HttpClient::new(server.uri(), "Other");
        let (prompt, calls) = counting_prompt("111111");
        second_factor(&http, 0, None, Some(&prompt)).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(server.received_requests().await.unwrap().is_empty());
    }
}
