//! Search: filtering as a query is typed, Proton's results, actions on them.

use super::*;

#[test]
fn server_results_replace_the_folder_listing() {
    let mut mailbox = loaded_inbox(&["a", "b"], 120);
    mailbox.set_search_query("invoice".into());

    let request = mailbox.start_search(3).expect("a query reaches the server");
    assert_eq!(request.query, "invoice");

    // Results stand on their own: what came back is not narrowed again.
    assert_eq!(mailbox.finish_search(&request, page(&["found"], 1)), None);
    assert_eq!(visible_ids(&mailbox), ["found"]);
    assert_eq!(mailbox.shown_search_query(), Some("invoice"));

    // One match needs no next search page, even though the folder has more.
    assert_eq!(mailbox.load_more(4), None);

    // Emptying the field puts the folder back.
    mailbox.set_search_query(String::new());
    assert_eq!(visible_ids(&mailbox), ["a", "b"]);
    assert_eq!(mailbox.shown_search_query(), None);
}

#[test]
fn search_loads_more_than_fifty_results_without_losing_the_first_page() {
    let mut mailbox = loaded_inbox(&["inbox"], 1);
    mailbox.set_search_query("invoice".into());
    let first = mailbox.start_search(3).unwrap();
    assert_eq!(first.page, 0);
    assert_eq!(
        mailbox.finish_search(&first, full_page(0, PAGE_SIZE + 1)),
        None
    );
    assert_eq!(visible_ids(&mailbox).len(), PAGE_SIZE as usize);
    assert!(mailbox.has_more());
    assert_eq!(mailbox.load_more(4), None);

    let next = mailbox.load_more_search(4).unwrap();
    assert_eq!(next.page, 1);
    assert_eq!(next.page_size, PAGE_SIZE);
    assert!(mailbox.load_more_search(5).is_none());
    assert_eq!(mailbox.finish_search(&first, page(&["stale"], 1)), None);
    assert_eq!(
        mailbox.finish_search(&next, page(&["p50"], PAGE_SIZE + 1)),
        None
    );

    assert_eq!(visible_ids(&mailbox).len(), PAGE_SIZE as usize + 1);
    assert_eq!(visible_ids(&mailbox).last(), Some(&"p50"));
    assert!(!mailbox.has_more());
    assert!(mailbox.load_more_search(6).is_none());
}

#[test]
fn search_pages_reuse_the_id_index_after_a_row_leaves_the_results() {
    let (mut mailbox, inbox) = open_with_page_size(2);
    mailbox.finish_page(inbox.id, page(&["inbox"], 1));
    mailbox.set_search_query("invoice".into());
    let first = mailbox.start_search(3).unwrap();
    mailbox.finish_search(&first, page(&["a", "b"], 5));
    mailbox.record_action("a", &MailAction::MoveTo(sys(MailFolder::Trash)));

    let next = mailbox.load_more_search(4).unwrap();
    mailbox.finish_search(&next, page(&["a", "b", "c"], 5));

    assert_eq!(visible_ids(&mailbox), ["b", "a", "c"]);
}

#[test]
fn an_abandoned_search_never_lands() {
    let mut mailbox = loaded_inbox(&["a"], 1);
    mailbox.set_search_query("invoice".into());
    let abandoned = mailbox.start_search(3).unwrap();

    // The user gave up before the answer came back.
    mailbox.set_search_query(String::new());

    assert_eq!(mailbox.finish_search(&abandoned, page(&["found"], 1)), None);
    assert_eq!(visible_ids(&mailbox), ["a"]);
    assert!(mailbox.shown_search_query().is_none());
}

#[test]
fn an_empty_query_leaves_the_results_behind() {
    let mut mailbox = loaded_inbox(&["a"], 1);
    mailbox.set_search_query("invoice".into());
    let request = mailbox.start_search(3).unwrap();
    let _ = mailbox.finish_search(&request, page(&["found"], 1));

    mailbox.set_search_query("   ".into());

    assert!(mailbox.start_search(4).is_none());
    assert_eq!(visible_ids(&mailbox), ["a"]);
}

#[test]
fn a_conversation_a_search_found_can_be_acted_on() {
    let mut mailbox = searched(&["a"], &["found"], "found");

    // The row is nowhere in the folder listing, so Proton is told to read
    // it across every folder rather than in the open one.
    let request = mailbox
        .start_action(MailAction::SetStarred(true), 3)
        .and_then(|start| start.send)
        .expect("a search result takes actions like any other row");
    assert_eq!(request.row_id, "found");
    assert_eq!(request.context, None);

    assert_eq!(mailbox.finish_action(&request, Ok(())).result, Ok(()));
    assert!(
        mailbox
            .selected_summary()
            .expect("the open row is still the search result")
            .starred
    );
}

#[test]
fn a_search_result_is_marked_read_like_any_other_row() {
    let mut mailbox = searched(&["a"], &["found"], "found");

    let request = mailbox
        .start_mark_read(3)
        .expect("opening a search result marks it read");
    assert_eq!(request.context, None);

    assert!(mailbox.finish_mark_read(&request, Ok(())).read_confirmed);
    assert!(!mailbox.selected_summary().unwrap().unread);
}

#[test]
fn a_row_in_both_listings_changes_in_both() {
    // The search found a row the open folder also lists.
    let mut mailbox = searched(&["a"], &["a"], "a");

    // It is listed in the folder, so the open folder is the context.
    let request = send(&mut mailbox, MailAction::SetStarred(true), 3);
    assert_eq!(request.context.as_ref(), Some(&sys(MailFolder::Inbox)));
    assert_eq!(mailbox.finish_action(&request, Ok(())).result, Ok(()));

    // Both copies agree, so the folder listing holds no stale row waiting
    // for the search to be left behind.
    assert!(mailbox.selected_summary().unwrap().starred);
    assert!(mailbox.conversations().iter().all(|row| row.starred));
}

#[test]
fn a_row_moved_out_leaves_the_search_results_too() {
    let mut mailbox = searched(&["a"], &["a", "b"], "a");

    let start = mailbox
        .start_action(MailAction::MoveTo(sys(MailFolder::Archive)), 3)
        .unwrap();

    // Reading goes on with the next result, and the moved row is gone
    // from the results and from the folder listing alike.
    assert_eq!(start.opened.as_deref(), Some("b"));
    assert!(!mailbox.has_row("a"));
    assert!(mailbox.conversations().is_empty());
    let _ = mailbox.finish_action(&start.send.unwrap(), Ok(()));
    assert!(!mailbox.has_row("a"));
}

#[test]
fn matching_ignores_case_with_or_without_ascii() {
    // The query arrives lowercased, as the mailbox trims and lowers it.
    assert!(contains_ignoring_case("Quarterly Roadmap", "roadmap"));
    assert!(contains_ignoring_case("ALICE@EXAMPLE.COM", "alice@"));
    assert!(!contains_ignoring_case("roadmap", "budget"));
    // A needle longer than what it is looked for in matches nothing.
    assert!(!contains_ignoring_case("hi", "hiring"));
    // An empty query narrows nothing, so everything matches it.
    assert!(contains_ignoring_case("", ""));

    // Text where case is more than one byte takes the slower path and
    // must still match.
    assert!(contains_ignoring_case("Ángel Ruíz", "ángel"));
    assert!(contains_ignoring_case("Ángel Ruíz", "ruíz"));
    assert!(contains_ignoring_case("Ángel Ruíz", "uíz"));
    assert!(contains_ignoring_case("ÉCOLE", "école"));
    assert!(contains_ignoring_case("CAFÉ DE PARIS", "paris"));
    assert!(contains_ignoring_case("CAFÉ DE PARIS", "café"));
    assert!(contains_ignoring_case("CAFÉ DE PARIS", "afé"));
    assert!(!contains_ignoring_case("Ángel", "ünsal"));
    // And an ASCII line cannot hold a word that is not ASCII.
    assert!(!contains_ignoring_case("Angel", "ángel"));
    assert!(!contains_ignoring_case("hello world", "café"));
}

#[test]
fn results_on_screen_offer_no_next_page() {
    // A total past one page is what leaves a next page to ask for.
    let mut mailbox = loaded_inbox(&["a"], PAGE_SIZE * 3);
    assert!(mailbox.has_more(), "the folder has more pages waiting");

    mailbox.set_search_query("report".to_owned());
    let search = mailbox.start_search(3).unwrap();
    assert_eq!(mailbox.finish_search(&search, page(&["found"], 1)), None);

    // The server answered in one batch, so offering to load more would
    // offer something nothing can ask for.
    assert!(!mailbox.has_more());
    assert!(mailbox.load_more(4).is_none());

    // The folder's own pages are still there once the results are gone.
    mailbox.clear_search();
    assert!(mailbox.has_more());
}

#[test]
fn moving_a_row_only_a_search_found_offers_no_undo() {
    let mut mailbox = searched(&["a"], &["found", "next"], "found");

    let start = mailbox
        .start_action(MailAction::MoveTo(sys(MailFolder::Archive)), 3)
        .unwrap();
    assert_eq!(start.opened.as_deref(), Some("next"));
    let _ = mailbox.finish_action(&start.send.unwrap(), Ok(()));

    // The search listed every folder at once, so the Inbox is not where
    // this row came from and putting it there would be a move of its own.
    assert!(mailbox.undo().is_none());
    assert!(!mailbox.has_row("found"));
}

#[test]
fn refreshing_keeps_the_search_result_that_is_open() {
    let mut mailbox = searched(&["a"], &["found"], "found");

    // A reload answers for the folder, which the results are not part of.
    let refresh = mailbox.refresh(3).expect("a search does not block reload");
    assert_eq!(mailbox.finish_page(refresh.id, page(&["a"], 1)), None);

    assert_eq!(mailbox.selected_conversation(), Some("found"));
}

#[test]
fn a_loaded_row_is_found_even_while_a_search_hides_it() {
    let mut mailbox = loaded_inbox(&["a", "b"], 2);

    assert!(mailbox.has_row("a"));
    assert!(!mailbox.has_row("missing"));

    // Hiding a row from view does not take it out of the loaded list, so
    // an action on it knows the row never left the folder.
    mailbox.set_search_query("nothing matches this".into());

    assert_eq!(mailbox.visible_position("a"), None);
    assert!(mailbox.has_row("a"));
}

#[test]
fn typing_in_the_search_leaves_the_open_conversation_open() {
    let mut mailbox = searchable_mailbox();
    load_detail(&mut mailbox, "sender", detail("sender", &["s1"]), 9);

    for typed in ["r", "ru", "rus"] {
        mailbox.set_search_query(typed.into());
        assert_eq!(mailbox.selected_conversation(), Some("sender"), "{typed}");
    }
    assert_eq!(visible_ids(&mailbox), ["subject"]);
    // Deleting what hid it brings its row back, still open.
    mailbox.set_search_query("r".into());
    assert!(visible_ids(&mailbox).contains(&"sender"));
    assert_eq!(mailbox.selected_conversation(), Some("sender"));
}

#[test]
fn moving_a_conversation_the_search_hides_opens_nothing_in_its_place() {
    let mut mailbox = searchable_mailbox();
    load_detail(&mut mailbox, "sender", detail("sender", &["s1"]), 9);
    mailbox.set_search_query("rust".into());

    let start = mailbox
        .start_action(MailAction::MoveTo(sys(MailFolder::Archive)), 10)
        .unwrap();

    assert_eq!(start.opened, None);
    assert_eq!(mailbox.selected_conversation(), None);
    assert_eq!(visible_ids(&mailbox), ["subject"]);
}

#[test]
fn search_does_not_mutate_loaded_detail() {
    let mut mailbox = searchable_mailbox();
    let conversation = detail("subject", &["message"]);
    load_detail(&mut mailbox, "subject", conversation.clone(), 3);

    mailbox.set_search_query("rust".into());
    assert_eq!(mailbox.reader().unwrap().detail(), &conversation);

    mailbox.set_search_query(String::new());
    assert_eq!(mailbox.reader().unwrap().detail(), &conversation);
}

#[test]
fn empty_search_returns_normal_folder_contents() {
    let mailbox = searchable_mailbox();

    assert_eq!(visible_ids(&mailbox), ["sender", "subject", "preview"]);
}

#[test]
fn search_is_case_insensitive() {
    let mut mailbox = searchable_mailbox();

    mailbox.set_search_query("rUsT".into());

    assert_eq!(visible_ids(&mailbox), ["subject"]);
}

#[test]
fn search_matches_sender_name() {
    let mut mailbox = searchable_mailbox();

    mailbox.set_search_query("alice stone".into());

    assert_eq!(visible_ids(&mailbox), ["sender"]);
}

#[test]
fn search_matches_sender_email() {
    let mut mailbox = searchable_mailbox();

    mailbox.set_search_query("alice@example.com".into());

    assert_eq!(visible_ids(&mailbox), ["sender"]);
}

#[test]
fn search_matches_subject() {
    let mut mailbox = searchable_mailbox();

    mailbox.set_search_query("project plan".into());

    assert_eq!(visible_ids(&mailbox), ["subject"]);
}

#[test]
fn search_matches_preview() {
    let mut mailbox = searchable_mailbox();

    mailbox.set_search_query("launch checklist".into());

    assert_eq!(visible_ids(&mailbox), ["preview"]);
}

#[test]
fn search_ignores_surrounding_whitespace() {
    let mut mailbox = searchable_mailbox();

    mailbox.set_search_query("  rust  ".into());

    assert_eq!(visible_ids(&mailbox), ["subject"]);
}

#[test]
fn search_excludes_non_matching_conversations() {
    let mut mailbox = searchable_mailbox();

    mailbox.set_search_query("no such conversation".into());

    assert!(visible_ids(&mailbox).is_empty());
}

#[test]
fn search_applies_to_the_selected_folder() {
    let mut mailbox = searchable_mailbox();
    mailbox.set_search_query("rust".into());
    let request = mailbox.select_folder(sys(MailFolder::Archive), 3).unwrap();
    mailbox.finish_page(
        request.id,
        page_of(
            vec![searchable_summary(
                "archived",
                "Archived Sender",
                "archive@example.com",
                "Archived Rust notes",
                "Old project",
            )],
            1,
        ),
    );

    assert_eq!(mailbox.folder(), &sys(MailFolder::Archive));
    assert_eq!(visible_ids(&mailbox), ["archived"]);
}

#[test]
fn clearing_a_search_keeps_a_failure_of_the_folder_itself() {
    let mut mailbox = searchable_mailbox();
    let refresh = mailbox.refresh(9).unwrap();
    mailbox.finish_page(refresh.id, Err(MailboxError::Connection));
    mailbox.set_search_query("rust".into());
    mailbox.set_search_query(String::new());
    assert_eq!(
        mailbox.status(),
        ListStatus::Failed(MailboxError::Connection, ListAttempt::Folder)
    );

    // The search's own failure goes with it.
    let mut mailbox = searchable_mailbox();
    mailbox.set_search_query("rust".into());
    let search = mailbox.start_search(3).unwrap();
    mailbox.finish_search(&search, Err(MailboxError::Connection));
    mailbox.set_search_query(String::new());
    assert_eq!(mailbox.status(), ListStatus::Loaded);
}

#[test]
fn clearing_search_restores_normal_results() {
    let mut mailbox = searchable_mailbox();
    mailbox.set_search_query("rust".into());

    mailbox.set_search_query(String::new());

    assert_eq!(visible_ids(&mailbox), ["sender", "subject", "preview"]);
}

#[test]
fn typing_keeps_a_conversation_that_is_still_loading() {
    let mut mailbox = searchable_mailbox();
    mailbox.start_conversation_load("sender".into(), 3);

    mailbox.set_search_query("rust".into());

    assert_eq!(mailbox.selected_conversation(), Some("sender"));
    assert_eq!(visible_ids(&mailbox), ["subject"]);
}

#[test]
fn search_does_not_change_mailbox_counts() {
    let mut mailbox = searchable_mailbox();
    let counts: MailboxCounts = [(sys(MailFolder::Inbox), 7)].into_iter().collect();
    mailbox.finish_counts(OPENING_COUNTS, Ok(counts.clone()));

    mailbox.set_search_query("rust".into());
    mailbox.set_search_query("missing".into());
    mailbox.set_search_query(String::new());

    assert_eq!(mailbox.counts(), Some(&counts));
}

#[test]
fn repeated_search_does_not_mutate_source_conversations() {
    let mut mailbox = searchable_mailbox();
    let original = mailbox.conversations().to_vec();

    for query in ["rust", "alice", "missing", ""] {
        mailbox.set_search_query(query.into());
        let _ = mailbox.visible_conversations().count();
    }

    assert_eq!(mailbox.conversations(), original);
}

#[test]
fn visible_accessors_and_navigation_use_cache() {
    let mut mailbox = searchable_mailbox();
    assert_eq!(mailbox.visible_count(), 3);
    assert!(!mailbox.is_empty_visible());
    assert_eq!(
        mailbox.visible_row(0).map(|r| r.id.as_str()),
        Some("sender")
    );
    assert_eq!(
        mailbox.visible_row(1).map(|r| r.id.as_str()),
        Some("subject")
    );
    assert_eq!(
        mailbox.visible_row(2).map(|r| r.id.as_str()),
        Some("preview")
    );
    assert_eq!(mailbox.visible_row(3), None);

    // visible_position works with cached indices
    assert_eq!(mailbox.visible_position("sender"), Some(0.0));
    assert_eq!(mailbox.visible_position("subject"), Some(0.5));
    assert_eq!(mailbox.visible_position("preview"), Some(1.0));
    assert_eq!(mailbox.visible_position("nonexistent"), None);

    // neighbour works with cached indices
    load_detail(&mut mailbox, "sender", detail("sender", &["m1"]), 10);
    assert_eq!(mailbox.neighbour(Step::Next), Some("subject".into()));

    // Filtering narrows cache
    mailbox.set_search_query("chen".into());
    assert_eq!(mailbox.visible_count(), 1);
    assert_eq!(
        mailbox.visible_row(0).map(|r| r.id.as_str()),
        Some("subject")
    );
    assert_eq!(mailbox.visible_position("subject"), None);

    // Clearing query restores cache
    mailbox.set_search_query(String::new());
    assert_eq!(mailbox.visible_count(), 3);
}
