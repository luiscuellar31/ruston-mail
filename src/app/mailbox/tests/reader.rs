//! The reader: loading a conversation, its cache, preloading, the next row.

use super::*;

#[test]
fn the_keyboard_steps_through_visible_rows() {
    let mut mailbox = loaded_inbox(&["a", "b", "c"], 3);

    // Nothing open yet, so either direction starts at the top.
    assert_eq!(mailbox.neighbour(Step::Next).as_deref(), Some("a"));
    assert_eq!(mailbox.neighbour(Step::Previous).as_deref(), Some("a"));

    load_detail(&mut mailbox, "b", detail("b", &["b1"]), 4);
    assert_eq!(mailbox.neighbour(Step::Next).as_deref(), Some("c"));
    assert_eq!(mailbox.neighbour(Step::Previous).as_deref(), Some("a"));

    // The ends stop rather than wrap around.
    load_detail(&mut mailbox, "c", detail("c", &["c1"]), 5);
    assert_eq!(mailbox.neighbour(Step::Next), None);
    load_detail(&mut mailbox, "a", detail("a", &["a1"]), 6);
    assert_eq!(mailbox.neighbour(Step::Previous), None);
}

#[test]
fn visible_position_places_a_row_between_the_ends() {
    let mut mailbox = loaded_inbox(&["a", "b", "c"], 3);

    assert_eq!(mailbox.visible_position("a"), Some(0.0));
    assert_eq!(mailbox.visible_position("b"), Some(0.5));
    assert_eq!(mailbox.visible_position("c"), Some(1.0));
    assert_eq!(mailbox.visible_position("missing"), None);

    // Positions follow what is on screen, not what is loaded.
    mailbox.set_search_query("nothing matches this".into());
    assert_eq!(mailbox.visible_position("b"), None);

    // A single row has nowhere to scroll to.
    assert_eq!(loaded_inbox(&["only"], 1).visible_position("only"), None);
}

#[test]
fn clicking_a_failed_conversation_again_retries_it() {
    let mut mailbox = loaded_inbox(&["a", "b"], 2);
    let request = mailbox.start_conversation_load("a".into(), 3).unwrap();
    assert_eq!(
        mailbox.finish_conversation(&request, Err(MailboxError::Connection)),
        Some(MailboxError::Connection)
    );

    // The failed row stays selected, so the click used to do nothing.
    assert!(mailbox.start_conversation_load("a".into(), 4).is_some());
}

#[test]
fn reselecting_and_returning_to_cached_mail_keep_expansion() {
    let mut mailbox = loaded_inbox(&["a", "b"], 2);
    load_detail(&mut mailbox, "a", detail("a", &["a1", "a2"]), 3);
    mailbox.toggle_message("a1");

    assert_eq!(mailbox.start_conversation_load("a".into(), 4), None);
    assert!(
        mailbox
            .reader()
            .unwrap()
            .is_expanded("a1", Reading::default())
    );

    load_detail(&mut mailbox, "b", detail("b", &["b1"]), 5);
    assert_eq!(mailbox.start_conversation_load("a".into(), 6), None);
    let reader = mailbox.reader().unwrap();
    assert!(reader.is_expanded("a1", Reading::default()));
    assert!(reader.is_expanded("a2", Reading::default()));
}

#[test]
fn changed_list_metadata_makes_a_cached_reader_reload() {
    let mut mailbox = loaded_inbox(&["a", "b"], 2);
    load_detail(&mut mailbox, "a", detail("a", &["a1"]), 3);

    let refresh = mailbox.refresh(4).unwrap();
    let mut changed = summary("a");
    changed.message_count = 2;
    mailbox.finish_page(refresh.id, page_of(vec![changed, summary("b")], 2));
    load_detail(&mut mailbox, "b", detail("b", &["b1"]), 5);

    let request = mailbox.start_conversation_load("a".into(), 6).unwrap();
    assert_eq!(request.conversation_id, "a");
    assert!(matches!(
        mailbox.reader_state(),
        ReaderState::Loading {
            conversation_id,
            request: 6,
        } if conversation_id == "a"
    ));
}

#[test]
fn reader_cache_evicts_the_least_recent_entry_at_its_limit() {
    let names = ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"];
    let mut mailbox = loaded_inbox(&names, names.len() as u32);
    for (offset, name) in names.iter().enumerate() {
        load_detail(&mut mailbox, name, detail(name, &[name]), 3 + offset as u64);
    }

    assert_eq!(mailbox.cached_readers.len(), READER_CACHE_CAPACITY);
    assert!(
        mailbox
            .cached_readers
            .iter()
            .all(|cached| { cached.reader.conversation_id() != "a" })
    );
    assert!(
        mailbox
            .cached_readers
            .iter()
            .any(|cached| { cached.reader.conversation_id() == "b" })
    );
}

#[test]
fn the_row_a_move_would_open_preloads_into_the_reader_cache_once() {
    let (mut mailbox, request) = open();
    mailbox.finish_page(
        request.id,
        page_of(vec![summary("a"), unread("b"), summary("c")], 3),
    );
    // Nothing shows yet, so nothing comes after it.
    assert_eq!(mailbox.start_preload(3), None);
    load_detail(&mut mailbox, "a", detail("a", &["a1"]), 4);

    let preload = mailbox.start_preload(5).expect("the row below");
    assert_eq!(preload.conversation_id, "b");
    assert_eq!(mailbox.start_preload(6), None, "asked for twice");
    assert_eq!(
        mailbox.finish_preload(&preload, Ok(detail("b", &["b1"]))),
        None
    );
    assert_eq!(mailbox.reader().unwrap().conversation_id(), "a");
    assert!(mailbox.row("b").unwrap().unread);
    assert_eq!(mailbox.start_preload(7), None, "cached already");

    // Moving the open one out of the view opens the preloaded one at once.
    let start = mailbox
        .start_action(MailAction::MoveTo(sys(MailFolder::Archive)), 8)
        .unwrap();
    assert_eq!(start.opened.as_deref(), Some("b"));
    assert_eq!(mailbox.start_conversation_load("b".into(), 9), None);
    assert_eq!(mailbox.reader().unwrap().conversation_id(), "b");
}

#[test]
fn the_last_row_preloads_the_one_above_and_a_lone_row_none() {
    let mut mailbox = loaded_inbox(&["a", "b"], 2);
    load_detail(&mut mailbox, "b", detail("b", &["b1"]), 3);
    assert_eq!(
        mailbox
            .start_preload(4)
            .map(|request| request.conversation_id),
        Some("a".into())
    );

    let mut alone = loaded_inbox(&["a"], 1);
    load_detail(&mut alone, "a", detail("a", &["a1"]), 3);
    assert_eq!(alone.start_preload(4), None);
}

#[test]
fn a_preload_no_longer_wanted_or_failed_is_not_kept() {
    let mut mailbox = loaded_inbox(&["a", "b", "c"], 3);
    load_detail(&mut mailbox, "a", detail("a", &["a1"]), 3);
    let first = mailbox.start_preload(4).unwrap();
    // Another open conversation wants another row.
    load_detail(&mut mailbox, "b", detail("b", &["b1"]), 5);
    let second = mailbox.start_preload(6).unwrap();
    assert_eq!(second.conversation_id, "c");

    assert_eq!(
        mailbox.finish_preload(&first, Ok(detail("b", &["b1"]))),
        None
    );
    assert_eq!(
        mailbox.finish_preload(&second, Err(MailboxError::Connection)),
        Some(MailboxError::Connection)
    );
    assert!(
        mailbox
            .cached_readers
            .iter()
            .all(|cached| cached.reader.conversation_id() == "a"),
        "only the reader left behind is cached"
    );

    // One stopped is let go, so the row can be asked for again.
    let third = mailbox.start_preload(7).unwrap();
    mailbox.forget_preload();
    assert_eq!(
        mailbox.finish_preload(&third, Ok(detail("c", &["c1"]))),
        None
    );
    assert!(mailbox.start_preload(8).is_some());
}

#[test]
fn invalidating_readers_for_a_send_forces_the_next_open_to_reload() {
    let mut mailbox = loaded_inbox(&["a", "b"], 2);
    load_detail(&mut mailbox, "a", detail("a", &["a1"]), 3);
    load_detail(&mut mailbox, "b", detail("b", &["b1"]), 4);
    assert_eq!(mailbox.cached_readers.len(), 1);

    mailbox.invalidate_reader_cache();

    assert!(mailbox.cached_readers.is_empty());
    assert!(mailbox.start_conversation_load("a".into(), 5).is_some());
}

#[test]
fn selecting_conversation_starts_detail_loading() {
    let mut mailbox = loaded_inbox(&["a"], 1);

    let request = mailbox.start_conversation_load("a".into(), 3).unwrap();

    assert_eq!(
        request,
        ReaderRequest {
            id: 3,
            conversation_id: "a".into(),
            kind: SummaryKind::Conversation,
            messages: 1,
        }
    );
    assert_eq!(
        mailbox.reader_state(),
        &ReaderState::Loading {
            conversation_id: "a".into(),
            request: 3,
        }
    );
}

#[test]
fn successful_detail_response_populates_matching_reader() {
    let mut mailbox = loaded_inbox(&["a"], 1);
    let request = mailbox.start_conversation_load("a".into(), 3).unwrap();

    assert_eq!(
        mailbox.finish_conversation(&request, Ok(detail("a", &["a1"]))),
        None
    );

    let reader = mailbox.reader().unwrap();
    assert_eq!(mailbox.selected_conversation(), Some("a"));
    assert_eq!(reader.detail().id, "a");
}

#[test]
fn mismatched_detail_id_is_rejected() {
    let mut mailbox = loaded_inbox(&["a"], 1);
    let request = mailbox.start_conversation_load("a".into(), 3).unwrap();

    assert_eq!(
        mailbox.finish_conversation(&request, Ok(detail("b", &["b1"]))),
        Some(MailboxError::Unavailable)
    );
    assert_eq!(
        mailbox.reader_state(),
        &ReaderState::Failed {
            conversation_id: "a".into(),
            error: MailboxError::Unavailable,
        }
    );
}

#[test]
fn newer_selection_ignores_stale_detail_response() {
    let mut mailbox = loaded_inbox(&["a", "b"], 2);
    let request_a = mailbox.start_conversation_load("a".into(), 3).unwrap();
    let request_b = mailbox.start_conversation_load("b".into(), 4).unwrap();

    assert_eq!(
        mailbox.finish_conversation(&request_a, Ok(detail("a", &["a1"]))),
        None
    );
    assert_eq!(mailbox.selected_conversation(), Some("b"));
    assert!(mailbox.reader().is_none());

    mailbox.finish_conversation(&request_b, Ok(detail("b", &["b1"])));
    assert_eq!(mailbox.reader().unwrap().detail().id, "b");
}

#[test]
fn a_late_answer_for_a_conversation_left_behind_is_not_awaited() {
    let mut mailbox = loaded_inbox(&["a", "b"], 2);
    load_detail(&mut mailbox, "a", detail("a", &["a1"]), 3);
    let slow = mailbox.start_conversation_load("b".into(), 4).unwrap();
    assert!(mailbox.awaits(&slow));

    // Back to A, which opens from the cache while B is still loading.
    assert_eq!(mailbox.start_conversation_load("a".into(), 5), None);
    assert_eq!(mailbox.reader().unwrap().detail().id, "a");

    // B's answer opens nothing, so nothing may be marked read for it.
    assert!(!mailbox.awaits(&slow));
    assert_eq!(
        mailbox.finish_conversation(&slow, Ok(detail("b", &["b1"]))),
        None
    );
    assert_eq!(mailbox.reader().unwrap().detail().id, "a");
}

#[test]
fn folder_switch_invalidates_outstanding_detail_request() {
    let mut mailbox = loaded_inbox(&["a"], 1);
    let detail_request = mailbox.start_conversation_load("a".into(), 3).unwrap();

    mailbox.select_folder(sys(MailFolder::Archive), 4);
    mailbox.finish_conversation(&detail_request, Ok(detail("a", &["a1"])));

    assert_eq!(mailbox.reader_state(), &ReaderState::Empty);
    assert_eq!(mailbox.selected_conversation(), None);
}

#[test]
fn failed_detail_can_retry_with_new_request() {
    let mut mailbox = loaded_inbox(&["a"], 1);
    let first = mailbox.start_conversation_load("a".into(), 3).unwrap();
    mailbox.finish_conversation(&first, Err(MailboxError::Connection));
    assert_eq!(
        mailbox.reader_state(),
        &ReaderState::Failed {
            conversation_id: "a".into(),
            error: MailboxError::Connection,
        }
    );

    let retry = mailbox.retry_conversation(4).unwrap();

    assert_eq!(retry.id, 4);
    assert_eq!(retry.conversation_id, "a");
    assert_eq!(
        mailbox.reader_state(),
        &ReaderState::Loading {
            conversation_id: "a".into(),
            request: 4,
        }
    );
}

#[test]
fn a_load_knows_how_long_its_conversation_is() {
    let (mut mailbox, request) = open();
    mailbox.finish_page(
        request.id,
        page_of(
            vec![ConversationSummary {
                message_count: 12,
                ..summary("long")
            }],
            1,
        ),
    );

    // The desktop gives the load time for every message the row lists.
    let first = mailbox.start_conversation_load("long".into(), 3).unwrap();
    assert_eq!(first.messages, 12);
    mailbox.finish_conversation(&first, Err(MailboxError::Connection));
    assert_eq!(mailbox.retry_conversation(4).unwrap().messages, 12);
}

#[test]
fn message_rows_load_as_single_messages() {
    let (mut mailbox, request) = open();
    mailbox.finish_page(
        request.id,
        page_of(vec![message_row("m1"), summary("c1")], 2),
    );

    let message = mailbox.start_conversation_load("m1".into(), 3).unwrap();
    assert_eq!(message.kind, SummaryKind::Message);
    mailbox.finish_conversation(&message, Err(MailboxError::Connection));
    assert_eq!(
        mailbox.retry_conversation(4).unwrap().kind,
        SummaryKind::Message
    );

    let conversation = mailbox.start_conversation_load("c1".into(), 5).unwrap();
    assert_eq!(conversation.kind, SummaryKind::Conversation);
}
