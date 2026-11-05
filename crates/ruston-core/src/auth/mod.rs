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
    let proofs = answer_challenge(username, password, &challenge)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine, engine::general_purpose::STANDARD};
    use proton_srp::{ServerClientProof, ServerClientVerifier, ServerInteraction};
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // An SRP modulus signed with Proton's key, as the auth info hands it out.
    const MODULUS: &str = "-----BEGIN PGP SIGNED MESSAGE-----\nHash: SHA256\n\nW2z5HBi8RvsfYzZTS7qBaUxxPhsfHJFZpu3Kd6s1JafNrCCH9rfvPLrfuqocxWPgWDH2R8neK7PkNvjxto9TStuY5z7jAzWRvFWN9cQhAKkdWgy0JY6ywVn22+HFpF4cYesHrqFIKUPDMSSIlWjBVmEJZ/MusD44ZT29xcPrOqeZvwtCffKtGAIjLYPZIEbZKnDM1Dm3q2K/xS5h+xdhjnndhsrkwm9U9oyA2wxzSXFL+pdfj2fOdRwuR5nW0J2NFrq3kJjkRmpO/Genq1UW+TEknIWAb6VzJJJA244K/H8cnSx2+nSNZO3bbo6Ys228ruV9A8m6DhxmS+bihN3ttQ==\n-----BEGIN PGP SIGNATURE-----\nVersion: ProtonMail\nComment: https://protonmail.com\n\nwl4EARYIABAFAlwB1j0JEDUFhcTpUY8mAAD8CgEAnsFnF4cF0uSHKkXa1GIa\nGO86yMV4zDZEZcDSJo0fgr8A/AlupGN9EdHlsrZLmTA1vhIx+rOgxdEff28N\nkvNM7qIK\n=q6vu\n-----END PGP SIGNATURE-----";

    /// A success answer carrying `fields`.
    fn answer(mut fields: serde_json::Value) -> ResponseTemplate {
        fields["Code"] = json!(1000);
        ResponseTemplate::new(200).set_body_json(fields)
    }

    /// Mounts Proton's side of a sign-in with `password`: a session opened
    /// for no one, the SRP challenge, and a session for a client that proves
    /// the password, sent with the server's own proof, or with a wrong one
    /// unless `proves_itself`. The account asks for a TOTP code.
    async fn srp_server(server: &MockServer, password: &str, proves_itself: bool) {
        let verifier =
            SRPAuth::generate_verifier(&RPGPVerifier::default(), password, None, MODULUS).unwrap();
        let mut srp = ServerInteraction::new_with_modulus_extractor(
            &RPGPVerifier::default(),
            MODULUS,
            &ServerClientVerifier::from(&verifier),
        )
        .unwrap();
        let challenge = json!({
            "Version": 4, "Modulus": MODULUS, "ServerEphemeral": srp.generate_challenge().encode_b64(),
            "Salt": STANDARD.encode(verifier.salt), "SRPSession": "srp-session"
        });
        let srp = std::sync::Mutex::new(srp);
        let opened = json!({
            "UID": "unauthenticated-uid", "AccessToken": "unauthenticated-access",
            "RefreshToken": "unauthenticated-refresh"
        });
        for (route, answered) in [
            ("/auth/v4/sessions", answer(opened)),
            ("/core/v4/auth/info", answer(challenge)),
            ("/core/v4/auth/2fa", answer(json!({}))),
        ] {
            let mock = Mock::given(method("POST")).and(path(route));
            mock.respond_with(answered).mount(server).await;
        }
        Mock::given(method("POST"))
            .and(path("/core/v4/auth"))
            .respond_with(move |request: &wiremock::Request| {
                let sent: serde_json::Value = request.body_json().unwrap();
                let field = |name: &str| sent[name].as_str().unwrap_or_default().to_owned();
                let proof =
                    ServerClientProof::new(&field("ClientEphemeral"), &field("ClientProof"));
                let Ok(server_proof) =
                    proof.and_then(|proof| srp.lock().unwrap().verify_proof(&proof))
                else {
                    return ResponseTemplate::new(422).set_body_json(json!({ "Code": 8002 }));
                };
                let server_proof = if proves_itself {
                    server_proof.encode_b64()
                } else {
                    STANDARD.encode([0u8; 256])
                };
                answer(json!({
                    "UID": "uid", "AccessToken": "access", "RefreshToken": "refresh",
                    "ServerProof": server_proof, "2FA": { "Enabled": 1 }, "PasswordMode": 2
                }))
            })
            .mount(server)
            .await;
    }

    /// What `server` received, in order: each request's path, the access
    /// token it carried, and its JSON body, in which the SRP proofs, new on
    /// every sign-in, read `"proof"`.
    async fn received(server: &MockServer) -> Vec<(String, Option<String>, serde_json::Value)> {
        let requests = server.received_requests().await.unwrap();
        let read = |request: &wiremock::Request| {
            let token = request.headers.get("authorization");
            let token = token.map(|token| token.to_str().unwrap().to_owned());
            let mut body: serde_json::Value = request.body_json().unwrap();
            for proof in ["ClientEphemeral", "ClientProof"] {
                if let Some(value) = body.get_mut(proof) {
                    *value = json!("proof");
                }
            }
            (request.url.path().to_owned(), token, body)
        };
        requests.iter().map(read).collect()
    }

    #[tokio::test]
    async fn signing_in_proves_the_password_then_the_server_in_a_session_of_its_own() {
        let server = MockServer::start().await;
        srp_server(&server, "the password", true).await;
        let http = HttpClient::new(server.uri(), "Other");

        let password = SecretString::from("the password");
        let signed_in = login(&http, "user", &password, Some("123456"));
        let signed_in = signed_in.await.unwrap();

        let tokens = &signed_in.tokens;
        let issued = [&tokens.access, &tokens.refresh].map(|token| token.expose_secret());
        assert_eq!(
            (tokens.uid.as_str(), issued),
            ("uid", ["access", "refresh"])
        );
        assert_eq!(signed_in.password_mode, 2);
        // The session is opened for no one; the challenge is fetched and
        // answered in it; the code goes to the session the answer opened.
        let request = |path: &str, token: Option<&str>, body| {
            (path.to_owned(), token.map(str::to_owned), body)
        };
        let unauthenticated = Some("Bearer unauthenticated-access");
        let proofs = json!({
            "Username": "user", "ClientProof": "proof", "ClientEphemeral": "proof",
            "SRPSession": "srp-session"
        });
        let code = json!({ "TwoFactorCode": "123456" });
        assert_eq!(
            received(&server).await,
            [
                request("/auth/v4/sessions", None, json!({})),
                request(
                    "/core/v4/auth/info",
                    unauthenticated,
                    json!({ "Username": "user" })
                ),
                request("/core/v4/auth", unauthenticated, proofs),
                request("/core/v4/auth/2fa", Some("Bearer access"), code),
            ]
        );
    }

    #[tokio::test]
    async fn a_server_that_cannot_prove_the_password_is_left_before_the_second_factor() {
        let server = MockServer::start().await;
        srp_server(&server, "the password", false).await;
        let http = HttpClient::new(server.uri(), "Other");

        let password = SecretString::from("the password");
        let refused = login(&http, "user", &password, Some("123456")).await;

        assert!(matches!(
            refused,
            Err(Error::Srp(message)) if message == "server proof verification failed"
        ));
        let routes: Vec<String> = received(&server)
            .await
            .into_iter()
            .map(|sent| sent.0)
            .collect();
        assert_eq!(
            routes,
            ["/auth/v4/sessions", "/core/v4/auth/info", "/core/v4/auth"]
        );
    }

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
    async fn logging_out_revokes_the_session_the_client_is_signed_in_to() {
        let server = MockServer::start().await;
        let signed_in = HttpClient::new(server.uri(), "Other");
        signed_in
            .set_tokens(
                "uid".into(),
                SecretString::from("access"),
                SecretString::from("refresh"),
            )
            .await;
        Mock::given(method("DELETE"))
            .and(path("/core/v4/auth"))
            .and(header("x-pm-uid", "uid"))
            .and(header("authorization", "Bearer access"))
            .respond_with(answer(json!({})))
            .expect(1)
            .mount(&server)
            .await;

        logout(&signed_in).await.unwrap();
        server.verify().await;
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
        http.set_refresh_persist_fallible(Arc::new(move |_| {
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
