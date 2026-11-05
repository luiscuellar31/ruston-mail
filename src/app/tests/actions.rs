//! Actions on conversations: moves, flags, undo, and what Proton refuses.

use super::*;

#[test]
fn a_rows_menu_acts_on_that_row_and_leaves_the_open_one_open() {
    let mut app = loaded_demo_app();
    open_conversation(&mut app, "demo-0");
    let listed = |app: &App, id: &str| app.mailbox().unwrap().has_row(id);
    let unread = |app: &App, id: &str| {
        app.mailbox()
            .unwrap()
            .conversations()
            .iter()
            .find(|row| row.id == id)
            .map(|row| row.unread)
    };

    let before = unread(&app, "demo-2").unwrap();
    let _ = app.update(Message::ApplyRowAction(
        "demo-2".into(),
        MailAction::SetUnread(!before),
    ));
    assert_eq!(unread(&app, "demo-2"), Some(!before));

    let _ = app.update(Message::ApplyRowAction(
        "demo-1".into(),
        MailAction::MoveTo(sys(MailFolder::Archive)),
    ));
    assert!(!listed(&app, "demo-1"));
    let mailbox = app.mailbox().unwrap();
    assert_eq!(mailbox.selected_conversation(), Some("demo-0"));
    let undo = mailbox.undo().expect("its folder is known");
    assert_eq!(undo.row_id, "demo-1");
    assert_eq!(undo.from, Folder::INBOX);
}

#[test]
fn an_undo_offer_that_expires_while_signing_out_still_goes() {
    let mut app = loaded_demo_app();
    open_conversation(&mut app, "demo-3");
    press_command(&mut app, Key::Backspace);
    let offer = app.mailbox().unwrap().undo().cloned().expect("an offer");

    // The list stays on screen while signing out, and so does the notice,
    // whose timer asks for it to go.
    app.set_auth_state(AuthState::SigningOut);
    let _ = app.update(Message::DismissUndo(offer));
    assert!(app.mailbox().unwrap().undo().is_none());
}

#[test]
fn demo_read_actions_update_row_and_count_idempotently() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::SearchChanged("design review".into()));
    open_conversation(&mut app, "demo-0");
    let detail = app.mailbox().unwrap().reader().unwrap().detail().clone();

    assert!(!app.mailbox().unwrap().conversations()[0].unread);
    assert_eq!(
        app.mailbox()
            .unwrap()
            .counts()
            .unwrap()
            .unread(&sys(MailFolder::Inbox)),
        Some(5)
    );

    let _ = app.update(Message::ApplyAction(MailAction::SetUnread(true)));
    let _ = app.update(Message::ApplyAction(MailAction::SetUnread(true)));
    assert_eq!(app.mailbox().unwrap().search_query(), "design review");
    assert_eq!(app.mailbox().unwrap().visible_conversations().count(), 1);
    assert!(app.mailbox().unwrap().conversations()[0].unread);
    assert_eq!(
        app.mailbox()
            .unwrap()
            .counts()
            .unwrap()
            .unread(&sys(MailFolder::Inbox)),
        Some(6)
    );

    let _ = app.update(Message::ApplyAction(MailAction::SetUnread(false)));
    let _ = app.update(Message::ApplyAction(MailAction::SetUnread(false)));
    let _ = app.update(Message::ApplyAction(MailAction::SetStarred(true)));
    assert_eq!(app.mailbox().unwrap().reader().unwrap().detail(), &detail);
    assert!(!app.mailbox().unwrap().conversations()[0].unread);
    assert_eq!(
        app.mailbox()
            .unwrap()
            .counts()
            .unwrap()
            .unread(&sys(MailFolder::Inbox)),
        Some(5)
    );
}

#[test]
fn demo_star_actions_update_starred_folder_and_selection() {
    let mut app = loaded_demo_app();
    open_conversation(&mut app, "demo-1");
    let _ = app.update(Message::ApplyAction(MailAction::SetStarred(true)));
    let _ = app.update(Message::ApplyAction(MailAction::SetStarred(true)));
    assert!(app.mailbox().unwrap().conversations()[1].starred);

    let _ = app.update(Message::SelectFolder(sys(MailFolder::Starred)));
    deliver_latest_demo_page(&mut app, 0);
    assert!(is_listed(&app, "demo-1"));

    let effects = app.update(Message::SelectConversation("demo-1".into()));
    // Restored, not loaded: besides scrolling, only the row after it loads.
    assert_eq!(effects.units(), 2);
    let preload = app.mailbox().unwrap().preloading().cloned().unwrap();
    assert_ne!(preload.conversation_id, "demo-1");
    let _ = app.update(Message::ApplyAction(MailAction::SetStarred(false)));
    let mailbox = app.mailbox().unwrap();
    assert!(!is_listed(&app, "demo-1"));
    assert!(
        mailbox
            .selected_conversation()
            .is_some_and(|id| id != "demo-1")
    );
}

#[test]
fn demo_archive_removes_selected_and_opens_next() {
    let mut app = loaded_demo_app();
    open_conversation(&mut app, "demo-0");

    let _ = app.update(Message::ApplyAction(MailAction::MoveTo(sys(
        MailFolder::Archive,
    ))));

    let mailbox = app.mailbox().unwrap();
    assert!(!is_listed(&app, "demo-0"));
    assert_eq!(mailbox.selected_conversation(), Some("demo-1"));

    let _ = app.update(Message::SelectFolder(sys(MailFolder::Archive)));
    deliver_latest_demo_page(&mut app, 0);
    assert!(is_listed(&app, "demo-0"));
}

#[test]
fn moving_selected_conversation_invalidates_its_pending_detail() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::SelectConversation("demo-0".into()));
    let stale = pending_reader_request(&app);

    let task = app.update(Message::ApplyAction(MailAction::MoveTo(sys(
        MailFolder::Archive,
    ))));
    let current = pending_reader_request(&app);
    let stale_detail = demo_service(&app).conversation_detail("demo-0", NOW);
    let _ = app.update(Message::ConversationLoaded(stale, stale_detail));

    // Opening the next conversation: its detail request and the scroll
    // back to the top of the reader.
    assert_eq!(task.units(), 2);
    assert_eq!(
        app.mailbox().unwrap().selected_conversation(),
        Some("demo-1")
    );
    assert_eq!(pending_reader_request(&app), current);
}

#[test]
fn undoing_a_demo_move_puts_the_conversation_back() {
    let mut app = loaded_demo_app();
    open_conversation(&mut app, "demo-3");

    let _ = app.update(Message::ApplyAction(MailAction::MoveTo(sys(
        MailFolder::Archive,
    ))));
    assert!(!is_listed(&app, "demo-3"));
    assert!(app.mailbox().unwrap().undo().is_some());

    let _ = app.update(Message::UndoMove);

    assert!(is_listed(&app, "demo-3"));
    assert!(app.mailbox().unwrap().undo().is_none());
}

#[test]
fn demo_trash_and_spam_moves_update_folders() {
    for (message, folder) in [
        (
            Message::ApplyAction(MailAction::MoveTo(sys(MailFolder::Trash))),
            sys(MailFolder::Trash),
        ),
        (
            Message::ApplyAction(MailAction::MoveTo(sys(MailFolder::Spam))),
            sys(MailFolder::Spam),
        ),
    ] {
        let mut app = loaded_demo_app();
        open_conversation(&mut app, "demo-3");
        let _ = app.update(message);
        assert!(!is_listed(&app, "demo-3"), "{folder:?}");

        let _ = app.update(Message::SelectFolder(folder.clone()));
        deliver_latest_demo_page(&mut app, 0);
        assert!(is_listed(&app, "demo-3"), "{folder:?}");
    }
}

#[test]
fn a_move_proton_refuses_brings_the_row_back_with_the_list() {
    let mut app = loaded_demo_app();
    open_conversation(&mut app, "demo-0");
    // As the Proton path does: the move shows at once.
    let request = app
        .mailbox
        .as_mut()
        .unwrap()
        .start_action(MailAction::MoveTo(sys(MailFolder::Archive)), 90)
        .and_then(|start| start.send)
        .unwrap();
    let has_row = |app: &App| app.mailbox().unwrap().has_row("demo-0");
    assert!(!has_row(&app));

    let _ = app.update(Message::ActionFinished(request, Err(MailboxError::Service)));

    let mailbox = app.mailbox().unwrap();
    assert_eq!(mailbox.action_error(), Some(MailboxError::Service));
    let ListStatus::Refreshing(page) = mailbox.status() else {
        panic!("the folder is read again");
    };
    deliver_demo_page(&mut app, page, 0);
    assert!(has_row(&app), "Proton never moved it");
}

#[test]
fn changes_on_their_way_to_proton_count_until_they_arrive() {
    let mut app = loaded_demo_app();
    assert_eq!(app.unsent_changes(), 0);
    // Proton's actions go one after another, the way the demo's never wait.
    let star = |app: &mut App, row: &str, request| {
        app.mailbox
            .as_mut()
            .unwrap()
            .start_action_on(
                row.to_owned(),
                crate::mail::SummaryKind::Conversation,
                MailAction::SetStarred(true),
                request,
            )
            .unwrap()
    };
    let sent = star(&mut app, "demo-0", 900).send.unwrap();
    assert!(
        star(&mut app, "demo-1", 901).send.is_none(),
        "it waits its turn"
    );
    assert_eq!(app.unsent_changes(), 2);

    let finish = |app: &mut App, request| {
        app.mailbox
            .as_mut()
            .unwrap()
            .finish_action(&request, Ok(()))
    };
    let next = finish(&mut app, sent).next_action.unwrap();
    assert_eq!(app.unsent_changes(), 1);
    let _ = finish(&mut app, next);
    assert_eq!(app.unsent_changes(), 0);
}

#[test]
fn demo_mail_marked_not_spam_goes_back_to_the_inbox() {
    let mut app = loaded_demo_app();
    open_conversation(&mut app, "demo-3");
    let _ = app.update(Message::ApplyAction(MailAction::MoveTo(sys(
        MailFolder::Spam,
    ))));
    let _ = app.update(Message::SelectFolder(sys(MailFolder::Spam)));
    deliver_latest_demo_page(&mut app, 0);
    open_conversation(&mut app, "demo-3");

    let _ = app.update(Message::ApplyAction(MailAction::MoveTo(Folder::INBOX)));

    assert!(!is_listed(&app, "demo-3"));
    let undo = app.mailbox().unwrap().undo().expect("an offer to undo");
    assert_eq!(undo.from, sys(MailFolder::Spam));
    assert_eq!(undo.to, Folder::INBOX);

    let _ = app.update(Message::SelectFolder(Folder::INBOX));
    deliver_latest_demo_page(&mut app, 0);
    assert!(is_listed(&app, "demo-3"));
}

#[test]
fn moving_only_conversation_closes_reader_cleanly() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::SelectConversation("demo-3".into()));
    let _ = app.update(Message::ApplyAction(MailAction::MoveTo(sys(
        MailFolder::Trash,
    ))));
    let _ = app.update(Message::SelectFolder(sys(MailFolder::Trash)));
    deliver_latest_demo_page(&mut app, 0);
    let _ = app.update(Message::SelectConversation("demo-3".into()));

    let _ = app.update(Message::ApplyAction(MailAction::MoveTo(sys(
        MailFolder::Archive,
    ))));

    let mailbox = app.mailbox().unwrap();
    assert!(mailbox.conversations().is_empty());
    assert!(mailbox.reader().is_none());
    assert_eq!(mailbox.selected_conversation(), None);
}
