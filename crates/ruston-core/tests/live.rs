//! Checks on a real Proton account, which run only when the environment
//! names one with `PROTON_TEST_USER` and `PROTON_TEST_PASSWORD`, adding
//! `PROTON_TEST_TOTP` and `PROTON_TEST_MAILBOX_PASSWORD` when the account
//! asks for them. Otherwise each test returns at once, so the usual
//! `cargo test` run passes without an account. `PROTON_TEST_API_URL`,
//! `PROTON_TEST_APP_VERSION` and `PROTON_TEST_USER_AGENT` change where the
//! client goes and what it presents. The test that sends a message to the
//! account itself, and trashes it again, also needs `PROTON_TEST_SEND=1`.
//!
//! Every test signs in under a profile of its own and signs out before it
//! checks anything, so a failed check leaves no session or keychain entry
//! behind; what an interrupted run left is removed when the same test runs
//! again. One at a time, with their output:
//!
//! ```bash
//! PROTON_TEST_USER=you@proton.me PROTON_TEST_PASSWORD=... PROTON_TEST_SEND=1 \
//!   cargo test -p ruston-core --test live -- --nocapture --test-threads=1
//! ```

use ruston_core::{Client, Error, LoginOptions, SendOptions};
use secrecy::SecretString;

/// The account the tests sign in to, and how to reach it.
struct Account {
    username: String,
    password: String,
    totp: Option<String>,
    mailbox_password: Option<String>,
    api_url: Option<String>,
    app_version: Option<String>,
    user_agent: Option<String>,
}

impl Account {
    /// The account the environment names, if it names a user and a
    /// password; `test` is skipped without them, and says so.
    fn from_env(test: &str) -> Option<Self> {
        Self::named().or_else(|| {
            eprintln!(
                "skipping {test}: PROTON_TEST_USER and PROTON_TEST_PASSWORD are not both set"
            );
            None
        })
    }

    fn named() -> Option<Self> {
        let var = |name: &str| std::env::var(name).ok();
        Some(Self {
            username: var("PROTON_TEST_USER")?,
            password: var("PROTON_TEST_PASSWORD")?,
            totp: var("PROTON_TEST_TOTP"),
            mailbox_password: var("PROTON_TEST_MAILBOX_PASSWORD"),
            api_url: var("PROTON_TEST_API_URL"),
            app_version: var("PROTON_TEST_APP_VERSION"),
            user_agent: var("PROTON_TEST_USER_AGENT"),
        })
    }

    /// Signs in under `profile`, after removing what an interrupted run of
    /// the same test left saved there.
    async fn sign_in(&self, profile: &str) -> Client {
        Client::forget_session(profile)
            .await
            .expect("an earlier run's session should be removable");
        let secret = |text: &str| SecretString::from(text.to_owned());
        let options = LoginOptions {
            username: self.username.clone(),
            password: secret(&self.password),
            totp: self.totp.as_deref().map(secret),
            mailbox_password: self.mailbox_password.as_deref().map(secret),
            profile: profile.to_owned(),
            base_url: self.api_url.clone(),
            app_version: self.app_version.clone(),
            user_agent: self.user_agent.clone(),
            hv: None,
        };
        Client::login(options)
            .await
            .expect("the test account should sign in")
    }
}

/// Whether the environment lets a test send mail, which it trashes again.
fn sending_allowed() -> bool {
    std::env::var("PROTON_TEST_SEND").is_ok_and(|value| value == "1")
}

#[tokio::test]
async fn live_inbox_lists_and_its_newest_message_opens() {
    let Some(account) = Account::from_env("live_inbox_lists_and_its_newest_message_opens") else {
        return;
    };
    let client = account.sign_in("live-test-read").await;
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
async fn live_mail_sent_to_self_is_found_read_and_trashed() {
    let test = "live_mail_sent_to_self_is_found_read_and_trashed";
    let Some(account) = Account::from_env(test) else {
        return;
    };
    if !sending_allowed() {
        eprintln!("skipping {test}: set PROTON_TEST_SEND=1 to let it send mail");
        return;
    }
    let client = account.sign_in("live-test-send").await;
    let token = format!("ruston-core live test {}", std::process::id());

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
                body: format!("Sent to itself by a live test: {token}"),
                ..Default::default()
            })
            .await?;

        // The backend finds a message shortly after it is sent: search until
        // it does, or until a deadline.
        let search = ruston_core::SearchOpts {
            keyword: Some(token.clone()),
            folder: Some("all".into()),
            limit: Some(5),
            ..Default::default()
        };
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        let hits = loop {
            let hits = client.search_messages(&search).await?;
            if !hits.is_empty() || tokio::time::Instant::now() >= deadline {
                break hits;
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        };
        let body = match hits.first() {
            Some(hit) => Some(client.read_message(&hit.id).await?.body),
            None => None,
        };

        // Every copy found goes to Trash, so the account keeps none.
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
