use ruston_core::ApiError;
use ruston_core::model::ConversationLabel;

use super::*;

#[tokio::test(start_paused = true)]
async fn the_sign_in_deadline_leaves_out_the_time_spent_on_prompts() {
    let prompts = PromptTime::default();
    // A CAPTCHA that takes ten minutes between two quick requests.
    let sign_in = async {
        tokio::time::sleep(Duration::from_secs(30)).await;
        {
            let _answering = prompts.start();
            tokio::time::sleep(Duration::from_secs(600)).await;
        }
        tokio::time::sleep(Duration::from_secs(30)).await;
        "signed in"
    };

    assert_eq!(
        within(SIGN_IN_TIMEOUT, &prompts, sign_in).await,
        Some("signed in")
    );
}

#[tokio::test(start_paused = true)]
async fn a_sign_in_runs_out_of_time_once_its_own_time_is_used_up() {
    let prompts = PromptTime::default();
    let sign_in = async {
        {
            let _answering = prompts.start();
            tokio::time::sleep(Duration::from_secs(600)).await;
        }
        // Proton stops answering once the prompt is solved.
        std::future::pending::<()>().await;
    };
    let started = tokio::time::Instant::now();

    assert_eq!(within(SIGN_IN_TIMEOUT, &prompts, sign_in).await, None);
    assert_eq!(
        started.elapsed(),
        Duration::from_secs(600) + SIGN_IN_TIMEOUT
    );

    // Without a prompt, the whole deadline is the sign-in's own.
    let started = tokio::time::Instant::now();
    let silent = std::future::pending::<()>();
    assert_eq!(
        within(SIGN_IN_TIMEOUT, &PromptTime::default(), silent).await,
        None
    );
    assert_eq!(started.elapsed(), SIGN_IN_TIMEOUT);
}

#[tokio::test(start_paused = true)]
async fn recipients_are_looked_up_together_within_one_deadline() {
    let addresses: Vec<String> = (0..8).map(|n| format!("person{n}@proton.me")).collect();

    // Each lookup takes ten seconds: one after the other, eighty.
    let started = tokio::time::Instant::now();
    let checks = protection_of(&addresses, |_| async {
        tokio::time::sleep(Duration::from_secs(10)).await;
        Ok(RecipientProtection::EndToEnd)
    })
    .await;
    assert_eq!(started.elapsed(), Duration::from_secs(20));
    let found: Vec<ProtectionCheck> = addresses
        .iter()
        .map(|address| (address.clone(), Ok(Protection::EndToEnd)))
        .collect();
    assert_eq!(checks, found);

    // Lookups Proton never answers all end with the batch's one deadline.
    let started = tokio::time::Instant::now();
    let checks = protection_of(&addresses, |_| std::future::pending()).await;
    assert_eq!(started.elapsed(), REQUEST_TIMEOUT);
    let unanswered: Vec<ProtectionCheck> = addresses
        .iter()
        .map(|address| (address.clone(), Err(MailboxError::Connection)))
        .collect();
    assert_eq!(checks, unanswered);

    // A failed lookup counts for its own address only.
    let checks = protection_of(&addresses[..3], |address| async move {
        if address == "person1@proton.me" {
            Err(Error::Other("refused".into()))
        } else {
            Ok(RecipientProtection::Pgp)
        }
    })
    .await;
    assert_eq!(checks[0], (addresses[0].clone(), Ok(Protection::Pgp)));
    assert!(checks[1].1.is_err());
    assert_eq!(checks[2], (addresses[2].clone(), Ok(Protection::Pgp)));
}

#[tokio::test]
async fn inspection_deadline_includes_waiting_for_a_permit() {
    let limiter = Semaphore::new(1);
    let _permit = limiter.acquire().await.unwrap();
    let called = AtomicBool::new(false);
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        limited_inspection(&limiter, Duration::from_millis(20), async {
            called.store(true, Ordering::SeqCst);
            Ok(())
        }),
    )
    .await
    .expect("admission must also have a deadline");
    assert_eq!(result, Err(MailboxError::Unavailable));
    assert!(!called.load(Ordering::SeqCst));
}

#[tokio::test]
async fn cancelled_inspection_releases_request_and_permit() {
    use futures::FutureExt;

    let limiter = Semaphore::new(1);
    let client = Arc::new(());
    let weak = Arc::downgrade(&client);
    let mut inspection = Box::pin(limited_inspection(
        &limiter,
        INSPECTION_BUDGET,
        async move {
            let _client = client;
            std::future::pending::<ruston_core::Result<()>>().await
        },
    ));
    assert!(inspection.as_mut().now_or_never().is_none());
    assert_eq!(limiter.available_permits(), 0);
    drop(inspection);
    assert_eq!(limiter.available_permits(), 1);
    assert!(weak.upgrade().is_none());
}

#[tokio::test]
async fn inspection_preserves_expired_session_errors() {
    let limiter = Semaphore::new(1);
    let result = limited_inspection(&limiter, INSPECTION_BUDGET, async {
        Err::<(), _>(Error::Unauthorized)
    })
    .await;
    assert_eq!(result, Err(MailboxError::SessionExpired));
    assert_eq!(limiter.available_permits(), 1);
}

#[test]
fn a_message_on_its_own_that_cannot_be_opened_shows_as_in_a_conversation() {
    let unreadable = UnreadableMessage {
        meta: MessageMetadata {
            id: "m1".into(),
            subject: " Invoice ".into(),
            label_ids: vec!["0".into()],
            sender: person("Boss", "boss@corp.example"),
            ..Default::default()
        },
        reply_tos: Vec::new(),
        error: Error::Crypto("no key of the account opens it".into()),
    };

    let detail = single_message_detail(
        "m1",
        ConversationMessage::Unreadable(unreadable),
        &OwnAddresses::new(),
    );

    assert_eq!(detail.id, "m1");
    assert_eq!(detail.subject.as_deref(), Some("Invoice"));
    assert_eq!(detail.labels, ["0"]);
    let [message] = &detail.messages[..] else {
        panic!("the message itself");
    };
    assert_eq!(message.id, "m1");
    assert!(matches!(message.body, MessageBody::Unreadable));
}

fn person(name: &str, address: &str) -> Recipient {
    Recipient {
        name: name.into(),
        address: address.into(),
        contact_id: None,
        is_proton: None,
    }
}

fn label(id: &str) -> ConversationLabel {
    ConversationLabel {
        id: id.into(),
        ..Default::default()
    }
}

fn api_error(http_status: u16) -> Error {
    Error::Api(ApiError {
        http_status,
        code: 0,
        message: String::new(),
        raw_body: String::new(),
    })
}

#[test]
fn decrypted_messages_map_to_reader_models() {
    let meta = MessageMetadata {
        id: "message".into(),
        sender: person("", "alex@example.com"),
        to_list: vec![person("Demo", "demo@example.com")],
        cc_list: vec![person("", "team@example.org")],
        bcc_list: vec![person("", "archive@example.net")],
        time: 1_700_000_000,
        ..Default::default()
    };

    let own = OwnAddresses::from(["demo@example.com".to_owned()]);
    let message = mail_message(
        FullMessage {
            meta,
            body: "<p>Hello &amp; welcome</p>".into(),
            mime_type: "text/html".into(),
            attachments: Vec::new(),
            verdict: ruston_core::Verdict::Invalid,
            reply_tos: vec![person("", "replies@example.com")],
        },
        &own,
    );

    assert_eq!(message.id, "message");
    assert_eq!(message.verdict, ruston_core::Verdict::Invalid);
    assert_eq!(message.sender.name, None);
    assert_eq!(message.sender.address, "alex@example.com");
    // To, Cc and Bcc stay apart, so the reader can say which is which.
    let addresses = |people: &[MailAddress]| -> Vec<String> {
        people.iter().map(|person| person.address.clone()).collect()
    };
    assert_eq!(addresses(&message.to), ["demo@example.com"]);
    assert_eq!(addresses(&message.cc), ["team@example.org"]);
    assert_eq!(addresses(&message.bcc), ["archive@example.net"]);
    // The reader knows where core will send a reply before it is written.
    assert_eq!(message.reply_to, ["replies@example.com"]);
    assert_eq!(message.reply_all_cc, ["team@example.org"]);
    assert!(message.reply_redirected, "Reply-To differs from the sender");
    assert_eq!(message.time, Some(1_700_000_000));
    let MessageBody::Rich(body) = &message.body else {
        panic!("expected a rich body for HTML");
    };
    assert_eq!(body.plain_text(), "Hello & welcome\n");
}

#[test]
fn plain_text_bodies_are_kept_verbatim() {
    let body = "Line <one>\n\n  indented & raw";

    let message = mail_message(
        FullMessage {
            meta: MessageMetadata::default(),
            body: body.into(),
            mime_type: "text/plain".into(),
            attachments: Vec::new(),
            verdict: ruston_core::Verdict::Unverified,
            reply_tos: Vec::new(),
        },
        &OwnAddresses::new(),
    );

    assert_eq!(message.body, MessageBody::PlainText(body.into()));
    assert_eq!(message.time, None);
}

#[test]
fn an_unreadable_message_keeps_its_headers_and_reply_recipients() {
    let meta = MessageMetadata {
        id: "message".into(),
        sender: person("Alex", "alex@example.com"),
        to_list: vec![person("Demo", "demo@example.com")],
        cc_list: vec![person("", "team@example.org")],
        time: 1_700_000_000,
        ..Default::default()
    };
    let own = OwnAddresses::from(["demo@example.com".to_owned()]);

    let message = unreadable_mail_message(
        UnreadableMessage {
            meta,
            reply_tos: vec![person("", "replies@example.com")],
            error: Error::Crypto("body decrypt failed".into()),
        },
        &own,
    );

    assert_eq!(message.id, "message");
    assert_eq!(message.body, MessageBody::Unreadable);
    assert_eq!(message.verdict, Verdict::Unverified);
    assert!(message.attachments.is_empty());
    assert_eq!(message.sender.address, "alex@example.com");
    assert_eq!(
        (message.to.len(), message.cc.len(), message.bcc.len()),
        (1, 1, 0)
    );
    // Answering still goes where core will send it.
    assert_eq!(message.reply_to, ["replies@example.com"]);
    assert_eq!(message.reply_all_cc, ["team@example.org"]);
    assert!(message.reply_redirected);
    assert_eq!(message.time, Some(1_700_000_000));
}

#[test]
fn every_body_verdict_survives_plain_and_html_conversion() {
    use ruston_core::Verdict;

    for verdict in [
        Verdict::Verified,
        Verdict::Unsigned,
        Verdict::Unverified,
        Verdict::Invalid,
    ] {
        for mime_type in ["text/plain", "text/html"] {
            let message = mail_message(
                FullMessage {
                    meta: MessageMetadata::default(),
                    body: "body".into(),
                    mime_type: mime_type.into(),
                    attachments: Vec::new(),
                    verdict,
                    reply_tos: Vec::new(),
                },
                &OwnAddresses::new(),
            );
            assert_eq!(message.verdict, verdict);
        }
    }
    assert_eq!(
        map_mailbox_error(Error::AttachmentVerificationFailed),
        MailboxError::AttachmentVerificationFailed
    );
}

fn captcha_challenge() -> HvChallenge {
    HvChallenge {
        token: "token".into(),
        methods: vec!["captcha".into()],
        web_url: String::new(),
    }
}

#[tokio::test(start_paused = true)]
async fn a_resume_that_outlasts_its_deadline_keeps_the_session() {
    let outcome = resume_within(std::future::pending(), SIGN_IN_TIMEOUT).await;
    assert!(matches!(
        outcome,
        ResumeOutcome::Failed(AuthError::TimedOut)
    ));
}

#[tokio::test]
async fn a_resume_signs_out_only_when_proton_refuses_the_session() {
    let refused = resume_within(async { Err(Error::Unauthorized) }, SIGN_IN_TIMEOUT).await;
    assert!(matches!(refused, ResumeOutcome::SignedOut));

    let unreachable = resume_within(
        async { Err(Error::Session("keyring get: locked".into())) },
        SIGN_IN_TIMEOUT,
    )
    .await;
    assert!(matches!(
        unreachable,
        ResumeOutcome::Failed(AuthError::SessionUnavailable)
    ));
}

#[test]
fn a_verification_says_whether_signing_in_again_answers_it() {
    let email = HvChallenge {
        methods: vec!["email".into(), "sms".into()],
        ..captcha_challenge()
    };
    let unnamed = HvChallenge {
        methods: Vec::new(),
        ..captcha_challenge()
    };

    for (challenge, expected) in [
        (captcha_challenge(), AuthError::HumanVerificationRequired),
        (unnamed, AuthError::HumanVerificationRequired),
        (email, AuthError::HumanVerificationUnsupported),
    ] {
        let methods = challenge.methods.clone();
        let outcome = map_sign_in_error(
            Error::HumanVerification(challenge.clone()),
            Supplied::default(),
        );
        assert!(
            matches!(outcome, SignInOutcome::Failed(error) if error == expected),
            "{methods:?}"
        );
        assert_eq!(
            map_auth_error(Error::HumanVerification(challenge)),
            expected,
            "{methods:?}"
        );
    }
}

#[tokio::test]
async fn a_challenge_after_sign_in_is_reported_as_for_a_resumed_session() {
    // The client keeps the sign-in's resolver for the whole session, but
    // once the sign-in is over nothing shows its prompt.
    let (events, receiver) = mpsc::channel(1);
    drop(receiver);
    let prompter = Prompter::new(events);
    let error = prompter.human_verification()(captcha_challenge())
        .await
        .unwrap_err();
    assert!(matches!(error, Error::HumanVerification(_)), "{error}");
    assert_eq!(map_mailbox_error(error), MailboxError::Service);
    assert!(!prompter.cancelled.load(Ordering::Relaxed));
}

#[tokio::test]
async fn an_abandoned_prompt_cancels_the_sign_in() {
    use futures::StreamExt;

    let (events, mut shown) = mpsc::channel(1);
    let prompter = Prompter::new(events.clone());
    let verification = tokio::spawn(prompter.human_verification()(captcha_challenge()));
    let Some(SignInEvent::Prompt(prompt)) = shown.next().await else {
        panic!("no verification prompt");
    };
    prompt.cancel();
    assert!(verification.await.unwrap().is_err());
    assert!(prompter.cancelled.load(Ordering::Relaxed));
    // An abandoned prompt stops counting as one.
    let now = tokio::time::Instant::now();
    assert_eq!(
        prompter.prompt_time.spent(now),
        prompter.prompt_time.spent(now + Duration::from_secs(60))
    );

    let prompter = Prompter {
        cancelled: Arc::default(),
        ..prompter
    };
    let answered = Arc::new(AtomicBool::new(false));
    let code = tokio::spawn(prompter.totp(answered.clone())());
    let Some(SignInEvent::Prompt(prompt)) = shown.next().await else {
        panic!("no TOTP prompt");
    };
    prompt.cancel();
    assert!(code.await.unwrap().is_err());
    assert!(prompter.cancelled.load(Ordering::Relaxed));
    assert!(!answered.load(Ordering::Relaxed));

    let prompter = Prompter::new(events);
    let answered = Arc::new(AtomicBool::new(false));
    let password = tokio::spawn(prompter.mailbox_password(answered.clone())());
    let Some(SignInEvent::Prompt(prompt @ SignInPrompt::MailboxPassword(_))) = shown.next().await
    else {
        panic!("no mailbox password prompt");
    };
    prompt.cancel();
    assert!(password.await.unwrap().is_err());
    assert!(prompter.cancelled.load(Ordering::Relaxed));
    assert!(!answered.load(Ordering::Relaxed));
}

#[test]
fn answering_your_own_sent_message_addresses_its_recipients() {
    let meta = MessageMetadata {
        sender: person("Me", "me@proton.me"),
        to_list: vec![person("", "bob@corp.example")],
        flags: message_flag::SENT,
        ..Default::default()
    };
    let message = mail_message(
        FullMessage {
            meta,
            body: "Following up".into(),
            mime_type: "text/plain".into(),
            attachments: Vec::new(),
            verdict: ruston_core::Verdict::Unsigned,
            reply_tos: vec![person("Me", "me@proton.me")],
        },
        &OwnAddresses::from(["me@proton.me".to_owned()]),
    );

    assert_eq!(message.reply_to, ["bob@corp.example"]);
    assert!(!message.reply_redirected, "nothing redirects your own mail");
}

#[test]
fn saving_an_attachment_outlasts_core_s_transfer_deadline() {
    use ruston_core::api::attachments::ATTACHMENT_REQUEST_TIMEOUT;

    // The file itself may take core's whole transfer deadline; the
    // message and the sender's keys come on top of it.
    assert!(ATTACHMENT_SAVE_TIMEOUT >= ATTACHMENT_REQUEST_TIMEOUT + REQUEST_TIMEOUT * 2);
}

#[test]
fn a_long_conversation_gets_time_for_every_message() {
    // One message still fits a request for it and one for its sender's keys.
    assert_eq!(conversation_deadline(0), REQUEST_TIMEOUT * 2);
    assert_eq!(conversation_deadline(1), REQUEST_TIMEOUT * 2);
    for messages in [2, 10, 50] {
        assert!(
            conversation_deadline(messages) > REQUEST_TIMEOUT * messages,
            "{messages} messages share too little time"
        );
    }
    assert!(conversation_deadline(u32::MAX) >= conversation_deadline(50));
}

#[test]
fn a_send_outlasts_core_s_own_upload_deadlines() {
    use ruston_core::api::attachments::ATTACHMENT_REQUEST_TIMEOUT;

    // Core ends a stalled upload with an error that removes the draft; the
    // desktop deadline must not cut in first and report an unknown outcome.
    for attachments in 0..4 {
        let uploads = ATTACHMENT_REQUEST_TIMEOUT * attachments;
        assert!(
            send_deadline(attachments as usize) > uploads + SEND_TIMEOUT / 2,
            "{attachments} attachment(s) leave too little time"
        );
    }
    assert_eq!(send_deadline(0), SEND_TIMEOUT);
}

#[test]
fn replies_are_delivered_once_or_cancelled() {
    let (reply, mut answer) = PromptAnswer::channel();
    reply.clone().send(1);
    reply.send(2);
    assert_eq!(answer.try_recv(), Ok(Some(1)));

    let (reply, mut answer) = PromptAnswer::<u8>::channel();
    reply.cancel();
    assert!(answer.try_recv().is_err());
}

#[test]
fn missing_mailbox_password_has_a_structured_outcome() {
    let outcome = map_sign_in_error(Error::MailboxPasswordRequired, Supplied::default());

    assert!(matches!(outcome, SignInOutcome::NeedsMailboxPassword));
}

#[test]
fn security_key_requirement_is_supported_failure() {
    let outcome = map_sign_in_error(Error::SecurityKeyRequired, Supplied::default());

    assert!(matches!(
        outcome,
        SignInOutcome::Failed(AuthError::SecurityKeyUnsupported)
    ));
}

#[test]
fn unauthorized_totp_retry_is_an_invalid_code() {
    let outcome = map_sign_in_error(
        Error::Unauthorized,
        Supplied {
            totp: true,
            ..Supplied::default()
        },
    );

    assert!(matches!(
        outcome,
        SignInOutcome::Failed(AuthError::InvalidTotp)
    ));
}

#[test]
fn failed_mailbox_unlock_is_an_invalid_mailbox_password() {
    let outcome = map_sign_in_error(
        Error::UserKeysLocked,
        Supplied {
            mailbox_password: true,
            ..Supplied::default()
        },
    );

    assert!(matches!(
        outcome,
        SignInOutcome::Failed(AuthError::InvalidMailboxPassword)
    ));
}

#[test]
fn a_session_bound_to_another_address_asks_to_sign_in_again() {
    use ruston_core::session::{MemoryStore, Paths, Session, Tokens};

    let folder =
        std::env::temp_dir().join(format!("ruston-session-address-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    let paths = Paths::with_base(&folder);
    let save = |base_url: &str, store: &MemoryStore| {
        let session = Session {
            uid: "UID1".into(),
            app_version: "Other".into(),
            base_url: base_url.into(),
            password_mode: 1,
            user_agent: None,
            user_id: None,
        };
        let tokens = Tokens {
            uid: "UID1".into(),
            access: SecretString::from("access"),
            refresh: SecretString::from("refresh"),
        };
        session
            .save(&paths, PROFILE, store, &tokens, &SecretString::from("skp"))
            .unwrap();
    };
    // The secret store keeps the first address; the file now names another.
    let bound = MemoryStore::new();
    save("https://mail.proton.me/api", &bound);
    save("https://example.test/api", &MemoryStore::new());

    let Err(error) = Session::load(&paths, PROFILE, &bound) else {
        panic!("a session bound to another address must not load");
    };
    std::fs::remove_dir_all(&folder).unwrap();

    assert_eq!(map_auth_error(error), AuthError::SessionAddressMismatch);
    assert_eq!(
        map_auth_error(Error::Session("keyring get: locked".into())),
        AuthError::SessionUnavailable
    );
}

const BANK: &str = "notifications@bank.example.com";
const ME: &str = "me@proton.me";

fn own() -> OwnAddresses {
    [ME.to_owned()].into_iter().collect()
}

fn metadata(id: &str, sender: &str, time: i64, labels: &[&str]) -> MessageMetadata {
    MessageMetadata {
        id: id.into(),
        sender: person("", sender),
        subject: format!("Subject {id}"),
        time,
        unread: 1,
        label_ids: labels.iter().map(|label| (*label).to_owned()).collect(),
        ..Default::default()
    }
}

fn ids(rows: &[ConversationSummary]) -> Vec<&str> {
    rows.iter().map(|row| row.id.as_str()).collect()
}

#[test]
fn actions_map_to_relabels_and_flags_never_deletes() {
    let moving = |folder: MailFolder| MailAction::MoveTo(Folder::System(folder));

    assert_eq!(
        action_call(&moving(MailFolder::Archive)),
        ActionCall::Move(label_ids::ARCHIVE)
    );
    assert_eq!(
        action_call(&moving(MailFolder::Spam)),
        ActionCall::Move(label_ids::SPAM)
    );
    assert_eq!(
        action_call(&moving(MailFolder::Trash)),
        ActionCall::Move(label_ids::TRASH)
    );
    assert_eq!(
        action_call(&moving(MailFolder::Inbox)),
        ActionCall::Move(label_ids::INBOX)
    );

    // A folder the account made carries its own id straight through.
    let custom = MailAction::MoveTo(Folder::custom("kZ9", "Invoices"));
    assert_eq!(action_call(&custom), ActionCall::Move("kZ9"));

    // A row only a search found has no folder to be read in, so Proton
    // is pointed at All Mail, which holds every conversation.
    assert_eq!(context_label(None), label_ids::ALL_MAIL);
    assert_eq!(context_label(Some(&Folder::INBOX)), label_ids::INBOX);
    assert_eq!(
        context_label(Some(&Folder::custom("kZ9", "Invoices"))),
        "kZ9"
    );

    // A label change names the label and says which way it goes.
    let receipts = Folder::label("wN2", "Receipts");
    assert_eq!(
        action_call(&MailAction::SetLabel {
            label: receipts.clone(),
            on: true,
        }),
        ActionCall::Label("wN2", true)
    );
    assert_eq!(
        action_call(&MailAction::SetLabel {
            label: receipts,
            on: false,
        }),
        ActionCall::Label("wN2", false)
    );
    assert_eq!(
        action_call(&MailAction::SetUnread(true)),
        ActionCall::MarkRead(false)
    );
    assert_eq!(
        action_call(&MailAction::SetUnread(false)),
        ActionCall::MarkRead(true)
    );
    assert_eq!(
        action_call(&MailAction::SetStarred(true)),
        ActionCall::Star(true)
    );
}

#[test]
fn inspection_failures_keep_grouping_but_expired_sessions_fail() {
    let outcome = |inspection| inspection_outcome(inspection, &Folder::INBOX, &own());

    let repeated = vec![
        metadata("m1", BANK, 10, &[label_ids::INBOX]),
        metadata("m2", BANK, 20, &[label_ids::INBOX]),
    ];
    let split = outcome(Ok(repeated))
        .unwrap()
        .expect("repeated mail splits");
    assert_eq!(ids(&split), ["m2", "m1"]);
    // What `limited_inspection` hands over: core's errors as the mailbox
    // maps them, and Unavailable for an inspection that ran out of time.
    for kept in [
        MailboxError::Unavailable,
        map_mailbox_error(Error::Other("unexpected".into())),
        map_mailbox_error(api_error(500)),
    ] {
        assert_eq!(outcome(Err(kept)), Ok(None));
    }
    for expired in [Error::Unauthorized, api_error(401)] {
        assert_eq!(
            outcome(Err(map_mailbox_error(expired))),
            Err(MailboxError::SessionExpired)
        );
    }
}

#[test]
fn repeated_inbound_mail_becomes_individual_rows() {
    let messages = vec![
        metadata("m1", BANK, 10, &[label_ids::INBOX]),
        metadata("m2", BANK, 30, &[label_ids::INBOX, label_ids::STARRED]),
        metadata("m3", BANK, 20, &[label_ids::INBOX]),
    ];

    let rows = inspect_messages(messages, &Folder::INBOX, &own()).expect("split");

    assert_eq!(ids(&rows), ["m2", "m3", "m1"]);
    assert!(rows.iter().all(|row| row.kind == SummaryKind::Message));
    assert!(rows.iter().all(|row| row.message_count == 1 && row.unread));
    assert!(rows[0].starred && !rows[1].starred);
    assert_eq!(rows[0].subject.as_deref(), Some("Subject m2"));
    assert_eq!(rows[0].correspondents.as_deref(), Some(BANK));
}

#[test]
fn split_rows_only_include_messages_in_the_folder() {
    let messages = vec![
        metadata("m1", BANK, 10, &[label_ids::INBOX]),
        metadata("m2", BANK, 20, &[label_ids::TRASH]),
    ];

    let rows = inspect_messages(messages, &Folder::INBOX, &own()).expect("split");

    assert_eq!(ids(&rows), ["m1"]);
}

#[test]
fn exchanges_and_answered_mail_stay_one_conversation() {
    let reply = vec![
        metadata("m1", BANK, 10, &[label_ids::INBOX]),
        metadata("m2", ME, 20, &[label_ids::SENT]),
    ];
    let mut answered = metadata("m3", BANK, 30, &[label_ids::INBOX]);
    answered.flags = message_flag::REPLIED;
    let flagged = vec![metadata("m1", BANK, 10, &[label_ids::INBOX]), answered];

    for messages in [reply, flagged] {
        assert_eq!(inspect_messages(messages, &Folder::INBOX, &own()), None);
    }
}

#[test]
fn classification_ignores_subjects() {
    let mut same_subject = vec![
        metadata("m1", BANK, 10, &[label_ids::INBOX]),
        metadata("m2", ME, 20, &[label_ids::INBOX]),
    ];
    for message in &mut same_subject {
        message.subject = "Statement".into();
    }
    let different_subjects = vec![
        metadata("m1", BANK, 10, &[label_ids::INBOX]),
        metadata("m2", BANK, 20, &[label_ids::INBOX]),
    ];

    let grouped = inspect_messages(same_subject, &Folder::INBOX, &own());
    let split = inspect_messages(different_subjects, &Folder::INBOX, &own());

    assert_eq!(grouped, None);
    assert_eq!(split.map(|rows| rows.len()), Some(2));
}

#[test]
fn missing_folder_labels_fall_back_to_the_conversation() {
    let messages = vec![
        metadata("m1", BANK, 10, &[label_ids::ARCHIVE]),
        metadata("m2", BANK, 20, &[label_ids::ARCHIVE]),
    ];

    assert_eq!(inspect_messages(messages, &Folder::INBOX, &own()), None);
}

#[test]
fn conversation_maps_to_summary() {
    let conversation = Conversation {
        id: "conversation".into(),
        subject: "  Quarterly report ".into(),
        time: 1_700_000_000,
        num_messages: 3,
        num_unread: 1,
        senders: vec![
            person("Ada", "ada@example.com"),
            person("", "bob@example.com"),
        ],
        labels: vec![label(label_ids::INBOX), label(label_ids::STARRED)],
        ..Default::default()
    };

    let summary = summarize(conversation, &Folder::INBOX);

    assert_eq!(
        summary,
        ConversationSummary {
            id: "conversation".into(),
            kind: SummaryKind::Conversation,
            subject: Some("Quarterly report".into()),
            correspondents: Some("Ada, bob@example.com".into()),
            participants: vec![
                MailAddress {
                    name: Some("Ada".into()),
                    address: "ada@example.com".into(),
                },
                MailAddress {
                    name: None,
                    address: "bob@example.com".into(),
                },
            ],
            preview: None,
            time: Some(1_700_000_000),
            unread: true,
            starred: true,
            message_count: 3,
            has_attachments: false,
        }
    );
}

#[test]
fn missing_subject_sender_and_time_are_absent() {
    let conversation = Conversation {
        id: "conversation".into(),
        subject: " ".into(),
        num_messages: -1,
        senders: vec![person(" ", "")],
        ..Default::default()
    };

    let summary = summarize(conversation, &Folder::INBOX);

    assert_eq!(summary.subject, None);
    assert_eq!(summary.correspondents, None);
    assert_eq!(summary.time, None);
    assert_eq!(summary.message_count, 0);
    assert!(!summary.unread);
    assert!(!summary.starred);
}

#[test]
fn a_message_row_trims_its_subject_and_names_and_drops_what_is_blank() {
    let padded = MessageMetadata {
        subject: "  Statement ready ".into(),
        sender: person("  Bank  ", &format!(" {BANK} ")),
        ..metadata("padded", BANK, 1_700_000_000, &[label_ids::INBOX])
    };
    let blank = MessageMetadata {
        subject: " ".into(),
        sender: person(" ", BANK),
        ..metadata("blank", BANK, 0, &[label_ids::INBOX])
    };

    let padded = message_summary(padded);
    let blank = message_summary(blank);

    assert_eq!(padded.subject.as_deref(), Some("Statement ready"));
    assert_eq!(padded.correspondents.as_deref(), Some("Bank"));
    assert_eq!(
        padded.participants,
        [MailAddress {
            name: Some("Bank".into()),
            address: BANK.into(),
        }]
    );
    assert_eq!(padded.time, Some(1_700_000_000));
    assert_eq!(blank.subject, None);
    assert_eq!(blank.correspondents.as_deref(), Some(BANK));
    assert_eq!(
        blank.participants,
        [MailAddress {
            name: None,
            address: BANK.into(),
        }]
    );
    assert_eq!(blank.time, None);
}

#[test]
fn a_message_on_its_own_with_a_blank_subject_has_none() {
    let unreadable = UnreadableMessage {
        meta: MessageMetadata {
            subject: "  ".into(),
            ..metadata("m1", BANK, 0, &[label_ids::INBOX])
        },
        reply_tos: Vec::new(),
        error: Error::Crypto("no key of the account opens it".into()),
    };

    let detail = single_message_detail(
        "m1",
        ConversationMessage::Unreadable(unreadable),
        &OwnAddresses::new(),
    );

    assert_eq!(detail.subject, None);
    assert_eq!(detail.messages[0].time, None);
}

#[test]
fn folder_context_overrides_conversation_totals() {
    let mut sent = label(label_ids::SENT);
    sent.context_num_unread = Some(0);
    sent.context_time = Some(42);
    let conversation = Conversation {
        id: "conversation".into(),
        time: 99,
        num_unread: 2,
        senders: vec![person("Sender", "")],
        recipients: vec![person("Recipient", "")],
        labels: vec![sent],
        ..Default::default()
    };

    let summary = summarize(conversation, &Folder::System(MailFolder::Sent));

    assert!(!summary.unread);
    assert_eq!(summary.time, Some(42));
    assert_eq!(summary.correspondents.as_deref(), Some("Recipient"));
}

#[test]
fn long_correspondent_lists_are_shortened() {
    let people: Vec<_> = ["A", "B", "C", "D"]
        .into_iter()
        .map(|name| person(name, ""))
        .collect();

    assert_eq!(display_names(&people).as_deref(), Some("A, B, C, …"));
}

#[test]
fn counts_map_only_known_folders() {
    let counts = [
        LabelCount {
            label_id: label_ids::INBOX.into(),
            total: 10,
            unread: 4,
        },
        LabelCount {
            label_id: "custom".into(),
            total: 1,
            unread: 1,
        },
        // The API should not duplicate identifiers, but preserving the
        // first one matches the former linear lookup if it ever does.
        LabelCount {
            label_id: label_ids::INBOX.into(),
            total: 10,
            unread: 9,
        },
    ];

    let invoices = Folder::custom("custom", "Invoices");
    let mapped = map_counts(&counts, std::slice::from_ref(&invoices));

    assert_eq!(mapped.unread(&Folder::INBOX), Some(4));
    assert_eq!(mapped.unread(&Folder::System(MailFolder::Trash)), None);
    // A folder the account made is counted like any other, once known.
    assert_eq!(mapped.unread(&invoices), Some(1));

    // Unknown to the sidebar means unknown to the counts.
    assert_eq!(map_counts(&counts, &[]).unread(&invoices), None);
}

#[test]
fn a_session_that_cannot_be_read_or_saved_is_named_as_such() {
    for error in [
        Error::Session("keyring get: locked".to_owned()),
        Error::Session("keyring set: access denied".to_owned()),
        Error::SessionAddressMismatch,
    ] {
        assert_eq!(map_mailbox_error(error), MailboxError::SessionUnavailable);
    }
    // Reading a file to attach has nothing to do with the session.
    assert_eq!(
        map_mailbox_error(Error::Io(std::io::Error::other("unreadable"))),
        MailboxError::Unavailable
    );
    let sending = SendError::Mailbox(MailboxError::SessionUnavailable).message();
    assert!(sending.contains("was not sent"), "{sending}");
    assert!(!sending.contains("would not accept"), "{sending}");
}

#[test]
fn expired_session_errors_are_distinguished() {
    assert_eq!(
        map_mailbox_error(Error::Unauthorized),
        MailboxError::SessionExpired
    );
    assert_eq!(
        map_mailbox_error(api_error(401)),
        MailboxError::SessionExpired
    );
    assert_eq!(map_mailbox_error(api_error(500)), MailboxError::Service);
    assert_eq!(
        map_mailbox_error(Error::Other("unexpected".into())),
        MailboxError::Unavailable
    );
}
