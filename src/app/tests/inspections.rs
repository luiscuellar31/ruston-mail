//! Thread inspections: how many run, which answers count, and what they split.

use super::*;

/// The request the inspection of `conversation_id` runs under.
fn running_inspection(app: &App, conversation_id: &str) -> RequestId {
    app.inspections
        .running_request(conversation_id)
        .expect("an inspection running")
}

fn inspection_page(app: &App, count: usize) -> ConversationPage {
    let template = app
        .mailbox()
        .unwrap()
        .visible_conversations()
        .next()
        .unwrap();
    let conversations: Vec<_> = (0..count)
        .map(|i| ConversationSummary {
            id: format!("candidate-{i}"),
            ..template.clone()
        })
        .collect();
    ConversationPage {
        inspect_candidates: conversations.iter().map(|row| row.id.clone()).collect(),
        conversations,
        total: count as u32,
    }
}

#[test]
fn inspection_candidates_do_not_spawn_an_unbounded_number_of_tasks() {
    let mut app = loaded_demo_app();
    // Twice as many candidates as the queue takes, each offered twice.
    let mut page = inspection_page(&app, 2 * inspection::CAPACITY);
    page.inspect_candidates
        .extend(page.inspect_candidates.clone());
    let _ = app.update(Message::RefreshMailbox);
    let request = pending_list_request(&app);
    let effects = app.update(Message::ConversationsLoaded(request, Ok(page)));
    assert_eq!(effects.units(), INSPECTION_CONCURRENCY);
}

#[test]
fn stale_page_response_does_not_start_inspections() {
    let mut app = loaded_demo_app();
    let page = inspection_page(&app, 2);
    let effects = app.update(Message::ConversationsLoaded(NEVER_ISSUED, Ok(page)));
    assert_eq!(effects.units(), 0);
}

#[test]
fn inspection_queue_drains_with_bounded_deduplicated_work() {
    let mut app = loaded_demo_app();
    // Twice as many candidates as the queue takes, each offered twice.
    let mut page = inspection_page(&app, 2 * inspection::CAPACITY);
    page.inspect_candidates
        .extend(page.inspect_candidates.clone());
    let _ = app.update(Message::RefreshMailbox);
    let request = pending_list_request(&app);
    let effects = app.update(Message::ConversationsLoaded(request, Ok(page)));
    let mut running: std::collections::VecDeque<_> = effects.into_iter().collect();
    let mut completed = std::collections::HashSet::new();
    while let Some(effect) = running.pop_front() {
        let Effect::Future(future) = effect else {
            panic!("expected an inspection")
        };
        let message = futures::executor::block_on(future);
        let Message::ThreadInspected {
            conversation_id, ..
        } = &message
        else {
            panic!("expected an inspection result");
        };
        assert!(
            completed.insert(conversation_id.clone()),
            "duplicate candidate"
        );
        let next = app.update(message.clone());
        assert!(next.units() <= 1);
        // A duplicate completion cannot release another slot or launch work.
        assert_eq!(app.update(message).units(), 0);
        running.extend(next.into_iter());
        assert!(running.len() <= INSPECTION_CONCURRENCY);
    }
    assert_eq!(
        completed.len(),
        inspection::CAPACITY,
        "overflow must keep Proton grouping"
    );
}

#[test]
fn inspections_are_cancelled_when_the_context_changes() {
    for (case, transition) in [
        (
            "another folder",
            Message::SelectFolder(sys(MailFolder::Sent)),
        ),
        ("a search", Message::SearchSubmitted),
        ("a refresh", Message::RefreshMailbox),
        ("signing out", Message::Logout),
    ] {
        let mut app = loaded_demo_app();
        let effects = app.inspect_candidates(vec!["demo-0".into()]);
        let request = running_inspection(&app, "demo-0");
        let Effect::Future(future) = effects.into_iter().next().unwrap() else {
            panic!("expected an inspection");
        };
        if matches!(transition, Message::SearchSubmitted) {
            let _ = app.update(Message::SearchChanged("mail".into()));
        }
        let _ = app.update(transition);
        let mut message = futures::executor::block_on(future);
        // Even a success or session error already queued before cancellation
        // must be discarded, without starting the next candidate.
        let Message::ThreadInspected { result, .. } = &mut message else {
            panic!("expected an inspection result");
        };
        *result = Err(MailboxError::SessionExpired);
        let expected_auth = app.auth_state.clone();
        assert_eq!(app.update(message).units(), 0, "{case}");
        assert_eq!(app.auth_state, expected_auth, "{case}");
        assert!(!app.inspections.finish(request, "demo-0"), "{case}");
    }
}

#[test]
fn an_inspection_asked_before_a_rename_still_frees_its_place() {
    let mut app = loaded_demo_app();
    let invoices = Folder::custom("kZ9", "Invoices");
    let _ = app.update(Message::SelectFolder(invoices.clone()));
    let page = ConversationPage {
        conversations: vec![ConversationSummary {
            id: "thread-1".into(),
            kind: crate::mail::SummaryKind::Conversation,
            subject: Some("Invoice".into()),
            correspondents: None,
            participants: Vec::new(),
            preview: None,
            time: Some(100),
            unread: false,
            starred: false,
            message_count: 2,
            has_attachments: false,
        }],
        total: 1,
        inspect_candidates: vec!["thread-1".into()],
    };
    let started = app.update(Message::ConversationsLoaded(
        pending_list_request(&app),
        Ok(page),
    ));
    assert_eq!(started.units(), 1, "the thread is inspected");
    let request = running_inspection(&app, "thread-1");
    deliver_folders(&mut app, Ok(vec![Folder::custom("kZ9", "Facturas")]));

    // The answer names the folder as it was called when it was asked.
    let _ = app.update(Message::ThreadInspected {
        revision: app.mailbox().map_or(0, Mailbox::inspection_revision),
        epoch: app.session_epoch,
        request,
        folder: invoices,
        conversation_id: "thread-1".into(),
        result: Ok(None),
    });
    assert!(
        !app.inspections.finish(request, "thread-1"),
        "the renamed folder's inspection kept its place"
    );
}

#[test]
fn returning_to_a_folder_does_not_accept_its_cancelled_inspection() {
    let mut app = loaded_demo_app();
    let _ = app.inspect_candidates(vec!["demo-0".into()]);
    let old_request = running_inspection(&app, "demo-0");
    let _ = app.update(Message::SelectFolder(sys(MailFolder::Sent)));
    let _ = app.update(Message::SelectFolder(Folder::INBOX));
    let _ = app.inspect_candidates(vec!["demo-0".into()]);
    let current_request = running_inspection(&app, "demo-0");
    let effects = app.update(Message::ThreadInspected {
        revision: app.mailbox().map_or(0, Mailbox::inspection_revision),
        epoch: app.session_epoch,
        request: old_request,
        folder: Folder::INBOX,
        conversation_id: "demo-0".into(),
        result: Err(MailboxError::SessionExpired),
    });
    assert_eq!(effects.units(), 0);
    assert!(matches!(app.auth_state, AuthState::Authenticated { .. }));
    assert!(app.inspections.finish(current_request, "demo-0"));
}

#[test]
fn conversations_loaded_with_candidates_starts_inspection_effects() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::RefreshMailbox);
    let request = pending_list_request(&app);

    let page = ConversationPage {
        conversations: vec![ConversationSummary {
            id: "demo-cand-1".into(),
            kind: crate::mail::SummaryKind::Conversation,
            subject: Some("Candidate 1".into()),
            correspondents: None,
            participants: Vec::new(),
            preview: None,
            time: Some(100),
            unread: false,
            starred: false,
            message_count: 2,
            has_attachments: false,
        }],
        total: 1,
        inspect_candidates: vec!["demo-cand-1".into(), "demo-cand-2".into()],
    };

    let effects = app.update(Message::ConversationsLoaded(request, Ok(page)));
    // Only a candidate present in the accepted listing may be inspected.
    assert_eq!(effects.units(), 1);
}

#[test]
fn verified_split_conversations_can_be_reinspected_without_regrouping_rows() {
    let mut app = loaded_demo_app();
    let rows = vec![ConversationSummary {
        id: "split-member".into(),
        kind: crate::mail::SummaryKind::Message,
        ..app.mailbox().unwrap().visible_row(0).unwrap().clone()
    }];
    app.mailbox
        .as_mut()
        .unwrap()
        .split_conversation(&Folder::INBOX, "demo-0", rows);
    assert!(app.mailbox().unwrap().has_row("split-member"));
    assert!(!app.mailbox().unwrap().has_row("demo-0"));
    assert_eq!(app.inspect_candidates(vec!["demo-0".into()]).units(), 1);
    assert!(app.mailbox().unwrap().has_row("split-member"));
}

#[test]
fn inspection_before_a_confirmed_mutation_cannot_replace_rows_and_releases_its_slot() {
    let mut app = loaded_demo_app();
    let effects = app.inspect_candidates((0..5).map(|index| format!("demo-{index}")).collect());
    let Effect::Future(future) = effects.into_iter().next().unwrap() else {
        panic!("expected an inspection");
    };
    let mut message = futures::executor::block_on(future);
    let stale = ConversationSummary {
        id: "stale-member".into(),
        kind: crate::mail::SummaryKind::Message,
        ..app.mailbox().unwrap().visible_row(0).unwrap().clone()
    };
    let Message::ThreadInspected {
        result, revision, ..
    } = &mut message
    else {
        panic!("expected an inspection result");
    };
    *result = Ok(Some(vec![stale]));
    let previous = *revision;
    let mailbox = app.mailbox.as_mut().unwrap();
    mailbox.record_action("demo-0", &MailAction::SetUnread(false));
    mailbox.invalidate_listings();
    let current = mailbox.inspection_revision();
    assert!(current > previous);

    let next = app.update(message);
    assert!(app.mailbox().unwrap().has_row("demo-0"));
    assert!(!app.mailbox().unwrap().has_row("stale-member"));
    assert_eq!(
        next.units(),
        1,
        "the next candidate can use the released slot"
    );
    let Effect::Future(future) = next.into_iter().next().unwrap() else {
        panic!("expected the next inspection");
    };
    assert!(
        matches!(futures::executor::block_on(future), Message::ThreadInspected { revision, .. } if revision == current)
    );
}

#[test]
fn thread_inspected_splits_conversation_when_successful() {
    let mut app = loaded_demo_app();
    let epoch = app.session_epoch;
    let _ = app.inspect_candidates(vec!["demo-0".into()]);
    let request = running_inspection(&app, "demo-0");

    let split_rows = vec![
        ConversationSummary {
            id: "split-1".into(),
            kind: crate::mail::SummaryKind::Message,
            subject: Some("Split 1".into()),
            correspondents: None,
            participants: Vec::new(),
            preview: None,
            time: Some(150),
            unread: false,
            starred: false,
            message_count: 1,
            has_attachments: false,
        },
        ConversationSummary {
            id: "split-2".into(),
            kind: crate::mail::SummaryKind::Message,
            subject: Some("Split 2".into()),
            correspondents: None,
            participants: Vec::new(),
            preview: None,
            time: Some(140),
            unread: false,
            starred: false,
            message_count: 1,
            has_attachments: false,
        },
    ];

    let _ = app.update(Message::ThreadInspected {
        revision: app.mailbox().map_or(0, Mailbox::inspection_revision),
        epoch,
        request,
        folder: Folder::INBOX,
        conversation_id: "demo-0".into(),
        result: Ok(Some(split_rows)),
    });

    let visible: Vec<_> = app
        .mailbox()
        .unwrap()
        .visible_conversations()
        .map(|c| c.id.as_str())
        .collect();
    assert!(visible.contains(&"split-1"));
    assert!(visible.contains(&"split-2"));
    assert!(!visible.contains(&"demo-0"));
}

#[test]
fn thread_inspected_stale_epoch_or_folder_is_ignored() {
    let mut app = loaded_demo_app();
    let stale_epoch = app.session_epoch + 99;
    let _ = app.inspect_candidates(vec!["demo-0".into()]);
    let request = running_inspection(&app, "demo-0");

    let split_rows = vec![ConversationSummary {
        id: "split-ignored".into(),
        kind: crate::mail::SummaryKind::Message,
        subject: Some("Split Ignored".into()),
        correspondents: None,
        participants: Vec::new(),
        preview: None,
        time: Some(150),
        unread: false,
        starred: false,
        message_count: 1,
        has_attachments: false,
    }];

    // Wrong epoch
    let _ = app.update(Message::ThreadInspected {
        revision: app.mailbox().map_or(0, Mailbox::inspection_revision),
        epoch: stale_epoch,
        request,
        folder: Folder::INBOX,
        conversation_id: "demo-0".into(),
        result: Ok(Some(split_rows.clone())),
    });

    let visible: Vec<_> = app
        .mailbox()
        .unwrap()
        .visible_conversations()
        .map(|c| c.id.as_str())
        .collect();
    assert!(!visible.contains(&"split-ignored"));
    assert!(visible.contains(&"demo-0"));

    // Wrong folder
    let _ = app.update(Message::ThreadInspected {
        revision: app.mailbox().map_or(0, Mailbox::inspection_revision),
        epoch: app.session_epoch,
        request,
        folder: Folder::System(crate::mail::MailFolder::Trash),
        conversation_id: "demo-0".into(),
        result: Ok(Some(split_rows)),
    });

    let visible: Vec<_> = app
        .mailbox()
        .unwrap()
        .visible_conversations()
        .map(|c| c.id.as_str())
        .collect();
    assert!(!visible.contains(&"split-ignored"));
    assert!(visible.contains(&"demo-0"));
}
