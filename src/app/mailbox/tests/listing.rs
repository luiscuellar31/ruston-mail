//! The open folder's list: its first page, more pages, refreshes and counts.

use super::*;

/// Confirms moving each of `rows` to Archive, from the open Inbox.
fn archive_confirmed(mailbox: &mut Mailbox, rows: &[&str], first_request: u64) {
    for (request, row) in (first_request..).zip(rows) {
        let moved = mailbox
            .start_action_on(
                (*row).into(),
                SummaryKind::Conversation,
                MailAction::MoveTo(sys(MailFolder::Archive)),
                request,
            )
            .and_then(|start| start.send)
            .unwrap();
        let _ = mailbox.finish_action(&moved, Ok(()));
    }
}

#[test]
fn a_failure_remembers_what_it_asked_for() {
    let mut mailbox = loaded_inbox(&["a"], 100);
    let refresh = mailbox.refresh(3).unwrap();
    mailbox.finish_page(refresh.id, Err(MailboxError::Connection));
    assert_eq!(
        mailbox.status(),
        ListStatus::Failed(MailboxError::Connection, ListAttempt::Folder)
    );

    let mut mailbox = loaded_inbox(&["a"], 100);
    let more = mailbox.load_more(3).unwrap();
    mailbox.finish_page(more.id, Err(MailboxError::Service));
    assert_eq!(
        mailbox.status(),
        ListStatus::Failed(MailboxError::Service, ListAttempt::More)
    );

    let mut mailbox = searchable_mailbox();
    mailbox.set_search_query("rust".into());
    let search = mailbox.start_search(3).unwrap();
    mailbox.finish_search(&search, Err(MailboxError::Connection));
    assert_eq!(
        mailbox.status(),
        ListStatus::Failed(MailboxError::Connection, ListAttempt::Search)
    );
}

#[test]
fn refreshing_keeps_the_pages_already_loaded() {
    let (mut mailbox, request) = open();
    mailbox.finish_page(request.id, full_page(0, 120));
    let more = mailbox.load_more(5).unwrap();
    mailbox.finish_page(more.id, full_page(PAGE_SIZE, 120));
    assert_eq!(mailbox.conversations().len(), 2 * PAGE_SIZE as usize);

    let refresh = mailbox.refresh(6).unwrap();
    mailbox.finish_page(refresh.id, full_page(0, 120));

    // Both pages survive, so refreshing does not send the user back to
    // the first fifty rows.
    assert_eq!(mailbox.conversations().len(), 2 * PAGE_SIZE as usize);
    assert_eq!(mailbox.loaded_conversation_limit(), 2 * PAGE_SIZE);
    assert!(mailbox.has_more());

    let more = mailbox.load_more(7).unwrap();
    mailbox.finish_page(
        more.id,
        dated_page(&[("p99", page_time(99)), ("p100", page_time(100))], 120),
    );
    assert_eq!(mailbox.conversations().len(), 2 * PAGE_SIZE as usize + 1);
    assert_eq!(mailbox.conversations().last().unwrap().id, "p100");

    // A short reload means the folder itself is short now.
    let refresh = mailbox.refresh(8).unwrap();
    mailbox.finish_page(refresh.id, dated_page(&[("p0", page_time(0))], 1));
    assert_eq!(visible_ids(&mailbox), ["p0"]);
}

#[test]
fn a_lost_counts_response_never_blocks_later_refreshes() {
    let mut mailbox = loaded_inbox(&["a"], 1);
    let counts =
        |unread: u32| -> MailboxCounts { [(sys(MailFolder::Inbox), unread)].into_iter().collect() };

    // The response for the first request never arrives.
    assert_eq!(mailbox.refresh_counts(3), 3);
    assert_eq!(mailbox.refresh_counts(4), 4);

    // Arriving late, it is ignored in favour of the newest request.
    assert_eq!(mailbox.finish_counts(3, Ok(counts(7))), None);
    assert!(mailbox.counts().is_none());

    assert_eq!(mailbox.finish_counts(4, Ok(counts(2))), None);
    assert_eq!(
        mailbox.counts().unwrap().unread(&sys(MailFolder::Inbox)),
        Some(2)
    );
}

#[test]
fn opening_loads_first_inbox_page_with_unknown_counts() {
    let (mailbox, request) = open();

    assert_eq!(
        request,
        PageRequest {
            id: 1,
            folder: Folder::INBOX,
            page: 0,
            page_size: PAGE_SIZE,
        }
    );
    assert_eq!(mailbox.status(), ListStatus::Loading(1));
    assert_eq!(mailbox.counts(), None);
    assert!(mailbox.reader().is_none());
}

#[test]
fn loading_then_loaded() {
    let mailbox = loaded_inbox(&["a", "b"], 2);

    assert_eq!(mailbox.status(), ListStatus::Loaded);
    assert_eq!(ids(&mailbox), ["a", "b"]);
    assert!(!mailbox.has_more());
}

#[test]
fn loading_then_error() {
    let (mut mailbox, request) = open();

    let error = mailbox.finish_page(request.id, Err(MailboxError::Connection));

    assert_eq!(error, Some(MailboxError::Connection));
    assert_eq!(
        mailbox.status(),
        ListStatus::Failed(MailboxError::Connection, ListAttempt::Folder)
    );
    assert!(mailbox.conversations().is_empty());
}

#[test]
fn load_more_appends_without_duplicates() {
    let mut mailbox = loaded_inbox(&["a", "b"], 120);
    assert!(mailbox.has_more());

    let request = mailbox.load_more(3).unwrap();
    assert_eq!(request.page, 1);
    assert_eq!(mailbox.status(), ListStatus::LoadingMore(3));
    assert_eq!(ids(&mailbox), ["a", "b"]);

    mailbox.finish_page(request.id, page(&["b", "c"], 120));

    assert_eq!(ids(&mailbox), ["a", "b", "c"]);
    assert!(mailbox.has_more());
    assert_eq!(mailbox.load_more(4).unwrap().page, 2);
}

#[test]
fn load_more_preserves_the_open_reader_on_success_and_failure() {
    for (case, result) in [
        ("a page", page(&["c"], 3)),
        ("a failure", Err(MailboxError::Connection)),
    ] {
        let mut mailbox = loaded_inbox(&["a", "b"], 120);
        load_detail(&mut mailbox, "b", detail("b", &["b1", "b2"]), 9);
        mailbox.toggle_message("b1");
        mailbox.toggle_quote("b2", 0);
        let reader = mailbox.reader_state().clone();

        let request = mailbox.load_more(10).unwrap();
        assert_eq!(mailbox.reader_state(), &reader, "{case}");
        mailbox.finish_page(request.id, result);
        assert_eq!(mailbox.reader_state(), &reader, "{case}");
    }
}

#[test]
fn page_size_controls_whether_more_pages_exist() {
    let (mut mailbox, request) = open_with_page_size(2);
    mailbox.finish_page(request.id, page(&["a", "b"], 3));
    assert!(mailbox.has_more());

    let request = mailbox.load_more(3).unwrap();
    assert_eq!(request.page_size, 2);
    mailbox.finish_page(request.id, page(&["c"], 3));

    assert!(!mailbox.has_more());
}

#[test]
fn duplicate_load_more_is_prevented() {
    let mut mailbox = loaded_inbox(&["a"], 120);

    assert!(mailbox.load_more(3).is_some());
    assert_eq!(mailbox.load_more(4), None);
    assert_eq!(mailbox.refresh(5), None);
    assert_eq!(mailbox.status(), ListStatus::LoadingMore(3));
}

#[test]
fn load_more_requires_another_page() {
    let mut mailbox = loaded_inbox(&["a"], 1);

    assert_eq!(mailbox.load_more(3), None);
}

#[test]
fn failed_load_more_keeps_loaded_conversations() {
    let mut mailbox = loaded_inbox(&["a"], 120);
    let request = mailbox.load_more(3).unwrap();

    mailbox.finish_page(request.id, Err(MailboxError::Connection));

    assert_eq!(ids(&mailbox), ["a"]);
    assert_eq!(mailbox.load_more(4).unwrap().page, 1);
}

#[test]
fn refresh_replaces_list_only_after_success() {
    let mut mailbox = loaded_inbox(&["a", "b"], 2);
    mailbox.start_conversation_load("b".into(), 9);

    let request = mailbox.refresh(3).unwrap();
    assert_eq!(request.page, 0);
    assert_eq!(mailbox.status(), ListStatus::Refreshing(3));
    assert_eq!(ids(&mailbox), ["a", "b"]);

    mailbox.finish_page(request.id, page(&["c", "b"], 2));

    assert_eq!(ids(&mailbox), ["c", "b"]);
    assert_eq!(mailbox.selected_conversation(), Some("b"));
}

#[test]
fn refresh_keeps_reader_when_row_is_absent_from_the_page() {
    let mut mailbox = loaded_inbox(&["a", "b"], 2);
    load_detail(&mut mailbox, "b", detail("b", &["b1"]), 9);
    let reader = mailbox.reader_state().clone();

    let request = mailbox.refresh(3).unwrap();
    mailbox.finish_page(request.id, page(&["a"], 1));

    assert_eq!(mailbox.reader_state(), &reader);
    assert!(mailbox.reader_stamp.is_none());

    // Typing keeps it too; folder navigation closes it.
    mailbox.set_search_query("no match".into());
    assert_eq!(mailbox.selected_conversation(), Some("b"));
    mailbox.set_search_query(String::new());
    assert_eq!(mailbox.selected_conversation(), Some("b"));
    load_detail(&mut mailbox, "a", detail("a", &["a1"]), 10);
    mailbox.select_folder(sys(MailFolder::Sent), 11);
    assert_eq!(mailbox.selected_conversation(), None);
}

#[test]
fn failed_refresh_keeps_previous_list() {
    let mut mailbox = loaded_inbox(&["a"], 1);
    let request = mailbox.refresh(3).unwrap();

    mailbox.finish_page(request.id, Err(MailboxError::Service));

    assert_eq!(ids(&mailbox), ["a"]);
    assert_eq!(
        mailbox.status(),
        ListStatus::Failed(MailboxError::Service, ListAttempt::Folder)
    );
}

#[test]
fn counts_apply_once() {
    let (mut mailbox, _) = open();

    // `open` left its counts request in flight; its response is the one that
    // counts.
    let counts: MailboxCounts = [(sys(MailFolder::Inbox), 4)].into_iter().collect();
    mailbox.finish_counts(OPENING_COUNTS, Ok(counts));
    assert_eq!(
        mailbox.counts().unwrap().unread(&sys(MailFolder::Inbox)),
        Some(4)
    );
    assert_eq!(
        mailbox.counts().unwrap().unread(&sys(MailFolder::Sent)),
        None
    );

    assert_eq!(
        mailbox.finish_counts(OPENING_COUNTS, Ok(MailboxCounts::default())),
        None
    );
    assert_eq!(
        mailbox.counts().unwrap().unread(&sys(MailFolder::Inbox)),
        Some(4)
    );
    assert_eq!(mailbox.refresh_counts(3), 3);
}

#[test]
fn an_empty_page_offers_no_next_one_whatever_the_total_says() {
    let (mut mailbox, request) = open();
    mailbox.finish_page(request.id, page_of(Vec::new(), 120));
    assert!(!mailbox.has_more(), "the folder ran out before its total");

    let mut mailbox = loaded_inbox(&["a"], 120);
    mailbox.set_search_query("invoice".into());
    let search = mailbox.start_search(3).unwrap();
    assert_eq!(
        mailbox.finish_search(&search, page_of(Vec::new(), 40)),
        None
    );
    assert!(
        !mailbox.has_more(),
        "the results ran out before their total"
    );
}

#[test]
fn loading_more_after_moves_out_does_not_skip_the_rows_that_moved_up() {
    let (mut mailbox, first) = open();
    mailbox.finish_page(first.id, full_page(0, 120));
    archive_confirmed(&mut mailbox, &["p10", "p20", "p30"], 3);

    // Proton's first page now reaches three rows further, to p52.
    let more = mailbox.load_more(10).unwrap();
    assert_eq!(more.page, 0);
    let rows = (0..53)
        .filter(|number| ![10, 20, 30].contains(number))
        .map(|number| dated(&format!("p{number}"), page_time(number)))
        .collect();
    mailbox.finish_page(more.id, page_of(rows, 117));
    let ids = visible_ids(&mailbox);
    assert_eq!(ids.len(), PAGE_SIZE as usize);
    assert_eq!(&ids[47..], ["p50", "p51", "p52"]);

    // The next page follows the one just loaded.
    assert_eq!(mailbox.load_more(11).unwrap().page, 1);
}

#[test]
fn a_move_of_a_row_only_a_search_found_leaves_the_pages_as_they_were() {
    let (mut mailbox, first) = open();
    mailbox.finish_page(first.id, full_page(0, 120));
    mailbox.set_search_query("invoice".into());
    let search = mailbox.start_search(3).unwrap();
    mailbox.finish_search(&search, page(&["found"], 1));
    archive_confirmed(&mut mailbox, &["found"], 4);

    // The loaded pages never held it, so Proton's later pages did not move.
    mailbox.set_search_query(String::new());
    assert_eq!(mailbox.load_more(6).unwrap().page, 1);
}

#[test]
fn moves_that_did_not_leave_keep_load_more_on_the_next_page() {
    let (mut mailbox, first) = open();
    mailbox.finish_page(first.id, full_page(0, 120));
    let refused = mailbox
        .start_action_on(
            "p10".into(),
            SummaryKind::Conversation,
            MailAction::MoveTo(sys(MailFolder::Archive)),
            3,
        )
        .and_then(|start| start.send)
        .unwrap();
    let _ = mailbox.finish_action(&refused, Err(MailboxError::Service));
    archive_confirmed(&mut mailbox, &["p20"], 4);
    let undo = mailbox.take_undo().unwrap();
    let back = mailbox
        .start_move_back(undo, 5)
        .and_then(|start| start.send)
        .unwrap();
    let _ = mailbox.finish_action(&back, Ok(()));

    // One move was refused and the other taken back: nothing left the pages.
    assert_eq!(mailbox.load_more(6).unwrap().page, 1);
}
