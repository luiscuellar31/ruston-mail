//! Opening a conversation in the reader, and the reader behind a draft.

use super::*;

fn is_unread(app: &App, id: &str) -> bool {
    app.mailbox()
        .unwrap()
        .conversations()
        .iter()
        .find(|row| row.id == id)
        .is_some_and(|row| row.unread)
}

fn unread_rows(app: &App) -> Vec<String> {
    app.mailbox()
        .unwrap()
        .conversations()
        .iter()
        .filter(|row| row.unread)
        .map(|row| row.id.clone())
        .collect()
}

/// Opens `id` while a draft covers the reader, as when a conversation asked
/// for just before Continue writing arrives after it: it waits, unread, for
/// the reader to show it.
fn open_behind_the_draft(app: &mut App, id: &str) {
    assert!(app.draft_covers_reader());
    let _ = app.start_conversation_load(id.to_owned());
    deliver_selected_demo_detail(app);
    assert!(app.defer_read(id));
}

#[test]
fn demo_selection_is_local() {
    let (mut app, _) = App::boot(true, Settings::default());
    deliver_demo_page(&mut app, OPENING_PAGE, 0);

    let _ = app.update(Message::SelectConversation("demo-3".into()));

    assert_eq!(
        app.mailbox().unwrap().selected_conversation(),
        Some("demo-3")
    );
}

#[test]
fn selecting_only_queues_a_detail_request() {
    let mut app = loaded_demo_app();
    let status = app.mailbox().unwrap().status();
    let counts = app.mailbox().unwrap().counts().cloned();
    let conversation_ids: Vec<_> = app
        .mailbox()
        .unwrap()
        .conversations()
        .iter()
        .map(|conversation| conversation.id.clone())
        .collect();

    let task = app.update(Message::SelectConversation("demo-3".into()));

    // The detail request and the scroll back to the top of the reader.
    // `last_request` is what proves only one of them reaches the backend.
    assert_eq!(task.units(), 2);
    assert_eq!(app.last_request, 3);
    let mailbox = app.mailbox().unwrap();
    assert_eq!(mailbox.status(), status);
    assert_eq!(mailbox.counts().cloned(), counts);
    assert_eq!(
        mailbox
            .conversations()
            .iter()
            .map(|conversation| conversation.id.clone())
            .collect::<Vec<_>>(),
        conversation_ids
    );
    assert!(matches!(
        mailbox.reader_state(),
        ReaderState::Loading {
            conversation_id,
            request: 3,
        } if conversation_id == "demo-3"
    ));
}

#[test]
fn no_selection_has_no_reader() {
    let (mut app, _) = App::boot(true, Settings::default());
    deliver_demo_page(&mut app, OPENING_PAGE, 0);

    assert_eq!(app.mailbox().unwrap().reader_state(), &ReaderState::Empty);
}

#[test]
fn demo_selection_opens_reader_with_newest_message_expanded() {
    let (mut app, _) = App::boot(true, Settings::default());
    deliver_demo_page(&mut app, OPENING_PAGE, 0);

    open_conversation(&mut app, "demo-0");

    let reader = app.mailbox().unwrap().reader().unwrap();
    assert_eq!(reader.conversation_id(), "demo-0");
    assert_eq!(reader.detail().id, reader.conversation_id());
    assert_eq!(reader.detail().messages.len(), 3);
    assert!(reader.is_expanded("demo-0-2", Reading::default()));
    assert!(!reader.is_expanded("demo-0-0", Reading::default()));
}

#[test]
fn returning_to_a_conversation_restores_its_reader_without_loading() {
    let (mut app, _) = App::boot(true, Settings::default());
    deliver_demo_page(&mut app, OPENING_PAGE, 0);
    open_conversation(&mut app, "demo-0");

    let _ = app.update(Message::ToggleMessageExpanded("demo-0-0".into()));
    let reader = app.mailbox().unwrap().reader().unwrap();
    assert!(reader.is_expanded("demo-0-0", Reading::default()));
    assert!(reader.is_expanded("demo-0-2", Reading::default()));

    open_conversation(&mut app, "demo-2");
    let effects = app.update(Message::SelectConversation("demo-0".into()));
    // The only work is scrolling the restored reader to its top and
    // preloading the row after it, not loading it again.
    assert_eq!(effects.units(), 2);
    let preload = app.mailbox().unwrap().preloading().cloned();
    assert_eq!(preload.unwrap().conversation_id, "demo-1");
    assert!(
        app.mailbox()
            .unwrap()
            .reader()
            .unwrap()
            .is_expanded("demo-0-0", Reading::default())
    );
}

#[test]
fn folder_switch_closes_reader() {
    let (mut app, _) = App::boot(true, Settings::default());
    deliver_demo_page(&mut app, OPENING_PAGE, 0);
    let _ = app.update(Message::SelectConversation("demo-0".into()));

    let _ = app.update(Message::SelectFolder(sys(MailFolder::Sent)));

    assert!(app.mailbox().unwrap().reader().is_none());
}

#[test]
fn an_open_conversation_preloads_the_one_archiving_it_opens_at_once() {
    let mut app = loaded_demo_app();
    // A row with an unread one below it, so reading that one would show.
    let (open, next) = {
        let rows: Vec<_> = app.mailbox().unwrap().visible_conversations().collect();
        let index = rows
            .windows(2)
            .position(|pair| pair[1].unread)
            .expect("an unread row below another");
        (rows[index].id.clone(), rows[index + 1].id.clone())
    };
    let _ = app.update(Message::SelectConversation(open.clone()));
    deliver_selected_demo_detail(&mut app);

    let preload = app
        .mailbox()
        .unwrap()
        .preloading()
        .cloned()
        .expect("a preload on its way");
    assert_eq!(preload.conversation_id, next);
    let result = demo_service(&app).conversation_detail(&next, NOW);
    let _ = app.update(Message::ConversationPreloaded(preload, result));
    let mailbox = app.mailbox().unwrap();
    assert_eq!(mailbox.reader().unwrap().conversation_id(), open);
    assert!(mailbox.row(&next).unwrap().unread, "preloading read it");

    let _ = app.update(Message::ApplyAction(MailAction::MoveTo(sys(
        MailFolder::Archive,
    ))));
    let reader = app.mailbox().unwrap().reader();
    assert_eq!(
        reader.map(|reader| reader.conversation_id()),
        Some(next.as_str())
    );
}

#[test]
fn detail_failure_and_retry_use_a_new_request() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::SelectConversation("demo-3".into()));
    let first = pending_reader_request(&app);
    let _ = app.update(Message::ConversationLoaded(
        first.clone(),
        Err(MailboxError::Connection),
    ));
    assert!(matches!(
        app.mailbox().unwrap().reader_state(),
        ReaderState::Failed {
            conversation_id,
            error: MailboxError::Connection,
        } if conversation_id == "demo-3"
    ));

    let task = app.update(Message::RetryConversation);
    let retry = pending_reader_request(&app);

    assert_eq!(task.units(), 1);
    assert!(retry.id > first.id);
    assert_eq!(retry.conversation_id, first.conversation_id);
}

#[test]
fn abandoned_conversation_loads_are_stopped_and_ignored() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::SelectConversation("demo-0".into()));
    let first_request = pending_reader_request(&app);
    let first = app.reader_load.clone().expect("a load is in flight");

    // Moving on to the next row leaves the first load unwanted.
    let _ = app.update(Message::SelectConversation("demo-1".into()));
    assert!(first.is_aborted());
    let second = app.reader_load.clone().unwrap();
    assert!(!second.is_aborted());
    // What an aborted load answers changes nothing on screen.
    let _ = app.update(Message::ConversationLoaded(
        first_request,
        Err(MailboxError::Unavailable),
    ));
    assert_eq!(pending_reader_request(&app).conversation_id, "demo-1");

    let _ = app.update(Message::SelectFolder(sys(MailFolder::Archive)));
    assert!(second.is_aborted(), "a folder change closes the reader");

    let mut app = loaded_demo_app();
    let _ = app.update(Message::SelectConversation("demo-0".into()));
    let load = app.reader_load.clone().unwrap();
    let _ = app.update(Message::Logout);
    assert!(load.is_aborted(), "the session ended");
}

#[test]
fn a_conversation_chosen_while_writing_shows_with_the_draft_folded_above_it() {
    let mut app = loaded_demo_app();
    let id = unread_rows(&app).remove(0);
    let _ = app.update(Message::OpenCompose);
    let _ = app.update(Message::ComposeChanged(ComposeField::Body, "Plans".into()));
    let draft = app.compose().unwrap().id();

    // A click in the list folds the draft and shows the conversation, which
    // is read like any other.
    open_conversation(&mut app, &id);
    assert_eq!(app.mailbox().unwrap().selected_conversation(), Some(&*id));
    assert!(app.compose().unwrap().folded());
    assert!(!app.draft_covers_reader());
    assert!(!is_unread(&app, &id));

    // Continue writing brings the same draft back over the reader.
    let _ = app.update(Message::ContinueWriting);
    let compose = app.compose().unwrap();
    assert_eq!(compose.id(), draft);
    assert!(!compose.folded());
    assert_eq!(compose.field(ComposeField::Body), "Plans");
    assert!(app.draft_covers_reader());

    // So does asking for another message, which says why none opened.
    let _ = app.update(Message::SelectConversation(id));
    assert!(app.compose().unwrap().folded());
    let _ = app.update(Message::OpenCompose);
    assert!(!app.compose().unwrap().folded());
    assert!(app.compose().unwrap().asked_again());
}

#[test]
fn a_folded_draft_leaves_the_keys_to_the_reader_it_shows() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::OpenCompose);
    let _ = app.update(Message::ComposeChanged(ComposeField::Body, "Plans".into()));
    open_conversation(&mut app, "demo-0");

    // The list keys walk the mail the reader shows...
    press(&mut app, Key::Character('j'));
    assert_eq!(
        app.mailbox().unwrap().selected_conversation(),
        Some("demo-1")
    );
    // ...the send key does not send the folded draft...
    press_command(&mut app, Key::Enter);
    assert!(!app.compose().unwrap().in_flight());
    // ...and Escape closes the conversation, not the draft.
    press(&mut app, Key::Escape);
    assert!(matches!(
        app.mailbox().unwrap().reader_state(),
        ReaderState::Empty
    ));
    let compose = app.compose().expect("the draft stays");
    assert!(compose.folded());
    assert!(!compose.confirming_discard());
}

#[test]
fn a_conversation_that_arrived_behind_the_draft_is_read_once_shown() {
    let mut app = loaded_demo_app();
    let id = unread_rows(&app).remove(0);
    let _ = app.update(Message::OpenCompose);
    open_behind_the_draft(&mut app, &id);
    assert!(is_unread(&app, &id), "marked read behind the draft");

    // Putting the untouched draft away shows it, and so reads it.
    let _ = app.update(Message::CloseCompose);
    assert!(app.compose().is_none());
    assert!(!is_unread(&app, &id));
}

#[test]
fn a_conversation_left_behind_a_draft_is_never_marked_read() {
    let mut app = loaded_demo_app();
    let inbox_unread = |app: &App| demo_service(app).counts().unread(&Folder::INBOX);
    let before = inbox_unread(&app);
    let id = unread_rows(&app).remove(0);
    let _ = app.update(Message::OpenCompose);
    open_behind_the_draft(&mut app, &id);

    // Another folder replaces it before the draft is put away.
    let _ = app.update(Message::SelectFolder(sys(MailFolder::Archive)));
    deliver_latest_demo_page(&mut app, 0);
    let _ = app.update(Message::CloseCompose);

    assert_eq!(inbox_unread(&app), before);
}

#[test]
fn moving_the_draft_to_its_own_window_shows_what_it_covered() {
    let mut app = loaded_demo_app();
    let id = unread_rows(&app).remove(0);
    let _ = app.update(Message::OpenCompose);
    open_behind_the_draft(&mut app, &id);
    assert!(is_unread(&app, &id));

    let _ = app.update(Message::SetComposePlacement(ComposePlacement::Window));

    assert!(app.compose().is_some(), "the draft is still being written");
    assert!(!is_unread(&app, &id));
    // With the reader in view, what opens is read at once.
    let next = unread_rows(&app).remove(0);
    open_conversation(&mut app, &next);
    assert!(!is_unread(&app, &next));
}
