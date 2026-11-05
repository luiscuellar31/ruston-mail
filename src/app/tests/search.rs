//! Searching on Proton and on this device, and results that actions change.

use super::*;

/// Runs what is typed against the demo backend, the way pressing Enter
/// asks the server for everything matching it.
fn deliver_demo_search(app: &mut App, query: &str) {
    let _ = app.update(Message::SearchChanged(query.to_owned()));
    let _ = app.update(Message::SearchSubmitted);
    let request = SearchRequest {
        id: pending_list_request(app),
        query: query.to_owned(),
        page: 0,
        page_size: demo::PAGE_SIZE,
    };
    let result = demo_service(app).search(query, demo::PAGE_SIZE, NOW);
    let _ = app.update(Message::SearchLoaded(request, result));
}

/// The app with its first page in, a search for `query` sent, and the
/// request it is waiting on.
fn searching(settings: Settings, query: &str) -> (App, SearchRequest) {
    let (mut app, _) = App::boot(true, settings);
    let folder = app.mailbox().unwrap().folder();
    let page = demo_service(&app).list_conversations(folder, 0, demo::PAGE_SIZE, NOW);
    let _ = app.update(Message::ConversationsLoaded(OPENING_PAGE, page));
    let _ = app.update(Message::SearchChanged(query.to_owned()));
    let _ = app.update(Message::SearchSubmitted);
    let request = SearchRequest {
        id: pending_list_request(&app),
        query: query.to_owned(),
        page: 0,
        page_size: demo::PAGE_SIZE,
    };
    (app, request)
}

#[test]
fn a_search_submitted_while_the_folder_loads_starts_after_it() {
    let (mut app, _) = App::boot(true, Settings::default());
    let _ = app.update(Message::SearchChanged("Avery".into()));

    // The first page is still on its way; a search now would drop it.
    assert_eq!(app.update(Message::SearchSubmitted).units(), 0);

    let folder = app.mailbox().unwrap().folder();
    let page = demo_service(&app).list_conversations(folder, 0, demo::PAGE_SIZE, NOW);
    let effects = app.update(Message::ConversationsLoaded(OPENING_PAGE, page));
    assert_eq!(effects.units(), 1, "the waiting search starts");
    assert!(app.mailbox().unwrap().loaded_count() > 0);

    let request = SearchRequest {
        id: pending_list_request(&app),
        query: "Avery".into(),
        page: 0,
        page_size: demo::PAGE_SIZE,
    };
    let result = demo_service(&app).search("Avery", demo::PAGE_SIZE, NOW);
    let _ = app.update(Message::SearchLoaded(request, result));
    assert_eq!(app.mailbox().unwrap().shown_search_query(), Some("Avery"));

    // Leaving the results shows the folder that loaded meanwhile.
    let _ = app.update(Message::SearchChanged(String::new()));
    assert!(app.mailbox().unwrap().visible_conversations().len() > 0);
}

#[test]
fn a_search_proton_cannot_answer_looks_on_this_device_only_once_asked() {
    // Not asked: the search fails as it always has.
    let (mut app, request) = searching(Settings::default(), "Avery");
    let effects = app.update(Message::SearchLoaded(
        request,
        Err(MailboxError::Connection),
    ));
    assert_eq!(effects.units(), 0);
    assert!(matches!(
        app.mailbox().unwrap().status(),
        ListStatus::Failed(MailboxError::Connection, ListAttempt::Search)
    ));

    // Asked: this device's index is searched instead, and what it finds shows.
    let mut settings = Settings::default();
    settings.search_device_offline = true;
    let (mut app, request) = searching(settings.clone(), "Avery");
    let effects = app.update(Message::SearchLoaded(
        request.clone(),
        Err(MailboxError::Connection),
    ));
    assert_eq!(effects.units(), 1, "this device's index is searched");
    assert!(matches!(
        app.mailbox().unwrap().status(),
        ListStatus::Loading(_)
    ));
    let found = demo_service(&app)
        .search("Avery", demo::PAGE_SIZE, NOW)
        .unwrap();
    let _ = app.update(Message::DeviceSearchLoaded(request, Ok(Some(found))));
    let mailbox = app.mailbox().unwrap();
    assert_eq!(mailbox.shown_search_query(), Some("Avery"));
    assert!(mailbox.shown_search_is_on_device());
    assert!(mailbox.visible_conversations().len() > 0);

    // Any other failure is not Proton being out of reach.
    let (mut app, request) = searching(settings.clone(), "Avery");
    let effects = app.update(Message::SearchLoaded(request, Err(MailboxError::Service)));
    assert_eq!(effects.units(), 0);

    // With no index here, the search fails, saying there was none to look in.
    let (mut app, request) = searching(settings, "Avery");
    let _ = app.update(Message::SearchLoaded(
        request.clone(),
        Err(MailboxError::Connection),
    ));
    let _ = app.update(Message::DeviceSearchLoaded(request, Ok(None)));
    let mailbox = app.mailbox().unwrap();
    assert!(matches!(
        mailbox.status(),
        ListStatus::Failed(MailboxError::Connection, ListAttempt::Search)
    ));
    assert_eq!(mailbox.device_index(), Some(DeviceIndex::Missing));
    // A new search starts afresh.
    let _ = app.update(Message::SearchChanged(String::new()));
    assert_eq!(app.mailbox().unwrap().device_index(), None);
}

#[test]
fn a_search_from_this_device_that_comes_too_late_is_dropped() {
    let mut settings = Settings::default();
    settings.search_device_offline = true;
    let (mut app, request) = searching(settings, "Avery");
    let _ = app.update(Message::SearchLoaded(
        request.clone(),
        Err(MailboxError::Connection),
    ));
    // The query changed meanwhile.
    let _ = app.update(Message::SearchChanged(String::new()));
    let found = demo_service(&app)
        .search("Avery", demo::PAGE_SIZE, NOW)
        .unwrap();
    let _ = app.update(Message::DeviceSearchLoaded(request, Ok(Some(found))));
    assert_eq!(app.mailbox().unwrap().shown_search_query(), None);
}

#[test]
fn a_search_waiting_for_the_folder_is_dropped_when_the_query_changes() {
    let (mut app, _) = App::boot(true, Settings::default());
    let _ = app.update(Message::SearchChanged("Avery".into()));
    let _ = app.update(Message::SearchSubmitted);
    let _ = app.update(Message::SearchChanged("Avery Q".into()));

    let folder = app.mailbox().unwrap().folder();
    let page = demo_service(&app).list_conversations(folder, 0, demo::PAGE_SIZE, NOW);
    let effects = app.update(Message::ConversationsLoaded(OPENING_PAGE, page));

    assert_eq!(effects.units(), 0, "Enter was for another query");
    assert_eq!(app.mailbox().unwrap().shown_search_query(), None);
}

#[test]
fn starred_search_reflects_unstar_immediately() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::SelectFolder(sys(MailFolder::Starred)));
    deliver_latest_demo_page(&mut app, 0);
    let _ = app.update(Message::SearchChanged("offsite".into()));
    let _ = app.update(Message::SelectConversation("demo-2".into()));

    let _ = app.update(Message::ApplyAction(MailAction::SetStarred(false)));

    let mailbox = app.mailbox().unwrap();
    assert_eq!(mailbox.search_query(), "offsite");
    assert_eq!(mailbox.visible_conversations().count(), 0);
    assert_eq!(mailbox.selected_conversation(), None);
}

#[test]
fn a_conversation_the_server_found_can_still_be_acted_on() {
    let mut app = loaded_demo_app();
    deliver_demo_search(&mut app, "offsite");
    let found = app
        .mailbox()
        .unwrap()
        .visible_conversations()
        .next()
        .expect("the search found something")
        .id
        .clone();
    open_conversation(&mut app, &found);

    // The row is not in the open folder's listing, which used to leave
    // the reader with no actions at all.
    assert_eq!(
        app.mailbox().unwrap().selected_summary().map(|row| &row.id),
        Some(&found)
    );

    let _ = app.update(Message::ApplyAction(MailAction::SetStarred(true)));

    // The results are what is on screen, so they carry the change.
    let mailbox = app.mailbox().unwrap();
    assert_eq!(mailbox.shown_search_query(), Some("offsite"));
    assert!(
        mailbox
            .visible_conversations()
            .find(|row| row.id == found)
            .expect("the row is still listed")
            .starred
    );
}

#[test]
fn archiving_from_the_results_takes_the_row_out_of_them() {
    let mut app = loaded_demo_app();
    deliver_demo_search(&mut app, "the");
    let rows: Vec<String> = app
        .mailbox()
        .unwrap()
        .visible_conversations()
        .map(|row| row.id.clone())
        .collect();
    assert!(rows.len() > 1, "the search found more than one row");
    open_conversation(&mut app, &rows[0]);

    let _ = app.update(Message::ApplyAction(MailAction::MoveTo(sys(
        MailFolder::Archive,
    ))));

    // The demo has no server to re-read, so nothing else would have taken
    // the row out of the results, and archiving would look like a no-op.
    let mailbox = app.mailbox().unwrap();
    assert!(
        !mailbox.visible_conversations().any(|row| row.id == rows[0]),
        "the archived row is still listed among the results"
    );
    assert_eq!(mailbox.selected_conversation(), Some(rows[1].as_str()));
}

#[test]
fn inbox_search_reflects_folder_moves_immediately() {
    for folder in [MailFolder::Archive, MailFolder::Trash, MailFolder::Spam] {
        let mut app = loaded_demo_app();
        let _ = app.update(Message::SearchChanged("blue notebook".into()));
        assert_eq!(app.mailbox().unwrap().visible_conversations().count(), 1);
        open_conversation(&mut app, "demo-3");

        let _ = app.update(Message::ApplyAction(MailAction::MoveTo(sys(folder))));

        let mailbox = app.mailbox().unwrap();
        assert_eq!(mailbox.search_query(), "blue notebook", "{folder:?}");
        assert_eq!(mailbox.visible_conversations().count(), 0, "{folder:?}");
        assert_eq!(mailbox.selected_conversation(), None, "{folder:?}");
    }
}
