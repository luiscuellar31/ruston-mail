//! Live integration tests against a real Proton account.
//!
//! These are **skipped** unless `PROTON_TEST_USER` / `PROTON_TEST_PASSWORD`
//! are set, so they are safe to leave in the normal `cargo test` run. The
//! send/trash test additionally requires `PROTON_TEST_SEND=1` (it sends a real
//! message to yourself and trashes it).
//!
//! Each test signs in under a profile of its own and signs out before its
//! checks, so a failing check leaves no session or keychain entry behind. A
//! run that was interrupted is cleared the next time the same test starts.
//!
//! ```bash
//! PROTON_TEST_USER=you@proton.me PROTON_TEST_PASSWORD=... \
//!   PROTON_TEST_TOTP=123456 PROTON_TEST_SEND=1 \
//!   cargo test -p ruston-core --test live -- --nocapture --test-threads=1
//! ```

use ruston_core::{Client, Error, LoginOptions, SendOptions};
use secrecy::SecretString;

struct Creds {
    user: String,
    pass: String,
    totp: Option<String>,
    mailbox: Option<String>,
}

fn creds() -> Option<Creds> {
    let user = std::env::var("PROTON_TEST_USER").ok()?;
    let pass = std::env::var("PROTON_TEST_PASSWORD").ok()?;
    Some(Creds {
        user,
        pass,
        totp: std::env::var("PROTON_TEST_TOTP").ok(),
        mailbox: std::env::var("PROTON_TEST_MAILBOX_PASSWORD").ok(),
    })
}

/// Signs in under `profile`, first removing what an interrupted run of the
/// same test left saved there.
async fn login(c: &Creds, profile: &str) -> Client {
    Client::forget_session(profile)
        .await
        .expect("an earlier run's session should be removable");
    Client::login(LoginOptions {
        username: c.user.clone(),
        password: SecretString::from(c.pass.clone()),
        totp: c.totp.clone().map(SecretString::from),
        mailbox_password: c.mailbox.clone().map(SecretString::from),
        profile: profile.into(),
        base_url: std::env::var("PROTON_TEST_API_URL").ok(),
        app_version: std::env::var("PROTON_TEST_APP_VERSION").ok(),
        user_agent: std::env::var("PROTON_TEST_USER_AGENT").ok(),
        hv: None,
    })
    .await
    .expect("login should succeed")
}

#[tokio::test]
async fn live_login_and_read_inbox() {
    let Some(c) = creds() else {
        eprintln!("skipping live_login_and_read_inbox (PROTON_TEST_USER not set)");
        return;
    };
    let client = login(&c, "live-test-read").await;
    let primary = client.primary_email().map(str::to_owned);

    // Nothing in here panics, so the session is signed out whatever happens.
    let read = async {
        let (total, msgs) = client.list_messages("inbox", 0, 5, false).await?;
        eprintln!("inbox total={total}, fetched={}", msgs.len());

        if let Some(first) = msgs.first() {
            let full = client.read_message(&first.id).await?;
            eprintln!(
                "first message: subject={:?} verdict={:?} body_len={}",
                full.meta.subject,
                full.verdict,
                full.body.len()
            );
        }
        Ok::<_, Error>(())
    }
    .await;
    client.logout().await.expect("logout");

    read.expect("list the inbox and read+decrypt its first message");
    assert!(primary.is_some(), "should resolve a primary address");
}

#[tokio::test]
async fn live_send_read_trash_roundtrip() {
    let Some(c) = creds() else {
        eprintln!("skipping live_send_read_trash_roundtrip (PROTON_TEST_USER not set)");
        return;
    };
    if std::env::var("PROTON_TEST_SEND").ok().as_deref() != Some("1") {
        eprintln!("skipping send roundtrip (set PROTON_TEST_SEND=1 to enable)");
        return;
    }
    let client = login(&c, "live-test-send").await;
    let token = format!("proton-rs selftest {}", std::process::id());

    // Nothing in here panics, so the session is signed out whatever happens.
    let roundtrip = async {
        let me = client
            .primary_email()
            .ok_or_else(|| Error::Other("no primary address".into()))?
            .to_string();
        client
            .send(&SendOptions {
                to: vec![me],
                subject: token.clone(),
                body: format!("Automated round-trip test: {token}"),
                ..Default::default()
            })
            .await?;

        // Give the backend a moment, then find it via search.
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        let hits = client
            .search_messages(&ruston_core::SearchOpts {
                keyword: Some(token.clone()),
                folder: Some("all".into()),
                limit: Some(5),
                ..Default::default()
            })
            .await?;
        let body = match hits.first() {
            Some(hit) => Some(client.read_message(&hit.id).await?.body),
            None => None,
        };

        // Clean up: trash the found copies.
        let ids: Vec<String> = hits.iter().map(|m| m.id.clone()).collect();
        if !ids.is_empty() {
            client.trash_messages(&ids).await?;
        }
        eprintln!("round-trip done; trashed {} message(s)", ids.len());
        Ok::<_, Error>(body)
    }
    .await;
    client.logout().await.expect("logout");

    let body = roundtrip
        .expect("send to self, search, read back and trash")
        .expect("sent message should be findable");
    assert!(
        body.contains(&token),
        "decrypted body should contain the token"
    );
}
