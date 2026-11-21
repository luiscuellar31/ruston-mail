use super::*;
use crate::mail::MailboxError;

fn written(to: &str) -> Compose {
    let mut compose = Compose::default();
    compose.set(ComposeField::To, to.to_owned());
    compose.set(ComposeField::Subject, "  Thursday  ".to_owned());
    compose.set(ComposeField::Body, "See you then.".to_owned());
    compose
}

fn composing(to: &str) -> App {
    let (mut app, _) = App::boot(true, crate::settings::Settings::default());
    let _ = app.update(Message::OpenCompose);
    let _ = app.update(Message::ComposeChanged(ComposeField::To, to.to_owned()));
    app
}

fn checked(app: &mut App, checks: Vec<ProtectionCheck>) {
    let epoch = app.session_epoch;
    let id = app.compose().unwrap().id();
    let _ = app.update(Message::ComposeProtectionChecked(epoch, id, checks));
}

fn notice(app: &App) -> Option<ProtectionNotice> {
    app.compose().unwrap().protection_notice()
}

#[test]
fn protection_is_looked_up_when_a_recipient_field_is_left() {
    let mut app = composing("ada@proton.me, half-typed");
    assert_eq!(notice(&app), None);

    let effects = app.update(Message::ComposeRecipientsLeft);
    assert_eq!(effects.units(), 1);
    assert_eq!(notice(&app), Some(ProtectionNotice::Checking));

    // Nothing new to ask about: the same address is not looked up twice.
    assert_eq!(app.update(Message::ComposeRecipientsLeft).units(), 0);

    checked(
        &mut app,
        vec![("ada@proton.me".into(), Ok(Protection::EndToEnd))],
    );
    assert_eq!(notice(&app), Some(ProtectionNotice::Encrypted));
}

#[test]
fn a_kept_draft_asks_again_about_recipients_its_session_left_unanswered() {
    let opened_again = |checking: bool| {
        let mut app = App::signed_in_as("user-1", "alex@proton.me");
        let _ = app.update(Message::OpenCompose);
        let _ = app.update(Message::ComposeChanged(
            ComposeField::To,
            "ada@proton.me".into(),
        ));
        let _ = app.update(Message::ComposeRecipientsLeft);
        if !checking {
            checked(
                &mut app,
                vec![("ada@proton.me".into(), Ok(Protection::EndToEnd))],
            );
        }
        let _ = app.update(Message::ConversationsLoaded(
            1,
            Err(MailboxError::SessionExpired),
        ));
        let effects = app.open_mailbox(
            crate::mail::MailBackend::demo(),
            None,
            Some("user-1".into()),
        );
        (app, effects.units())
    };

    let (_, answered) = opened_again(false);
    let (mut app, unanswered) = opened_again(true);
    // The ended session's answer will never come, so it is asked again.
    assert_eq!(unanswered, answered + 1);
    assert_eq!(notice(&app), Some(ProtectionNotice::Checking));
    checked(
        &mut app,
        vec![("ada@proton.me".into(), Ok(Protection::EndToEnd))],
    );
    assert_eq!(notice(&app), Some(ProtectionNotice::Encrypted));
}

#[test]
fn an_unencrypted_recipient_is_named() {
    let mut app = composing("ada@proton.me; key@example.org, plain@example.com");
    let _ = app.update(Message::ComposeRecipientsLeft);
    checked(
        &mut app,
        vec![
            ("ada@proton.me".into(), Ok(Protection::EndToEnd)),
            ("key@example.org".into(), Ok(Protection::Pgp)),
            ("plain@example.com".into(), Ok(Protection::Unencrypted)),
        ],
    );

    assert_eq!(
        notice(&app),
        Some(ProtectionNotice::Warning {
            unencrypted: vec!["plain@example.com".into()],
            unknown: Vec::new(),
            unchecked: 0,
        })
    );
}

#[test]
fn past_the_lookup_limit_recipients_are_counted_not_looked_up() {
    let addresses: Vec<String> = (0..120).map(|n| format!("person{n}@proton.me")).collect();
    let mut app = composing(&addresses.join(", "));
    assert_eq!(app.update(Message::ComposeRecipientsLeft).units(), 1);
    let asked: Vec<String> = app.compose().unwrap().protection.keys().cloned().collect();
    assert_eq!(asked.len(), PROTECTION_LOOKUP_LIMIT);
    assert!(
        asked
            .iter()
            .all(|address| addresses[..100].contains(address))
    );
    checked(
        &mut app,
        addresses[..100]
            .iter()
            .map(|address| (address.clone(), Ok(Protection::EndToEnd)))
            .collect(),
    );

    // Nothing more is asked, and the rest are counted in one line.
    assert_eq!(app.update(Message::ComposeRecipientsLeft).units(), 0);
    let notice = notice(&app).unwrap();
    assert_eq!(
        notice,
        ProtectionNotice::Warning {
            unencrypted: Vec::new(),
            unknown: Vec::new(),
            unchecked: 20,
        }
    );
    assert_eq!(
        notice.message(),
        "Not checked: 20 more recipients, past the first 100."
    );
}

#[test]
fn a_failed_lookup_claims_nothing() {
    let mut app = composing("ada@proton.me");
    let _ = app.update(Message::ComposeRecipientsLeft);
    checked(
        &mut app,
        vec![("ada@proton.me".into(), Err(MailboxError::Connection))],
    );

    assert_eq!(
        notice(&app),
        Some(ProtectionNotice::Warning {
            unencrypted: Vec::new(),
            unknown: vec!["ada@proton.me".into()],
            unchecked: 0,
        })
    );
}

#[test]
fn an_unchecked_recipient_is_named_beside_an_unencrypted_one() {
    let mut app = composing("plain@example.com, ada@proton.me");
    let _ = app.update(Message::ComposeRecipientsLeft);
    checked(
        &mut app,
        vec![
            ("plain@example.com".into(), Ok(Protection::Unencrypted)),
            ("ada@proton.me".into(), Err(MailboxError::Connection)),
        ],
    );

    let notice = notice(&app).unwrap();
    assert_eq!(
        notice,
        ProtectionNotice::Warning {
            unencrypted: vec!["plain@example.com".into()],
            unknown: vec!["ada@proton.me".into()],
            unchecked: 0,
        }
    );
    let message = notice.message();
    assert!(message.contains("plain@example.com"), "{message}");
    assert!(message.contains("ada@proton.me"), "{message}");
}

#[test]
fn a_new_address_silences_the_notice_until_it_is_checked() {
    let mut app = composing("ada@proton.me");
    let _ = app.update(Message::ComposeRecipientsLeft);
    checked(
        &mut app,
        vec![("ada@proton.me".into(), Ok(Protection::EndToEnd))],
    );

    let _ = app.update(Message::ComposeChanged(
        ComposeField::To,
        "ada@proton.me, plain@example.com".into(),
    ));
    assert_eq!(notice(&app), None);

    // Only the new address is asked about.
    assert_eq!(app.update(Message::ComposeRecipientsLeft).units(), 1);
    assert_eq!(notice(&app), Some(ProtectionNotice::Checking));
}

#[test]
fn an_answer_for_a_removed_address_is_dropped() {
    let mut app = composing("plain@example.com");
    let _ = app.update(Message::ComposeRecipientsLeft);
    let _ = app.update(Message::ComposeChanged(
        ComposeField::To,
        "ada@proton.me".into(),
    ));
    checked(
        &mut app,
        vec![("plain@example.com".into(), Ok(Protection::Unencrypted))],
    );

    assert_eq!(notice(&app), None);
}

#[test]
fn an_answer_for_another_draft_is_dropped() {
    let mut app = composing("plain@example.com");
    let _ = app.update(Message::ComposeRecipientsLeft);
    let epoch = app.session_epoch;
    let stale = app.compose().unwrap().id() + 1;
    let _ = app.update(Message::ComposeProtectionChecked(
        epoch,
        stale,
        vec![("plain@example.com".into(), Ok(Protection::Unencrypted))],
    ));

    assert_eq!(notice(&app), Some(ProtectionNotice::Checking));
}

#[test]
fn a_reply_says_how_its_recipients_are_protected_like_any_message() {
    let mut compose = written("ada@proton.me");
    compose.kind = OutgoingKind::Reply {
        message_id: "message".into(),
        everyone: false,
        quote: Quote::Original,
    };
    compose
        .protection
        .insert("ada@proton.me".into(), Checked::Known(Protection::EndToEnd));
    assert_eq!(
        compose.protection_notice(),
        Some(ProtectionNotice::Encrypted)
    );
}

#[test]
fn every_change_to_a_recipient_field_is_read_again() {
    let mut compose = written("Ana@Example.org; oops");
    assert_eq!(
        compose.not_ready(),
        Some(NotReady::BadAddresses(vec!["oops".into()]))
    );

    compose.set(ComposeField::To, "Ana@Example.org".into());
    compose.set(ComposeField::Cc, "ana@example.org, bob@example.org".into());
    assert_eq!(compose.not_ready(), None);
    // Lookups name each person once, as first written.
    assert_eq!(
        compose.typed.addresses,
        ["Ana@Example.org", "bob@example.org"]
    );
    let outgoing = compose.ready(BodyFormat::PlainText).unwrap();
    assert_eq!(outgoing.to, ["Ana@Example.org"]);
    assert_eq!(outgoing.cc, ["ana@example.org", "bob@example.org"]);

    // An address that leaves the fields takes its lookup with it.
    compose
        .protection
        .insert("bob@example.org".into(), Checked::Checking);
    compose.set(ComposeField::Cc, String::new());
    assert!(compose.protection.is_empty());
    compose.set(ComposeField::To, String::new());
    assert_eq!(compose.not_ready(), Some(NotReady::NoRecipients));
    assert!(compose.typed.addresses.is_empty());
}

fn answered_message(reply_to: &str) -> MailMessage {
    MailMessage {
        id: "message".into(),
        sender: crate::mail::MailAddress {
            name: Some("Alex Rivera".into()),
            address: "alex@corp.example".into(),
        },
        to: Vec::new(),
        cc: Vec::new(),
        bcc: Vec::new(),
        reply_to: vec![reply_to.into()],
        reply_all_cc: vec!["bob@corp.example".into()],
        reply_redirected: !reply_to.eq_ignore_ascii_case("alex@corp.example"),
        time: None,
        unread: false,
        body: crate::mail::MessageBody::PlainText("Invoice attached".into()),
        verdict: crate::mail::Verdict::Unverified,
        attachments: Vec::new(),
    }
}

fn answering(kind: AnswerKind, message: &MailMessage) -> Answering {
    answering_for(kind, message, Some("Invoice"))
}

#[test]
fn a_forward_leaves_each_file_out_once_and_a_reply_has_none_to_leave() {
    let mut message = answered_message("alex@corp.example");
    message.attachments = ["a1", "a2"]
        .map(|id| crate::mail::MailAttachment {
            id: id.into(),
            name: format!("{id}.pdf"),
            size: 1,
            inline: false,
            content_id: None,
        })
        .to_vec();
    let mut forward = Compose {
        kind: AnswerKind::Forward.outgoing("message"),
        answering: Some(answering(AnswerKind::Forward, &message)),
        ..Compose::default()
    };
    forward.leave_out_forwarded("a1");
    forward.leave_out_forwarded("a1");
    assert_eq!(forward.kind.leave_out(), ["a1"]);
    let left: Vec<&str> = forward.forwarded().map(|file| file.id.as_str()).collect();
    assert_eq!(left, ["a2"]);

    let mut reply = Compose {
        kind: AnswerKind::Reply.outgoing("message"),
        answering: Some(answering(AnswerKind::Reply, &message)),
        ..Compose::default()
    };
    reply.leave_out_forwarded("a1");
    assert!(reply.kind.leave_out().is_empty());
    assert_eq!(reply.forwarded().count(), 0);
}

#[test]
fn an_answer_says_what_goes_out_with_its_text() {
    let mut message = answered_message("alex@corp.example");
    message.attachments = vec![crate::mail::MailAttachment {
        id: "a1".into(),
        name: "invoice.pdf".into(),
        size: 2048,
        inline: false,
        content_id: None,
    }];

    let forward = answering(AnswerKind::Forward, &message);
    assert_eq!(forward.forwarded, message.attachments);
    assert!(forward.quotes_original);
    for kind in [AnswerKind::Reply, AnswerKind::ReplyAll] {
        let reply = answering(kind, &message);
        assert!(reply.forwarded.is_empty(), "{kind:?} carries no files");
        assert!(reply.quotes_original, "{kind:?}");
    }

    // Core quotes only an original that opens.
    message.body = crate::mail::MessageBody::Unreadable;
    assert!(!answering(AnswerKind::Reply, &message).quotes_original);
}

#[test]
fn a_reply_shows_where_it_goes_and_flags_a_redirect() {
    let honest = answering(AnswerKind::Reply, &answered_message("Alex@Corp.example"));
    assert_eq!(honest.to, ["Alex@Corp.example"]);
    assert!(honest.cc.is_empty());
    assert_eq!(honest.redirected_from, None);

    let redirected = answering(
        AnswerKind::Reply,
        &answered_message("attacker@evil.example"),
    );
    assert_eq!(redirected.to, ["attacker@evil.example"]);
    assert_eq!(
        redirected.redirected_from.as_deref(),
        Some("alex@corp.example")
    );

    let everyone = answering(AnswerKind::ReplyAll, &answered_message("alex@corp.example"));
    assert_eq!(everyone.cc, ["bob@corp.example"]);

    let forward = answering(
        AnswerKind::Forward,
        &answered_message("attacker@evil.example"),
    );
    assert!(forward.to.is_empty() && forward.cc.is_empty());
    assert_eq!(forward.redirected_from, None, "a forward is addressed here");
}

#[test]
fn with_the_check_off_nothing_is_asked_and_no_notice_is_shown() {
    let mut app = composing("plain@example.com");
    let _ = app.update(Message::ComposeRecipientsLeft);
    checked(
        &mut app,
        vec![("plain@example.com".into(), Ok(Protection::Unencrypted))],
    );
    assert!(notice(&app).is_some());

    let _ = app.update(Message::SetCheckRecipientProtection(false));
    assert!(!app.settings().check_recipient_protection);
    assert_eq!(notice(&app), None);
    let _ = app.update(Message::ComposeChanged(
        ComposeField::To,
        "plain@example.com, ada@proton.me".into(),
    ));
    assert_eq!(app.update(Message::ComposeRecipientsLeft).units(), 0);
    assert_eq!(notice(&app), None);

    // Turned back on, the next field left looks every address up again.
    let _ = app.update(Message::SetCheckRecipientProtection(true));
    assert_eq!(app.update(Message::ComposeRecipientsLeft).units(), 1);
    assert_eq!(notice(&app), Some(ProtectionNotice::Checking));
}

#[test]
fn international_domains_are_taken_only_when_settings_allow_them() {
    let mut app = composing("ana@m\u{fc}nchen.de");
    let compose = app.compose().unwrap();
    let refusal = compose.not_ready().unwrap();
    assert_eq!(
        refusal,
        NotReady::InternationalDomains(vec!["ana@m\u{fc}nchen.de".into()])
    );
    // The refusal says how to send it after all.
    let message = refusal.message();
    assert!(message.contains("xn--"), "{message}");
    assert!(
        message.contains("Advanced section of Settings"),
        "{message}"
    );
    assert_eq!(compose.international_notice(), None);

    // The open draft follows the setting at once.
    let _ = app.update(Message::SetAcceptInternationalDomains(true));
    assert!(app.settings().accept_international_domains);
    let compose = app.compose().unwrap();
    assert_eq!(compose.not_ready(), None);
    assert_eq!(
        compose.ready(BodyFormat::PlainText).unwrap().to,
        ["ana@xn--mnchen-3ya.de"]
    );
    let notice = compose.international_notice().unwrap();
    assert!(
        notice.contains("m\u{fc}nchen.de is sent as xn--mnchen-3ya.de"),
        "{notice}"
    );

    let _ = app.update(Message::SetAcceptInternationalDomains(false));
    let compose = app.compose().unwrap();
    assert!(compose.not_ready().is_some());
    assert_eq!(compose.international_notice(), None);
}

#[test]
fn a_new_draft_takes_the_domains_settings_allow() {
    let mut settings = crate::settings::Settings::default();
    settings.accept_international_domains = true;
    let (mut app, _) = App::boot(true, settings);
    let _ = app.update(Message::OpenCompose);
    let _ = app.update(Message::ComposeChanged(
        ComposeField::Cc,
        "ana@m\u{fc}nchen.de, sam@m\u{fc}nchen.de".into(),
    ));

    let compose = app.compose().unwrap();
    assert_eq!(compose.not_ready(), None);
    // One line for the domain, however many addresses use it.
    let notice = compose.international_notice().unwrap();
    assert_eq!(notice.matches("xn--mnchen-3ya.de").count(), 1, "{notice}");
}

#[test]
fn protection_is_looked_up_for_the_address_that_is_sent() {
    let mut app = composing("ana@m\u{fc}nchen.de");
    let _ = app.update(Message::SetAcceptInternationalDomains(true));
    assert_eq!(app.update(Message::ComposeRecipientsLeft).units(), 1);
    checked(
        &mut app,
        vec![("ana@xn--mnchen-3ya.de".into(), Ok(Protection::Unencrypted))],
    );
    assert_eq!(
        notice(&app),
        Some(ProtectionNotice::Warning {
            unencrypted: vec!["ana@xn--mnchen-3ya.de".into()],
            unknown: Vec::new(),
            unchecked: 0,
        })
    );

    // Refused again, the address and what was looked up for it are gone.
    let _ = app.update(Message::SetAcceptInternationalDomains(false));
    assert_eq!(notice(&app), None);
    let _ = app.update(Message::SetAcceptInternationalDomains(true));
    assert_eq!(notice(&app), None);
    assert_eq!(app.update(Message::ComposeRecipientsLeft).units(), 1);
}

#[test]
fn only_a_script_refusal_points_at_the_setting() {
    // A real mistake beside it gets the plain refusal, which names both.
    let mut compose = written("ana@m\u{fc}nchen.de, not-an-address");
    assert_eq!(
        compose.not_ready(),
        Some(NotReady::BadAddresses(vec![
            "ana@m\u{fc}nchen.de".into(),
            "not-an-address".into()
        ]))
    );
    // With international domains on, one that cannot be converted is
    // simply not an address.
    compose.set_domains(Domains::International);
    compose.set(ComposeField::To, "ana@m\u{fc}nchen".to_owned());
    assert_eq!(
        compose.not_ready(),
        Some(NotReady::BadAddresses(vec!["ana@m\u{fc}nchen".into()]))
    );
}

#[test]
fn a_message_needs_someone_to_go_to() {
    let mut compose = Compose::default();
    assert_eq!(compose.not_ready(), Some(NotReady::NoRecipients));

    compose.set(ComposeField::Subject, "Thursday".to_owned());
    assert_eq!(compose.not_ready(), Some(NotReady::NoRecipients));

    compose.set(ComposeField::To, "alex@example.com".to_owned());
    assert_eq!(compose.not_ready(), None);
}

#[test]
fn a_copy_field_alone_is_enough_to_send() {
    let mut compose = Compose::default();
    compose.set(ComposeField::Bcc, "alex@example.com".to_owned());

    assert_eq!(compose.not_ready(), None);
}

#[test]
fn one_bad_address_stops_the_whole_message() {
    // Sending to everyone else and quietly dropping the rest would reach
    // fewer people than the sender believes.
    let mut compose = written("alex@example.com, oops");
    compose.set(ComposeField::Cc, "also-wrong".to_owned());

    let Some(NotReady::BadAddresses(pieces)) = compose.not_ready() else {
        panic!("expected the addresses to be refused");
    };
    assert_eq!(pieces, ["oops", "also-wrong"]);
    assert!(compose.not_ready().unwrap().message().contains("oops"));
}

#[test]
fn a_ready_message_carries_what_was_typed() {
    let compose = written("alex@example.com; sam@example.org");
    let outgoing = compose.ready(BodyFormat::PlainText).unwrap();

    assert_eq!(outgoing.to, ["alex@example.com", "sam@example.org"]);
    // A subject is tidied, a body is not: its spacing is the message.
    assert_eq!(outgoing.subject, "Thursday");
    assert_eq!(outgoing.body, "See you then.");
    assert_eq!(outgoing.format, BodyFormat::PlainText);
}

#[test]
fn the_format_comes_from_the_settings_not_the_window() {
    let compose = written("alex@example.com");

    assert!(!compose.ready(BodyFormat::PlainText).unwrap().is_html());
    assert!(compose.ready(BodyFormat::Html).unwrap().is_html());
}

#[test]
fn a_message_with_no_subject_may_still_be_sent() {
    let mut compose = Compose::default();
    compose.set(ComposeField::To, "alex@example.com".to_owned());

    assert_eq!(compose.not_ready(), None);
    assert_eq!(compose.ready(BodyFormat::PlainText).unwrap().subject, "");
}

#[test]
fn typing_after_a_refusal_clears_it() {
    let mut compose = written("alex@example.com");
    compose.state = Sending::Failed(SendError::DemoLimitReached);

    compose.set(ComposeField::Body, "Actually, Friday.".to_owned());

    assert_eq!(compose.sending(), Sending::Writing);
}

#[test]
fn editing_does_not_hide_an_unconfirmed_send_warning() {
    let mut compose = written("alex@example.com");
    compose.state = Sending::Failed(SendError::Unconfirmed);

    compose.set(ComposeField::Body, "A revised body.".to_owned());
    compose.add_attachments([PathBuf::from("note.txt")]);
    compose.remove_attachment(0);

    assert_eq!(compose.sending(), Sending::Failed(SendError::Unconfirmed));
}

#[test]
fn an_untouched_message_is_told_from_a_written_one() {
    assert!(Compose::default().is_untouched());
    assert!(!written("alex@example.com").is_untouched());

    let mut spaces = Compose::default();
    spaces.set(ComposeField::Body, "   \n ".to_owned());
    assert!(spaces.is_untouched(), "whitespace is not writing");
}

#[test]
fn closing_an_untouched_message_discards_immediately() {
    let mut app = App::new(crate::settings::Settings::default());
    let _ = app.open_compose();
    assert!(app.compose().is_some());

    app.close_compose();
    assert!(app.compose().is_none());
}

#[test]
fn closing_a_written_message_asks_for_confirmation() {
    let mut app = App::new(crate::settings::Settings::default());
    let _ = app.open_compose();
    app.change_compose(ComposeField::Body, "Draft content".to_owned());

    app.close_compose();
    assert!(app.compose().is_some());
    assert!(app.compose().unwrap().confirming_discard());

    // Cancelling confirmation returns to writing
    app.cancel_discard();
    assert!(!app.compose().unwrap().confirming_discard());
    assert!(app.compose().is_some());

    // Discarding clears the draft
    app.discard_compose();
    assert!(app.compose().is_none());
}

#[test]
fn attachments_make_a_message_touched() {
    let mut compose = Compose::default();
    assert!(compose.is_untouched());

    compose.add_attachments([PathBuf::from("/tmp/test.txt")]);
    assert!(!compose.is_untouched());
    assert_eq!(compose.attachments(), &[PathBuf::from("/tmp/test.txt")]);

    compose.remove_attachment(0);
    assert!(compose.attachments().is_empty());
    assert!(compose.is_untouched());
}

#[test]
fn attachments_are_deduplicated() {
    let mut compose = Compose::default();
    compose.add_attachments([
        PathBuf::from("/tmp/test.txt"),
        PathBuf::from("/tmp/test.txt"),
    ]);
    assert_eq!(compose.attachments().len(), 1);
}

#[test]
fn ready_carries_attachments_without_blocking_io() {
    let mut compose = written("alex@example.com");
    let non_existent = PathBuf::from("/tmp/this_file_does_not_exist_ruston_test.xyz");
    compose.add_attachments([non_existent.clone()]);

    // In-memory UI readiness must not perform blocking disk I/O
    assert_eq!(compose.not_ready(), None);

    let outgoing = compose.ready(BodyFormat::PlainText).unwrap();
    assert_eq!(outgoing.attachments, vec![non_existent]);
}
