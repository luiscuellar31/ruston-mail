//! Writing, sending and answering a message, and the files attached to it.

use super::*;

#[test]
fn escape_key_closes_untouched_compose_without_prompt() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::OpenCompose);
    assert!(app.compose().is_some());

    press(&mut app, Key::Escape);
    assert!(app.compose().is_none());
}

#[test]
fn escape_key_prompts_to_discard_when_message_has_content() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::OpenCompose);
    let _ = app.update(Message::ComposeChanged(
        ComposeField::Body,
        "Drafting...".into(),
    ));

    // First escape asks for confirmation instead of discarding
    press(&mut app, Key::Escape);
    assert!(app.compose().is_some());
    assert!(app.compose().unwrap().confirming_discard());

    // Second escape cancels confirmation and keeps the draft
    press(&mut app, Key::Escape);
    assert!(app.compose().is_some());
    assert!(!app.compose().unwrap().confirming_discard());

    // Discard button dispatches DiscardCompose
    let _ = app.update(Message::CloseCompose);
    assert!(app.compose().unwrap().confirming_discard());
    let _ = app.update(Message::DiscardCompose);
    assert!(app.compose().is_none());
}

#[test]
fn typing_clears_discard_prompt() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::OpenCompose);
    let _ = app.update(Message::ComposeChanged(
        ComposeField::Body,
        "Drafting...".into(),
    ));
    let _ = app.update(Message::CloseCompose);
    assert!(app.compose().unwrap().confirming_discard());

    let _ = app.update(Message::ComposeChanged(
        ComposeField::Body,
        "Drafting more...".into(),
    ));
    assert!(!app.compose().unwrap().confirming_discard());
}

#[test]
fn asking_for_another_message_brings_the_open_one_forward_and_says_why() {
    let mut app = loaded_demo_app();
    open_conversation(&mut app, "demo-0");
    let _ = app.update(Message::SetComposePlacement(ComposePlacement::Window));
    let _ = app.update(Message::OpenCompose);
    let _ = app.update(Message::ComposeChanged(ComposeField::Body, "Plans".into()));
    let draft = app.compose().unwrap().id();

    for asked in [
        Message::OpenCompose,
        Message::Answer {
            message_id: "demo-0-2".into(),
            kind: AnswerKind::Reply,
        },
    ] {
        let effects = app.update(asked);
        let compose = app.compose().unwrap();
        assert_eq!(compose.id(), draft, "the open message stays as it was");
        assert!(compose.asked_again());
        assert!(
            effects
                .into_iter()
                .any(|effect| matches!(effect, Effect::Ui(UiEffect::RaiseComposer)))
        );
        let _ = app.update(Message::ComposeChanged(ComposeField::Body, "More".into()));
        assert!(
            !app.compose().unwrap().asked_again(),
            "writing on answers it"
        );
    }

    // In the reading pane the draft is already in view.
    let _ = app.update(Message::SetComposePlacement(ComposePlacement::ReadingPane));
    let effects = app.update(Message::OpenCompose);
    assert!(app.compose().unwrap().asked_again());
    assert_eq!(effects.units(), 0);
}

#[test]
fn picking_compose_attachments_emits_ui_effect() {
    let mut app = loaded_demo_app();
    // When compose is closed, picking does nothing
    let effects = app.update(Message::PickComposeAttachments);
    assert_eq!(effects.units(), 0);

    let _ = app.update(Message::OpenCompose);
    let compose_id = app.compose().unwrap().id();
    let effects = app.update(Message::PickComposeAttachments);
    assert_eq!(effects.units(), 1);
    let effect = effects.into_iter().next().unwrap();
    assert!(matches!(
        effect,
        Effect::Ui(UiEffect::PickComposeAttachments(id)) if id == compose_id
    ));
}

#[test]
fn compose_attachments_can_be_added_and_removed_through_messages() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::OpenCompose);
    let compose_id = app.compose().unwrap().id();

    let file1 = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let file2 = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("README.md");

    let _ = app.update(Message::AddComposeAttachments(
        compose_id,
        vec![file1.clone(), file2.clone()],
    ));
    let compose = app.compose().unwrap();
    assert_eq!(compose.attachments(), &[file1, file2.clone()]);
    assert!(!compose.is_untouched());

    let _ = app.update(Message::RemoveComposeAttachment(0));
    let compose = app.compose().unwrap();
    assert_eq!(compose.attachments(), &[file2]);
}

#[tokio::test]
async fn attached_files_are_measured_away_from_the_interface() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::OpenCompose);
    let draft = app.compose().unwrap().id();
    let file = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let length = std::fs::metadata(&file).unwrap().len();

    let measure = app.update(Message::AddComposeAttachments(draft, vec![file.clone()]));
    assert_eq!(app.compose().unwrap().attachment_size(&file), None);
    for effect in measure.into_iter() {
        let Effect::Future(future) = effect else {
            panic!("the sizes are read by a future");
        };
        let _ = app.update(future.await);
    }
    assert_eq!(app.compose().unwrap().attachment_size(&file), Some(length));
    // A file taken off forgets its size, which may differ when it comes back.
    let _ = app.update(Message::RemoveComposeAttachment(0));
    assert_eq!(app.compose().unwrap().attachment_size(&file), None);

    // Sizes read for a draft that is gone change nothing in the next one.
    let _ = app.update(Message::DiscardCompose);
    let _ = app.update(Message::OpenCompose);
    let _ = app.update(Message::AddComposeAttachments(
        app.compose().unwrap().id(),
        vec![file.clone()],
    ));
    let _ = app.update(Message::AttachmentSizesRead(
        draft,
        [(file.clone(), Some(1))].into_iter().collect(),
    ));
    assert_eq!(app.compose().unwrap().attachment_size(&file), None);
}

#[test]
fn attachments_from_discarded_compose_are_ignored_by_new_compose() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::OpenCompose);
    let first_id = app.compose().unwrap().id();

    let effects = app.update(Message::PickComposeAttachments);
    assert_eq!(effects.units(), 1);
    let effect = effects.into_iter().next().unwrap();
    assert!(matches!(
        effect,
        Effect::Ui(UiEffect::PickComposeAttachments(id)) if id == first_id
    ));

    // Discard the first compose draft and open a second one
    let _ = app.update(Message::DiscardCompose);
    assert!(app.compose().is_none());

    let _ = app.update(Message::OpenCompose);
    let second_id = app.compose().unwrap().id();
    assert_ne!(first_id, second_id);

    // If the file dialog for first_id finishes now, its attachments must be ignored
    let file = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let _ = app.update(Message::AddComposeAttachments(first_id, vec![file.clone()]));

    let compose = app.compose().unwrap();
    assert!(compose.attachments().is_empty());
    assert!(compose.is_untouched());

    // But attachments targeting second_id are accepted
    let _ = app.update(Message::AddComposeAttachments(
        second_id,
        vec![file.clone()],
    ));
    let compose = app.compose().unwrap();
    assert_eq!(compose.attachments(), &[file]);
}

#[test]
fn a_message_is_written_sent_and_put_away() {
    let mut app = loaded_demo_app();
    write(&mut app, "alex@example.com, sam@example.org");

    // Handing it over is work, and the window says so meanwhile.
    let task = app.update(Message::Send);
    assert_eq!(task.units(), 1);
    assert_eq!(app.compose().map(Compose::sending), Some(Sending::InFlight));

    deliver_send(&mut app, Ok(()));
    assert!(
        app.compose().is_none(),
        "the window stays open after sending"
    );
}

#[test]
fn send_result_from_a_previous_session_keeps_the_new_draft() {
    let mut app = loaded_demo_app();
    let previous_epoch = app.session_epoch;
    write(&mut app, "old@example.com");
    let _ = app.update(Message::Send);
    // Signing out waits for the send; an expired session cannot.
    app.close_mailbox(Some(AuthError::SessionExpired));

    let _ = app.open_mailbox(MailBackend::demo(), None, None);
    write(&mut app, "new@example.com");
    let _ = app.update(Message::Sent(previous_epoch, Ok(())));

    let compose = app.compose().expect("the new session keeps its draft");
    assert_eq!(compose.field(ComposeField::To), "new@example.com");
}

#[test]
fn signing_out_never_takes_a_written_or_sending_message_with_it() {
    let mut app = loaded_demo_app();
    write(&mut app, "alex@example.com");

    // The desktop asks first and discards the message itself.
    assert_eq!(app.update(Message::Logout).units(), 0);
    assert!(matches!(app.auth_state, AuthState::Authenticated { .. }));
    assert!(app.compose().is_some());

    // A send's outcome must reach this session.
    let _ = app.update(Message::Send);
    assert_eq!(app.update(Message::Logout).units(), 0);
    assert!(matches!(app.auth_state, AuthState::Authenticated { .. }));
    assert!(app.compose().unwrap().in_flight());

    deliver_send(&mut app, Ok(()));
    let _ = app.update(Message::Logout);
    assert_eq!(app.auth_state, AuthState::SignedOut);
}

#[test]
fn sending_revalidates_a_cached_sent_folder() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::SelectFolder(sys(MailFolder::Sent)));
    deliver_latest_demo_page(&mut app, 0);
    assert_eq!(
        app.update(Message::SelectFolder(sys(MailFolder::Inbox)))
            .units(),
        0
    );

    write(&mut app, "alex@example.com");
    let _ = app.update(Message::Send);
    deliver_send(&mut app, Ok(()));

    assert_eq!(
        app.update(Message::SelectFolder(sys(MailFolder::Sent)))
            .units(),
        1
    );
    assert!(matches!(
        app.mailbox().unwrap().status(),
        ListStatus::Refreshing(_)
    ));
    assert!(!app.mailbox().unwrap().conversations().is_empty());
}

#[test]
fn a_message_with_nowhere_to_go_does_not_leave() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::OpenCompose);
    let _ = app.update(Message::ComposeChanged(
        ComposeField::Body,
        "Ready to go".to_owned(),
    ));

    // Pressing Send is not an instruction to guess.
    assert_eq!(app.update(Message::Send).units(), 0);
    assert_eq!(app.compose().map(Compose::sending), Some(Sending::Writing));
}

#[test]
fn a_refusal_keeps_the_message_and_says_why() {
    let mut app = loaded_demo_app();
    write(&mut app, "alex@example.com");
    let _ = app.update(Message::Send);

    deliver_send(&mut app, Err(SendError::DemoLimitReached));

    let writing = app.compose().expect("the message is still here");
    assert_eq!(
        writing.sending(),
        Sending::Failed(SendError::DemoLimitReached)
    );
    // Nothing typed is lost, so it can be sent again once the cause is gone.
    assert_eq!(writing.field(ComposeField::Body), "See you then.");
}

#[test]
fn a_message_already_gone_cannot_be_sent_twice_or_dismissed() {
    // Once it is out of the sender's hands, the window is a report, not a
    // form: a second press must not put a second copy on the wire.
    let mut app = loaded_demo_app();
    write(&mut app, "alex@example.com");
    let _ = app.update(Message::Send);

    assert_eq!(app.update(Message::Send).units(), 0);
    let _ = app.update(Message::CloseCompose);
    assert_eq!(app.compose().map(Compose::sending), Some(Sending::InFlight));
    press(&mut app, Key::Escape);
    assert!(app.compose().is_some(), "escape dismissed a sent message");
}

#[test]
fn escape_puts_an_unsent_message_away() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::OpenCompose);
    press(&mut app, Key::Escape);
    assert!(
        app.compose().is_none(),
        "untouched compose is put away immediately"
    );

    write(&mut app, "alex@example.com");
    press(&mut app, Key::Escape);
    assert!(
        app.compose().is_some_and(|c| c.confirming_discard()),
        "written compose asks for confirmation before being put away"
    );
    let _ = app.update(Message::DiscardCompose);
    assert!(
        app.compose().is_none(),
        "confirming discard puts message away"
    );
}

#[test]
fn a_reply_looks_up_its_recipients_as_it_opens_but_a_forward_has_none() {
    let answer = |kind: AnswerKind, check: bool| {
        let mut app = loaded_demo_app();
        let _ = app.update(Message::SetCheckRecipientProtection(check));
        open_conversation(&mut app, "demo-0");
        let effects = app.update(Message::Answer {
            message_id: "demo-0-2".into(),
            kind,
        });
        (app, effects.units())
    };
    // Its notice shows without leaving a field, and nothing is asked twice.
    let (mut app, asked) = answer(AnswerKind::ReplyAll, true);
    assert_eq!(asked, 1);
    assert_eq!(
        app.compose().unwrap().protection_notice(),
        Some(compose::ProtectionNotice::Checking)
    );
    assert_eq!(app.update(Message::ComposeRecipientsLeft).units(), 0);
    // Off in Settings, nothing is asked before sending.
    assert_eq!(answer(AnswerKind::Reply, false).1, 0);
    assert_eq!(answer(AnswerKind::Forward, true).1, 0);
}

#[test]
fn a_reply_opens_addressed_and_its_recipients_can_change() {
    // A reply opens addressed to whom core would answer, Cc included for
    // Reply all, and like any message it goes to whoever is typed.
    let mut app = loaded_demo_app();
    open_conversation(&mut app, "demo-0");

    let _ = app.update(Message::Answer {
        message_id: "demo-0-2".into(),
        kind: AnswerKind::ReplyAll,
    });

    let writing = app.compose().expect("a reply is open");
    let answering = writing.answering().expect("it answers a message").clone();
    assert!(!answering.to.is_empty());
    assert_eq!(writing.field(ComposeField::To), answering.to.join(", "));
    assert_eq!(writing.field(ComposeField::Cc), answering.cc.join(", "));
    assert_eq!(writing.showing_copies(), !answering.cc.is_empty());
    assert!(!writing.asks_for_subject());
    assert!(writing.is_untouched(), "opening addressed is not writing");

    let _ = app.update(Message::ComposeChanged(ComposeField::To, String::new()));
    let _ = app.update(Message::ComposeChanged(ComposeField::Cc, String::new()));
    assert_eq!(
        app.compose().unwrap().not_ready(),
        Some(compose::NotReady::NoRecipients)
    );
    assert_eq!(app.update(Message::Send).units(), 0, "it left unaddressed");

    let _ = app.update(Message::ComposeChanged(
        ComposeField::To,
        "carol@example.com".into(),
    ));
    assert!(!app.compose().unwrap().is_untouched());
    assert_eq!(app.update(Message::Send).units(), 1);
}

#[test]
fn an_answer_quotes_the_original_unless_told_not_to() {
    let mut app = loaded_demo_app();
    open_conversation(&mut app, "demo-0");
    let _ = app.update(Message::Answer {
        message_id: "demo-0-2".into(),
        kind: AnswerKind::Reply,
    });
    assert_eq!(
        app.compose().unwrap().quote(),
        Some(ruston_core::Quote::Original)
    );

    let _ = app.update(Message::ToggleComposeQuote);
    let writing = app.compose().unwrap();
    assert_eq!(writing.quote(), Some(ruston_core::Quote::Omitted));
    assert!(
        writing.is_untouched(),
        "leaving the quote out is not writing"
    );
    let _ = app.update(Message::ToggleComposeQuote);
    assert_eq!(
        app.compose().unwrap().quote(),
        Some(ruston_core::Quote::Original)
    );

    // A new message answers nothing.
    let _ = app.update(Message::CloseCompose);
    let _ = app.update(Message::OpenCompose);
    let _ = app.update(Message::ToggleComposeQuote);
    assert_eq!(app.compose().unwrap().quote(), None);
}

#[test]
fn a_forward_still_has_to_be_addressed() {
    let mut app = loaded_demo_app();
    open_conversation(&mut app, "demo-0");

    let _ = app.update(Message::Answer {
        message_id: "demo-0-2".into(),
        kind: AnswerKind::Forward,
    });

    let writing = app.compose().expect("a forward is open");
    assert!(writing.field(ComposeField::To).is_empty());
    // Core writes the subject, so the window does not ask for one.
    assert!(!writing.asks_for_subject());
    assert_eq!(app.update(Message::Send).units(), 0, "it left unaddressed");

    let _ = app.update(Message::ComposeChanged(
        ComposeField::To,
        "alex@example.com".into(),
    ));
    assert_eq!(app.update(Message::Send).units(), 1);
}

#[test]
fn answering_says_what_is_being_answered() {
    let mut app = loaded_demo_app();
    open_conversation(&mut app, "demo-0");

    let _ = app.update(Message::Answer {
        message_id: "demo-0-2".into(),
        kind: AnswerKind::ReplyAll,
    });

    let answering = app
        .compose()
        .and_then(Compose::answering)
        .expect("it says what it answers");
    assert_eq!(answering.kind, AnswerKind::ReplyAll);
    assert!(!answering.subject.is_empty());
    assert!(!answering.sender.is_empty());
}

#[test]
fn answering_a_message_that_is_not_there_opens_nothing() {
    let mut app = loaded_demo_app();

    let _ = app.update(Message::Answer {
        message_id: "demo-0-2".into(),
        kind: AnswerKind::Reply,
    });

    assert!(app.compose().is_none());
}

#[tokio::test]
async fn sending_with_missing_attachment_fails_gracefully_in_background() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::OpenCompose);
    let _ = app.update(Message::ComposeChanged(
        ComposeField::To,
        "alex@example.com".into(),
    ));
    let _ = app.update(Message::ComposeChanged(ComposeField::Body, "Hello".into()));
    let non_existent = PathBuf::from("/tmp/missing_attachment_test_ruston.pdf");
    let compose_id = app.compose().unwrap().id();
    let _ = app.update(Message::AddComposeAttachments(
        compose_id,
        vec![non_existent],
    ));

    // In-memory UI readiness must be immediate and non-blocking
    assert!(app.compose().unwrap().not_ready().is_none());

    // User triggers Send
    let effects = app.update(Message::Send);
    assert_eq!(effects.units(), 1);
    assert!(app.compose().unwrap().in_flight());

    // Execute background effect
    let effect = effects.into_iter().next().unwrap();
    let Effect::Future(future) = effect else {
        panic!("expected future effect");
    };
    let message = future.await;
    let _ = app.update(message);

    // Compose should transition to Failed with MissingAttachment, preserving draft content
    let sending = app.compose().unwrap().sending();
    assert_eq!(
        sending,
        Sending::Failed(SendError::MissingAttachment(
            "missing_attachment_test_ruston.pdf".to_owned()
        ))
    );
    assert_eq!(app.compose().unwrap().field(ComposeField::Body), "Hello");

    // Removing the missing attachment clears the refusal state back to Writing
    let _ = app.update(Message::RemoveComposeAttachment(0));
    assert_eq!(app.compose().unwrap().sending(), Sending::Writing);
    assert!(app.compose().unwrap().attachments().is_empty());
}
