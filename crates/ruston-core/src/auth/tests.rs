use super::test_support::MODULUS;
use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use proton_srp::{ServerClientProof, ServerClientVerifier, ServerInteraction};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

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
            let proof = ServerClientProof::new(&field("ClientEphemeral"), &field("ClientProof"));
            let Ok(server_proof) = proof.and_then(|proof| srp.lock().unwrap().verify_proof(&proof))
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
    let request =
        |path: &str, token: Option<&str>, body| (path.to_owned(), token.map(str::to_owned), body);
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
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"Code": 1000})))
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
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({"Code": 401})))
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
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"Code": 1000})))
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
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"Code": 1000})))
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
