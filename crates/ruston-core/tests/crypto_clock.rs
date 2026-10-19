//! OpenPGP runs at Proton's time once its API has said what that is. The
//! clock is process-wide, so this test has a process of its own.

use std::time::{SystemTime, UNIX_EPOCH};

use proton_crypto::crypto::{
    DataEncoding, KeyGenerator, KeyGeneratorSync, PGPProviderSync, Signer, SignerSync,
    UnixTimestamp, Verifier, VerifierSync,
};
use ruston_core::transport::{Doer, HttpClient, Request};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

const HOUR: u64 = 60 * 60;

fn local_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[tokio::test]
async fn signatures_are_dated_and_checked_at_protons_time() {
    let provider = ruston_core::crypto::provider();
    let key = provider
        .new_key_generator()
        .with_user_id("Alex", "alex@example.com")
        .generate()
        .unwrap();
    let public = provider.private_key_to_public_key(&key).unwrap();
    // Signed by a correct clock, an hour ahead of this machine's.
    let signature = provider
        .new_signer()
        .with_signing_key(&key)
        .at_signing_time(UnixTimestamp::new(local_now() + HOUR))
        .sign_detached(b"hello", DataEncoding::Bytes)
        .unwrap();
    let verify = |signature: &[u8]| {
        provider
            .new_verifier()
            .with_verification_key(&public)
            .verify_detached(b"hello", signature, DataEncoding::Bytes)
    };
    assert!(
        verify(&signature).is_err(),
        "a signature made in this machine's future passed"
    );

    let server = MockServer::start().await;
    let proton_now = chrono::Utc::now() + chrono::Duration::hours(1);
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header(
                    "Date",
                    proton_now
                        .format("%a, %d %b %Y %H:%M:%S GMT")
                        .to_string()
                        .as_str(),
                )
                .set_body_json(serde_json::json!({"Code": 1000})),
        )
        .mount(&server)
        .await;
    HttpClient::new(server.uri(), "Other")
        .do_raw(Request::get("/core/v4/tests/ping"))
        .await
        .unwrap();

    assert!(
        verify(&signature).is_ok(),
        "a signature made at Proton's time did not pass"
    );
    let ours = provider
        .new_signer()
        .with_signing_key(&key)
        .sign_detached(b"hello", DataEncoding::Bytes)
        .unwrap();
    let dated = verify(&ours).unwrap().signature_creation_time.0;
    assert!(
        dated + 60 >= local_now() + HOUR,
        "our signature is dated {dated}, not at Proton's time"
    );
}
