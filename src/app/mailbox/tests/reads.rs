//! Marking an opened conversation read, and the changes that wait for it.

use super::*;

fn opened_unread(ids: &[&str], opened: &str) -> Mailbox {
    let (mut mailbox, request) = open();
    // Fixtures issue IDs from 80, clear of the 3, 4 and on that tests issue.
    let rows = ids.iter().map(|id| unread(id)).collect();
    mailbox.finish_page(request.id, page_of(rows, ids.len() as u32));
    let load = mailbox.start_conversation_load(opened.into(), 82).unwrap();
    mailbox.finish_conversation(&load, Ok(detail(opened, &["message"])));
    mailbox
}

#[test]
fn a_thread_split_while_open_is_still_marked_read() {
    let (mut mailbox, request) = open();
    mailbox.finish_page(
        request.id,
        Ok(ConversationPage {
            conversations: vec![
                ConversationSummary {
                    unread: true,
                    ..dated("c1", 50)
                },
                dated("c2", 20),
            ],
            total: 2,
            inspect_candidates: vec!["c1".into()],
        }),
    );
    load_detail(&mut mailbox, "c1", detail("c1", &["m1", "m2"]), 3);
    // The inspection answers once the thread is open: its messages take
    // its row in the list.
    mailbox.split_conversation(
        &Folder::INBOX,
        "c1",
        vec![
            ConversationSummary {
                unread: true,
                ..split_message("m1", 60)
            },
            ConversationSummary {
                unread: true,
                ..split_message("m2", 40)
            },
        ],
    );

    let read = mailbox
        .start_mark_read(4)
        .expect("the open thread is still read");
    assert_eq!(read.row_id, "c1");
    // Its messages are listed here, so the folder is still the context.
    assert_eq!(read.context.as_ref(), Some(&Folder::INBOX));
    assert!(mailbox.finish_mark_read(&read, Ok(())).read_confirmed);
    for message in ["m1", "m2"] {
        assert!(!mailbox.row(message).unwrap().unread, "{message}");
    }
    assert!(mailbox.row("c2").is_some());
}

#[test]
fn opened_unread_rows_are_marked_read_once_confirmed() {
    let mut mailbox = opened_unread(&["a", "b"], "a");

    let request = mailbox.start_mark_read(3).unwrap();
    assert_eq!(request.row_id, "a");
    assert_eq!(request.action, MailAction::SetUnread(false));
    assert_eq!(mailbox.start_mark_read(4), None);
    assert!(mailbox.selected_summary().unwrap().unread);

    assert!(mailbox.finish_mark_read(&request, Ok(())).read_confirmed);
    assert!(!mailbox.selected_summary().unwrap().unread);
    assert_eq!(mailbox.start_mark_read(5), None);
}

#[test]
fn reads_wait_for_the_content_to_load() {
    let (mut mailbox, request) = open();
    mailbox.finish_page(request.id, page_of(vec![unread("a")], 1));
    let load = mailbox.start_conversation_load("a".into(), 3).unwrap();

    assert_eq!(mailbox.start_mark_read(4), None);

    mailbox.finish_conversation(&load, Err(MailboxError::Connection));
    assert_eq!(mailbox.start_mark_read(5), None);
}

#[test]
fn failed_or_stale_reads_leave_the_row_unread() {
    let mut mailbox = opened_unread(&["a"], "a");
    let request = mailbox.start_mark_read(3).unwrap();
    let stale = ActionRequest {
        id: NEVER_ISSUED,
        ..request.clone()
    };

    assert_eq!(
        mailbox.finish_mark_read(&stale, Ok(())),
        ReadCompletion::default()
    );
    assert_eq!(
        mailbox
            .finish_mark_read(&request, Err(MailboxError::Connection))
            .error,
        Some(MailboxError::Connection)
    );
    assert!(mailbox.selected_summary().unwrap().unread);
    assert!(mailbox.start_mark_read(4).is_some());
}

#[test]
fn explicit_read_mutations_wait_for_the_automatic_read() {
    for unread in [true, false] {
        let mut mailbox = opened_unread(&["a"], "a");
        let _read = mailbox.start_mark_read(3).unwrap();

        assert_eq!(
            mailbox.start_action(MailAction::SetUnread(unread), 4),
            Some(ActionStart::default()),
            "{unread}"
        );
        assert!(
            mailbox.action_pending(),
            "the user's action stays queued: {unread}"
        );
        assert_eq!(mailbox.start_mark_read(5), None, "{unread}");
    }
}

#[test]
fn automatic_reads_wait_for_explicit_read_mutations_on_the_same_row() {
    let mut mailbox = opened_unread(&["a", "b"], "a");
    mailbox
        .start_action(MailAction::SetUnread(true), 3)
        .unwrap();

    assert_eq!(mailbox.start_mark_read(4), None);
    let load = mailbox.start_conversation_load("b".into(), 5).unwrap();
    mailbox.finish_conversation(&load, Ok(detail("b", &["message-b"])));
    assert!(
        mailbox.start_mark_read(6).is_some(),
        "another row remains independent"
    );
}

#[test]
fn failed_automatic_reads_fail_the_queued_action_without_sending_it() {
    for error in [
        MailboxError::Connection,
        MailboxError::Service,
        MailboxError::SessionExpired,
    ] {
        let mut mailbox = opened_unread(&["a"], "a");
        let read = mailbox.start_mark_read(3).unwrap();
        assert_eq!(
            mailbox.start_action(MailAction::SetUnread(true), 4),
            Some(ActionStart::default())
        );

        assert_eq!(
            mailbox.finish_mark_read(&read, Err(error)).error,
            Some(error)
        );
        assert!(!mailbox.action_pending(), "{error:?}");
        assert_eq!(mailbox.action_error(), Some(error));
        assert!(mailbox.selected_summary().unwrap().unread, "{error:?}");
        assert_eq!(
            mailbox.finish_mark_read(&read, Ok(())),
            ReadCompletion::default(),
            "{error:?}"
        );
    }
}

#[test]
fn a_read_change_queued_behind_another_action_still_waits_for_the_automatic_read() {
    let mut mailbox = opened_unread(&["a"], "a");
    let read = mailbox.start_mark_read(3).unwrap();
    // A star does not wait for the read; the explicit unread queues behind it.
    let star = send(&mut mailbox, MailAction::SetStarred(true), 4);
    assert_eq!(
        mailbox.start_action(MailAction::SetUnread(true), 5),
        Some(ActionStart::default())
    );

    // Once the star answers, the unread is next in line but still waits.
    assert_eq!(mailbox.finish_action(&star, Ok(())).next_action, None);
    assert!(mailbox.action_pending());
    let completion = mailbox.finish_mark_read(&read, Ok(()));
    assert!(completion.read_confirmed);
    assert_eq!(completion.next_action.map(|action| action.id), Some(5));
}

#[test]
fn a_read_change_queued_behind_another_action_waits_for_it_after_the_read() {
    let mut mailbox = opened_unread(&["a"], "a");
    let read = mailbox.start_mark_read(3).unwrap();
    let star = send(&mut mailbox, MailAction::SetStarred(true), 4);
    mailbox
        .start_action(MailAction::SetUnread(true), 5)
        .unwrap();

    // The read answers first: the unread still waits for the star.
    assert_eq!(mailbox.finish_mark_read(&read, Ok(())).next_action, None);
    let next = mailbox.finish_action(&star, Ok(())).next_action.unwrap();
    assert_eq!(next.id, 5);
}

#[test]
fn a_failed_automatic_read_still_sends_the_action_behind_the_one_it_failed() {
    let mut mailbox = opened_unread(&["a", "b"], "a");
    let read = mailbox.start_mark_read(3).unwrap();
    assert_eq!(
        mailbox.start_action(MailAction::SetUnread(true), 4),
        Some(ActionStart::default())
    );
    let star = mailbox
        .start_action_on(
            "b".into(),
            SummaryKind::Conversation,
            MailAction::SetStarred(true),
            5,
        )
        .unwrap();
    assert_eq!(star.send, None);

    let completion = mailbox.finish_mark_read(&read, Err(MailboxError::Connection));
    assert_eq!(completion.error, Some(MailboxError::Connection));
    // The unread fails with the read, and the star of the other row goes.
    let next = completion.next_action.unwrap();
    assert_eq!((next.id, next.row_id.as_str()), (5, "b"));
}

#[test]
fn a_failed_explicit_unread_keeps_the_confirmed_automatic_read() {
    let mut mailbox = opened_unread(&["a"], "a");
    let read = mailbox.start_mark_read(3).unwrap();
    assert_eq!(
        mailbox.start_action(MailAction::SetUnread(true), 4),
        Some(ActionStart::default())
    );
    let action = mailbox.finish_mark_read(&read, Ok(())).next_action.unwrap();

    assert_eq!(
        mailbox
            .finish_action(&action, Err(MailboxError::Connection))
            .result,
        Err(MailboxError::Connection)
    );
    assert!(!mailbox.selected_summary().unwrap().unread);
    assert_eq!(mailbox.action_error(), Some(MailboxError::Connection));
}

#[test]
fn only_the_current_read_of_the_same_row_releases_a_queued_action() {
    let mut mailbox = opened_unread(&["a", "b"], "a");
    let first = mailbox.start_mark_read(3).unwrap();
    let load = mailbox.start_conversation_load("b".into(), 4).unwrap();
    mailbox.finish_conversation(&load, Ok(detail("b", &["message-b"])));
    let second = mailbox.start_mark_read(5).unwrap();
    assert_eq!(
        mailbox.start_action(MailAction::SetUnread(true), 6),
        Some(ActionStart::default())
    );

    let stale = ActionRequest {
        id: NEVER_ISSUED,
        ..second.clone()
    };
    assert_eq!(
        mailbox.finish_mark_read(&stale, Ok(())),
        ReadCompletion::default()
    );
    assert!(
        mailbox
            .finish_mark_read(&first, Ok(()))
            .next_action
            .is_none()
    );
    let completion = mailbox.finish_mark_read(&second, Ok(()));
    assert_eq!(completion.next_action.unwrap().id, 6);
    assert_eq!(
        mailbox.finish_mark_read(&second, Ok(())),
        ReadCompletion::default()
    );
}

#[test]
fn queued_read_actions_keep_their_original_folder_after_navigation() {
    let mut mailbox = opened_unread(&["a"], "a");
    let read = mailbox.start_mark_read(3).unwrap();
    assert_eq!(
        mailbox.start_action(MailAction::SetUnread(true), 4),
        Some(ActionStart::default())
    );
    mailbox.select_folder(sys(MailFolder::Sent), 5);
    mailbox.finish_page(5, page(&["b"], 1));

    let completion = mailbox.finish_mark_read(&read, Ok(()));
    assert!(completion.read_confirmed, "Proton's counts changed");
    let action = completion.next_action.unwrap();
    assert_eq!(action.context, Some(Folder::INBOX));
    assert_eq!(action.folder, Folder::INBOX);
    assert_eq!(mailbox.finish_action(&action, Ok(())).result, Ok(()));
    assert_eq!(ids(&mailbox), ["b"]);
}

#[test]
fn unrelated_actions_do_not_wait_for_automatic_reads() {
    let mut mailbox = opened_unread(&["a"], "a");
    let read = mailbox.start_mark_read(3).unwrap();
    let star = send(&mut mailbox, MailAction::SetStarred(true), 4);
    assert!(
        mailbox
            .finish_mark_read(&read, Ok(()))
            .next_action
            .is_none()
    );
    assert_eq!(mailbox.finish_action(&star, Ok(())).result, Ok(()));
    assert!(mailbox.selected_summary().unwrap().starred);
}

#[test]
fn explicit_unread_wins_over_an_automatic_read() {
    // A conversation row and a message split out of one wait alike: the
    // explicit request is held until the automatic read is confirmed.
    for kind in [SummaryKind::Conversation, SummaryKind::Message] {
        let mut mailbox = opened_unread(&["a"], "a");
        mailbox.conversations[0].kind = kind;
        let read = mailbox.start_mark_read(3).unwrap();
        assert_eq!(
            mailbox.start_action(MailAction::SetUnread(true), 4),
            Some(ActionStart::default())
        );

        let completion = mailbox.finish_mark_read(&read, Ok(()));
        assert!(completion.read_confirmed, "{kind:?}");
        assert!(!mailbox.selected_summary().unwrap().unread, "{kind:?}");
        let unread = completion
            .next_action
            .expect("the explicit action is now ready");
        assert_eq!(unread.id, 4, "{kind:?}");
        assert_eq!(unread.action, MailAction::SetUnread(true), "{kind:?}");
        assert_eq!(
            mailbox.finish_action(&unread, Ok(())).result,
            Ok(()),
            "{kind:?}"
        );
        assert!(mailbox.selected_summary().unwrap().unread, "{kind:?}");
        assert_eq!(
            mailbox.finish_mark_read(&read, Ok(())),
            ReadCompletion::default(),
            "{kind:?}"
        );
    }
}

#[test]
fn reads_after_a_folder_switch_change_only_the_counts() {
    let mut mailbox = opened_unread(&["a"], "a");
    let request = mailbox.start_mark_read(3).unwrap();
    mailbox.select_folder(sys(MailFolder::Sent), 4);
    mailbox.finish_page(4, page_of(vec![unread("a")], 1));

    // The rows of the folder now open stay as Proton listed them, but the
    // counts the read changed are reloaded.
    assert_eq!(
        mailbox.finish_mark_read(&request, Ok(())),
        ReadCompletion {
            read_confirmed: true,
            ..ReadCompletion::default()
        }
    );
    assert!(mailbox.conversations()[0].unread);

    let refresh = mailbox
        .select_folder(sys(MailFolder::Inbox), 5)
        .expect("the cached source folder was invalidated");
    assert_eq!(mailbox.status(), ListStatus::Refreshing(refresh.id));
    assert_eq!(ids(&mailbox), ["a"]);
}

#[test]
fn automatic_refresh_waits_for_pending_mail_changes() {
    let mut mailbox = opened_unread(&["a"], "a");
    assert!(mailbox.auto_refresh_available());

    let read = mailbox.start_mark_read(3).unwrap();
    assert!(!mailbox.auto_refresh_available());
    assert!(mailbox.finish_mark_read(&read, Ok(())).read_confirmed);

    let action = send(&mut mailbox, MailAction::SetStarred(true), 4);
    assert!(!mailbox.auto_refresh_available());
    assert_eq!(mailbox.finish_action(&action, Ok(())).result, Ok(()));
    assert!(mailbox.auto_refresh_available());
}

#[test]
fn a_read_confirmed_while_the_list_refreshes_stays_shown() {
    let (mut mailbox, first) = open();
    mailbox.finish_page(first.id, page_of(vec![unread("a"), unread("b")], 2));
    load_detail(&mut mailbox, "a", detail("a", &["message"]), 3);
    let read = mailbox.start_mark_read(4).expect("the open row is unread");
    let refresh = mailbox.refresh(5).unwrap();
    assert!(mailbox.finish_mark_read(&read, Ok(())).read_confirmed);
    assert!(!mailbox.row("a").unwrap().unread);

    // Proton read the page before it marked "a" read.
    mailbox.finish_page(refresh.id, page_of(vec![unread("a"), unread("b")], 2));
    assert!(!mailbox.row("a").unwrap().unread, "it stays read");
    assert!(mailbox.row("b").unwrap().unread);
}
