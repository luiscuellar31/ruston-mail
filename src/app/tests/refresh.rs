//! Refreshing the open folder, when asked and on its own.

use super::*;

#[test]
fn the_apps_own_refresh_goes_unannounced_until_someone_asks_for_one() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::AutoRefreshMailbox);
    assert!(matches!(
        app.mailbox().unwrap().status(),
        ListStatus::Refreshing(_)
    ));
    assert!(app.refreshing_on_its_own());

    // Asking now shows the refresh already on its way.
    let _ = app.update(Message::RefreshMailbox);
    assert!(matches!(
        app.mailbox().unwrap().status(),
        ListStatus::Refreshing(_)
    ));
    assert!(!app.refreshing_on_its_own());
}

#[test]
fn automatic_refresh_updates_the_open_folder_and_invalidates_other_caches() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::SelectFolder(sys(MailFolder::Sent)));
    deliver_latest_demo_page(&mut app, 0);
    assert_eq!(
        app.update(Message::SelectFolder(sys(MailFolder::Inbox)))
            .units(),
        0
    );

    assert_eq!(
        app.update(Message::AutoRefreshMailbox).units(),
        PAGE_AND_COUNTS
    );
    let page_request = pending_list_request(&app);
    deliver_demo_page(&mut app, page_request, 0);

    assert_eq!(
        app.update(Message::SelectFolder(sys(MailFolder::Sent)))
            .units(),
        1
    );
    assert!(matches!(
        app.mailbox().unwrap().status(),
        ListStatus::Refreshing(_)
    ));
}

#[test]
fn an_automatic_refresh_holds_back_neither_a_move_nor_its_undo() {
    let mut app = loaded_demo_app();
    open_conversation(&mut app, "demo-3");
    assert_eq!(
        app.update(Message::AutoRefreshMailbox).units(),
        PAGE_AND_COUNTS
    );
    press_command(&mut app, Key::Backspace);
    assert!(!is_listed(&app, "demo-3"));

    // The next poll starts while the notice is still up, and keeps it.
    assert_eq!(
        app.update(Message::AutoRefreshMailbox).units(),
        PAGE_AND_COUNTS
    );
    assert!(app.mailbox().unwrap().undo().is_some());
    let _ = app.update(Message::UndoMove);
    assert!(is_listed(&app, "demo-3"));
}

#[test]
fn automatic_refresh_does_not_overlap_a_list_request() {
    let mut app = loaded_demo_app();
    assert_eq!(app.update(Message::RefreshMailbox).units(), PAGE_AND_COUNTS);
    let requests = app.last_request;

    assert_eq!(app.update(Message::AutoRefreshMailbox).units(), 0);
    assert_eq!(app.last_request, requests);
}

#[test]
fn automatic_refresh_waits_while_a_message_is_being_sent() {
    let mut app = loaded_demo_app();
    write(&mut app, "alex@example.com");
    assert_eq!(app.update(Message::Send).units(), 1);
    let requests = app.last_request;

    assert!(!app.auto_refresh_available());
    assert_eq!(app.update(Message::AutoRefreshMailbox).units(), 0);
    assert_eq!(app.last_request, requests);
}

#[test]
fn a_mutation_during_refresh_is_followed_by_a_fresh_request() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::RefreshMailbox);
    let page_request = pending_list_request(&app);
    app.mailbox.as_mut().unwrap().invalidate_listings();
    let folder = app.mailbox().unwrap().folder();
    let page = demo_service(&app).list_conversations(folder, 0, demo::PAGE_SIZE, NOW);

    assert_eq!(
        app.update(Message::ConversationsLoaded(page_request, page))
            .units(),
        2
    );
    assert!(matches!(
        app.mailbox().unwrap().status(),
        ListStatus::Refreshing(_)
    ));
}
