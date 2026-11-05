//! Actions on rows: moves, flags and labels, undo, and the order they go in.

use super::*;

/// A mailbox showing `view` with rows "a" and "b", "a" open.
fn viewing(view: Folder) -> Mailbox {
    let mut mailbox = loaded_inbox(&["x"], 1);
    let request = mailbox.select_folder(view, 2).unwrap();
    mailbox.finish_page(request.id, page(&["a", "b"], 2));
    load_detail(&mut mailbox, "a", detail("a", &["a1"]), 3);
    mailbox
}

fn selected_in(folder: MailFolder, ids: &[&str], selected: &str) -> Mailbox {
    let (mut mailbox, inbox) = open();
    // Fixtures issue IDs from 80, clear of the 3, 4 and on that tests issue.
    let listing = if folder == MailFolder::Inbox {
        inbox.id
    } else {
        mailbox.select_folder(sys(folder), 80).unwrap().id
    };
    mailbox.finish_page(listing, page(ids, ids.len() as u32));
    let load = mailbox
        .start_conversation_load(selected.into(), 82)
        .unwrap();
    mailbox.finish_conversation(&load, Ok(detail(selected, &["message"])));
    mailbox
}

/// A two-page Inbox with `row` archived and the archive confirmed.
fn archived_from_a_deeper_page(row: &str) -> Mailbox {
    let (mut mailbox, first) = open();
    mailbox.finish_page(first.id, full_page(0, 120));
    let more = mailbox.load_more(3).unwrap();
    mailbox.finish_page(more.id, full_page(PAGE_SIZE, 120));
    load_detail(&mut mailbox, row, detail(row, &[row]), 4);
    let archive = mailbox
        .start_action(MailAction::MoveTo(sys(MailFolder::Archive)), 5)
        .and_then(|start| start.send)
        .unwrap();
    let _ = mailbox.finish_action(&archive, Ok(()));
    assert!(!mailbox.has_row(row));
    mailbox
}

#[test]
fn a_refresh_keeps_the_offer_to_undo() {
    let mut mailbox = loaded_inbox(&["a", "b"], 2);
    load_detail(&mut mailbox, "a", detail("a", &["a1"]), 3);
    let request = send(
        &mut mailbox,
        MailAction::MoveTo(sys(MailFolder::Archive)),
        4,
    );
    let _ = mailbox.finish_action(&request, Ok(()));

    // The moved row is already gone, so a reload changes nothing the offer
    // describes.
    let refresh = mailbox.refresh(5).unwrap();
    assert!(mailbox.undo().is_some());

    mailbox.finish_page(refresh.id, page(&["b"], 1));
    let undo = mailbox.take_undo().expect("the offer outlives the refresh");
    assert_eq!(undo.row_id, "a");
    assert!(
        mailbox
            .start_action_on(undo.row_id, undo.kind, MailAction::MoveTo(undo.from), 6)
            .is_some()
    );
}

#[test]
fn a_move_out_of_a_folder_can_be_taken_back() {
    let mut mailbox = loaded_inbox(&["a", "b"], 2);
    load_detail(&mut mailbox, "a", detail("a", &["a1"]), 3);
    let start = mailbox
        .start_action(MailAction::MoveTo(sys(MailFolder::Archive)), 4)
        .unwrap();

    // The move shows before Proton answers: the row leaves, the next one
    // opens, and the move can be taken back at once.
    assert_eq!(start.opened.as_deref(), Some("b"));
    let undo = mailbox.undo().expect("a move out of the Inbox undoes");
    assert_eq!(undo.row_id, "a");
    assert_eq!(undo.from, sys(MailFolder::Inbox));
    assert_eq!(undo.to, sys(MailFolder::Archive));
    let finish = mailbox.finish_action(&start.send.unwrap(), Ok(()));
    assert_eq!(finish.result, Ok(()));
    assert!(
        mailbox.undo().is_some(),
        "confirming the move keeps the offer"
    );

    // The offer is claimed once, and the next action replaces it.
    assert!(mailbox.take_undo().is_some());
    assert!(mailbox.undo().is_none());
}

#[test]
fn archived_mail_can_go_back_to_the_inbox_and_that_can_be_taken_back_too() {
    let mut mailbox = viewing(sys(MailFolder::Archive));

    let start = mailbox
        .start_action(MailAction::MoveTo(Folder::INBOX), 4)
        .unwrap();
    assert_eq!(start.opened.as_deref(), Some("b"));
    let _ = mailbox.finish_action(&start.send.unwrap(), Ok(()));

    let undo = mailbox.undo().expect("a move back to the Inbox undoes");
    assert_eq!(undo.from, sys(MailFolder::Archive));
    assert_eq!(undo.to, Folder::INBOX);
}

#[test]
fn mail_moved_to_another_folder_stays_under_its_star_or_label() {
    let invoices = Folder::custom("kZ9", "Invoices");
    for view in [sys(MailFolder::Starred), Folder::label("wN2", "Receipts")] {
        for to in [sys(MailFolder::Archive), Folder::INBOX, invoices.clone()] {
            let mut mailbox = viewing(view.clone());
            let start = mailbox
                .start_action(MailAction::MoveTo(to.clone()), 4)
                .unwrap();

            assert_eq!(start.opened, None, "{view:?} to {to:?}");
            assert!(mailbox.has_row("a"), "{view:?} to {to:?}");
            // The reader knows where it went, so it offers the way back
            // instead of the move just made.
            assert_eq!(mailbox.selected_location(), Some(&to), "{view:?}");
            let _ = mailbox.finish_action(&start.send.unwrap(), Ok(()));
            assert!(mailbox.has_row("a"), "{view:?} to {to:?}");
            assert_eq!(mailbox.selected_location(), Some(&to), "{view:?}");
        }

        // Spam and Trash still take the row away.
        for to in [MailFolder::Spam, MailFolder::Trash] {
            let mut mailbox = viewing(view.clone());
            let start = mailbox
                .start_action(MailAction::MoveTo(sys(to)), 4)
                .unwrap();
            assert_eq!(start.opened.as_deref(), Some("b"), "{view:?} to {to:?}");
            assert!(!mailbox.has_row("a"), "{view:?} to {to:?}");
        }
    }
}

#[test]
fn a_refused_move_under_a_label_forgets_where_the_row_went() {
    let mut mailbox = viewing(sys(MailFolder::Starred));
    let request = send(
        &mut mailbox,
        MailAction::MoveTo(sys(MailFolder::Archive)),
        4,
    );
    let _ = mailbox.finish_action(&request, Err(MailboxError::Service));

    assert!(mailbox.has_row("a"));
    assert_eq!(mailbox.selected_location(), None);
}

#[test]
fn taking_back_a_move_out_of_a_label_view_asks_for_the_row_again() {
    let mut mailbox = viewing(sys(MailFolder::Starred));
    let archived = send(
        &mut mailbox,
        MailAction::MoveTo(sys(MailFolder::Archive)),
        4,
    );
    let _ = mailbox.finish_action(&archived, Ok(()));
    let trashed = send(&mut mailbox, MailAction::MoveTo(sys(MailFolder::Trash)), 5);
    let _ = mailbox.finish_action(&trashed, Ok(()));
    let undo = mailbox
        .undo()
        .cloned()
        .expect("it was known to be in Archive");
    assert_eq!(undo.from, sys(MailFolder::Archive));

    let back = mailbox
        .start_action_on(undo.row_id, undo.kind, MailAction::MoveTo(undo.from), 6)
        .and_then(|start| start.send)
        .unwrap();
    let _ = mailbox.finish_action(&back, Ok(()));

    assert!(!mailbox.has_row("a"));
    assert!(mailbox.brought_back(&back), "only a reload can show it");
}

#[test]
fn only_a_folder_that_lists_the_row_says_where_it_is() {
    let mut mailbox = loaded_inbox(&["a"], 1);
    load_detail(&mut mailbox, "a", detail("a", &["a1"]), 2);
    assert_eq!(mailbox.selected_location(), Some(&Folder::INBOX));

    // A label is a name mail carries, wherever the mail lives.
    let request = mailbox
        .select_folder(Folder::label("wN2", "Receipts"), 3)
        .unwrap();
    mailbox.finish_page(request.id, page(&["r1"], 1));
    load_detail(&mut mailbox, "r1", detail("r1", &["m1"]), 4);
    assert_eq!(mailbox.selected_location(), None);

    // A search looks through every folder at once.
    let mailbox = searched(&["a"], &["found"], "found");
    assert_eq!(mailbox.selected_location(), None);
}

#[test]
fn an_expired_offer_cannot_dismiss_the_move_after_it() {
    let mut mailbox = loaded_inbox(&["a", "b"], 2);
    mailbox.offer_undo(
        "a",
        SummaryKind::Conversation,
        MailAction::MoveTo(sys(MailFolder::Archive)),
    );
    let expired = mailbox.undo().unwrap().clone();
    mailbox.offer_undo(
        "b",
        SummaryKind::Conversation,
        MailAction::MoveTo(sys(MailFolder::Trash)),
    );

    mailbox.dismiss_undo(&expired);

    assert_eq!(mailbox.undo().map(|offer| offer.row_id.as_str()), Some("b"));
    let current = mailbox.undo().unwrap().clone();
    mailbox.dismiss_undo(&current);
    assert!(mailbox.undo().is_none());
}

#[test]
fn a_label_leaves_the_row_where_it_is() {
    let receipts = Folder::label("wN2", "Receipts");
    let mut mailbox = loaded_inbox(&["a", "b"], 2);
    load_detail(&mut mailbox, "a", detail("a", &["a1"]), 3);

    let request = mailbox
        .start_action(
            MailAction::SetLabel {
                label: receipts.clone(),
                on: true,
            },
            4,
        )
        .and_then(|start| start.send)
        .unwrap();
    assert_eq!(mailbox.finish_action(&request, Ok(())).result, Ok(()));

    // The row keeps its place in the Inbox, and the reader shows the
    // label without loading the conversation again.
    assert!(mailbox.has_row("a"));
    assert!(
        mailbox
            .reader()
            .expect("the conversation is still open")
            .detail()
            .carries(&receipts)
    );
    assert!(mailbox.undo().is_none());
}

#[test]
fn taking_a_label_away_inside_its_own_view_empties_the_row_out() {
    let receipts = Folder::label("wN2", "Receipts");
    let mut mailbox = loaded_inbox(&["a"], 1);
    let page_request = mailbox.select_folder(receipts.clone(), 2).unwrap();
    mailbox.finish_page(page_request.id, page(&["r1", "r2"], 2));
    load_detail(&mut mailbox, "r1", detail("r1", &["m1"]), 3);

    let start = mailbox
        .start_action(
            MailAction::SetLabel {
                label: receipts,
                on: false,
            },
            4,
        )
        .unwrap();

    // Nothing in this view carries the label any more, so reading moves
    // on to the row that took its place.
    assert_eq!(start.opened.as_deref(), Some("r2"));
    assert!(!mailbox.has_row("r1"));
}

#[test]
fn flags_and_labels_offer_nothing_to_undo() {
    // A flag change moves nothing.
    let mut mailbox = loaded_inbox(&["a"], 1);
    load_detail(&mut mailbox, "a", detail("a", &["a1"]), 2);
    let request = send(&mut mailbox, MailAction::SetStarred(true), 3);
    let _ = mailbox.finish_action(&request, Ok(()));
    assert!(mailbox.undo().is_none());

    // Starred is a label, so there is no folder to put the row back into.
    let page_request = mailbox.select_folder(sys(MailFolder::Starred), 4).unwrap();
    mailbox.finish_page(page_request.id, page(&["s1"], 1));
    load_detail(&mut mailbox, "s1", detail("s1", &["m1"]), 5);
    let request = send(
        &mut mailbox,
        MailAction::MoveTo(sys(MailFolder::Archive)),
        6,
    );
    let _ = mailbox.finish_action(&request, Ok(()));

    assert!(mailbox.undo().is_none());

    // A label view cannot identify the row's original folder for undo.
    let page_request = mailbox
        .select_folder(Folder::label("wN2", "Receipts"), 7)
        .unwrap();
    mailbox.finish_page(page_request.id, page(&["r1"], 1));
    load_detail(&mut mailbox, "r1", detail("r1", &["m1"]), 8);
    let request = send(
        &mut mailbox,
        MailAction::MoveTo(sys(MailFolder::Archive)),
        9,
    );
    let _ = mailbox.finish_action(&request, Ok(()));

    assert!(mailbox.undo().is_none());
}

#[test]
fn a_later_change_of_the_same_flag_decides_what_the_row_shows() {
    for result in [Ok(()), Err(MailboxError::Connection)] {
        let mut mailbox = selected_in(MailFolder::Inbox, &["a"], "a");
        let star = send(&mut mailbox, MailAction::SetStarred(true), 3);
        // Unstarred again while the star is on its way.
        let unstar = mailbox
            .start_action(MailAction::SetStarred(false), 4)
            .unwrap();
        assert_eq!(unstar.send, None, "{result:?}");

        // Whatever Proton says to the star, the row keeps the later choice.
        let next = mailbox.finish_action(&star, result).next_action.unwrap();
        assert_eq!(next.id, 4, "{result:?}");
        assert!(!mailbox.selected_summary().unwrap().starred, "{result:?}");
    }
}

#[test]
fn actions_wait_for_rows_still_to_come_but_not_for_a_refresh() {
    let (mut mailbox, first) = open();
    mailbox.finish_page(first.id, full_page(0, 120));
    load_detail(&mut mailbox, "p0", detail("p0", &["m"]), 3);

    // Load more asks Proton by position, which a move meanwhile would shift.
    let more = mailbox.load_more(4).unwrap();
    assert_eq!(mailbox.start_action(MailAction::SetStarred(true), 5), None);
    mailbox.finish_page(more.id, full_page(50, 120));
    let archive = send(
        &mut mailbox,
        MailAction::MoveTo(sys(MailFolder::Archive)),
        6,
    );
    let _ = mailbox.finish_action(&archive, Ok(()));
    load_detail(&mut mailbox, "p1", detail("p1", &["m"]), 7);
    // A search's answer replaces the rows on screen. The offer to undo
    // waits for it, since the move back could not start.
    mailbox.set_search_query("invoice".into());
    let _ = mailbox.start_search(8).unwrap();
    assert_eq!(mailbox.start_action(MailAction::SetStarred(true), 9), None);
    assert!(mailbox.take_undo().is_none());
    assert!(mailbox.undo().is_some());
    mailbox.set_search_query(String::new());

    // A refresh leaves the rows on screen, and they take actions meanwhile.
    let _ = mailbox.refresh(10).unwrap();
    assert!(
        mailbox
            .start_action(MailAction::SetStarred(true), 11)
            .is_some()
    );
    assert!(mailbox.action_pending());
}

#[test]
fn a_page_read_before_proton_applied_an_action_still_shows_it() {
    let mut mailbox = selected_in(MailFolder::Inbox, &["a", "b", "c"], "a");
    let archive = send(
        &mut mailbox,
        MailAction::MoveTo(sys(MailFolder::Archive)),
        3,
    );
    let refresh = mailbox.refresh(4).unwrap();
    let _ = mailbox.finish_action(&archive, Ok(()));
    let star = mailbox
        .start_action_on(
            "c".into(),
            SummaryKind::Conversation,
            MailAction::SetStarred(true),
            5,
        )
        .and_then(|start| start.send)
        .unwrap();

    // Proton read the page before it archived "a" or starred "c": the move
    // was confirmed meanwhile, and the star is still on its way.
    mailbox.finish_page(refresh.id, page(&["a", "b", "c"], 3));
    assert_eq!(ids(&mailbox), ["b", "c"]);
    assert!(mailbox.row("c").unwrap().starred);
    let _ = mailbox.finish_action(&star, Ok(()));
    assert_eq!(ids(&mailbox), ["b", "c"]);
    assert!(mailbox.row("c").unwrap().starred);
    // The app asks once more for what Proton has now.
    assert!(mailbox.needs_revalidation());
}

#[test]
fn search_results_read_before_proton_applied_an_action_still_show_it() {
    let mut mailbox = selected_in(MailFolder::Inbox, &["a", "b", "c"], "a");
    let archive = send(
        &mut mailbox,
        MailAction::MoveTo(sys(MailFolder::Archive)),
        3,
    );
    let star = mailbox
        .start_action_on(
            "c".into(),
            SummaryKind::Conversation,
            MailAction::SetStarred(true),
            4,
        )
        .expect("the star waits behind the move");
    assert!(star.send.is_none());
    mailbox.set_search_query("report".into());
    let search = mailbox.start_search(5).unwrap();
    let star = mailbox
        .finish_action(&archive, Ok(()))
        .next_action
        .expect("the star goes once the move is confirmed");

    // Proton read the results before it archived "a" or starred "c": the
    // move was confirmed meanwhile, and the star is still on its way.
    mailbox.finish_search(&search, page(&["a", "b", "c"], 3));
    let shown: Vec<_> = mailbox
        .visible_conversations()
        .map(|row| row.id.as_str())
        .collect();
    assert_eq!(shown, ["b", "c"]);
    assert!(mailbox.row("c").unwrap().starred);
    let _ = mailbox.finish_action(&star, Ok(()));
    assert!(mailbox.row("c").unwrap().starred);
}

#[test]
fn a_move_confirmed_while_an_emptied_folder_loads_again_stays_shown() {
    let mut mailbox = selected_in(MailFolder::Inbox, &["a"], "a");
    let archive = send(
        &mut mailbox,
        MailAction::MoveTo(sys(MailFolder::Archive)),
        3,
    );
    // Nothing is left to show, so the folder loads from scratch.
    let reload = mailbox.refresh(4).unwrap();
    assert!(matches!(mailbox.status(), ListStatus::Loading(_)));
    let _ = mailbox.finish_action(&archive, Ok(()));

    mailbox.finish_page(reload.id, page(&["a"], 1));
    assert!(ids(&mailbox).is_empty());
}

#[test]
fn a_move_taken_back_while_the_list_refreshes_keeps_its_row() {
    let (mut mailbox, first) = open();
    mailbox.finish_page(first.id, dated_page(&[("a", 30), ("b", 20)], 2));
    load_detail(&mut mailbox, "a", detail("a", &["m"]), 3);
    let archive = send(
        &mut mailbox,
        MailAction::MoveTo(sys(MailFolder::Archive)),
        4,
    );
    let _ = mailbox.finish_action(&archive, Ok(()));
    let refresh = mailbox.refresh(5).unwrap();

    let undo = mailbox
        .take_undo()
        .expect("a refresh does not hold Undo back");
    let back = mailbox.start_move_back(undo, 6).unwrap().send.unwrap();
    assert_eq!(ids(&mailbox), ["a", "b"]);

    // Proton read the page while "a" was still archived.
    mailbox.finish_page(refresh.id, dated_page(&[("b", 20)], 1));
    assert_eq!(ids(&mailbox), ["a", "b"]);
    // It read the next one after moving "a" back, before saying so.
    let refresh = mailbox.refresh(7).unwrap();
    mailbox.finish_page(refresh.id, dated_page(&[("a", 30), ("b", 20)], 2));
    assert_eq!(ids(&mailbox), ["a", "b"]);
    let _ = mailbox.finish_action(&back, Ok(()));
    assert_eq!(ids(&mailbox), ["a", "b"]);
    assert!(!mailbox.brought_back(&back));
}

#[test]
fn an_action_proton_refused_during_a_refresh_is_not_shown_again() {
    let mut mailbox = selected_in(MailFolder::Inbox, &["a", "b"], "a");
    let refresh = mailbox.refresh(3).unwrap();
    let star = send(&mut mailbox, MailAction::SetStarred(true), 4);
    let _ = mailbox.finish_action(&star, Err(MailboxError::Connection));
    let archive = send(
        &mut mailbox,
        MailAction::MoveTo(sys(MailFolder::Archive)),
        5,
    );
    let _ = mailbox.finish_action(&archive, Err(MailboxError::Connection));

    mailbox.finish_page(refresh.id, page(&["a", "b"], 2));
    assert_eq!(ids(&mailbox), ["a", "b"]);
    assert!(!mailbox.row("a").unwrap().starred);
}

#[test]
fn a_page_asked_for_after_an_action_was_confirmed_shows_what_proton_says() {
    let mut mailbox = selected_in(MailFolder::Inbox, &["a", "b"], "a");
    let failed = mailbox.refresh(3).unwrap();
    let star = send(&mut mailbox, MailAction::SetStarred(true), 4);
    let _ = mailbox.finish_action(&star, Ok(()));
    mailbox.finish_page(failed.id, Err(MailboxError::Connection));

    // Asked for once Proton had starred it, this page knows better: the star
    // was taken off elsewhere since.
    let refresh = mailbox.refresh(5).unwrap();
    mailbox.finish_page(refresh.id, page(&["a", "b"], 2));
    assert!(!mailbox.row("a").unwrap().starred);
}

#[test]
fn confirmed_flag_changes_update_the_row() {
    let mut mailbox = selected_in(MailFolder::Inbox, &["a", "b"], "a");

    let unread = send(&mut mailbox, MailAction::SetUnread(true), 3);
    assert_eq!(mailbox.finish_action(&unread, Ok(())).result, Ok(()));
    let star = send(&mut mailbox, MailAction::SetStarred(true), 4);
    assert_eq!(mailbox.finish_action(&star, Ok(())).result, Ok(()));

    let row = mailbox.selected_summary().unwrap();
    assert!(row.unread && row.starred);
    assert!(!mailbox.action_pending());
}

#[test]
fn a_move_takes_the_row_out_and_opens_the_next_before_proton_answers() {
    let mut mailbox = selected_in(MailFolder::Inbox, &["a", "b", "c"], "b");

    let start = mailbox
        .start_action(MailAction::MoveTo(sys(MailFolder::Archive)), 3)
        .unwrap();

    assert_eq!(start.opened.as_deref(), Some("c"));
    assert_eq!(ids(&mailbox), ["a", "c"]);
    assert_eq!(mailbox.reader_state(), &ReaderState::Empty);
    let finish = mailbox.finish_action(&start.send.unwrap(), Ok(()));
    assert_eq!(finish.result, Ok(()));
    assert_eq!(finish.opened, None);
    assert_eq!(ids(&mailbox), ["a", "c"]);
}

#[test]
fn moving_into_the_current_folder_keeps_the_row() {
    let mut mailbox = selected_in(MailFolder::Archive, &["a"], "a");

    let request = send(
        &mut mailbox,
        MailAction::MoveTo(sys(MailFolder::Archive)),
        3,
    );

    assert_eq!(mailbox.finish_action(&request, Ok(())).result, Ok(()));
    assert_eq!(ids(&mailbox), ["a"]);
}

#[test]
fn unstarring_in_starred_removes_the_row() {
    let mut mailbox = selected_in(MailFolder::Starred, &["a", "b"], "b");

    let start = mailbox
        .start_action(MailAction::SetStarred(false), 3)
        .unwrap();

    assert_eq!(start.opened.as_deref(), Some("a"));
    assert_eq!(ids(&mailbox), ["a"]);
}

#[test]
fn a_refused_move_reports_the_error_and_asks_for_the_list_again() {
    let mut mailbox = selected_in(MailFolder::Inbox, &["a", "b"], "a");
    let request = send(&mut mailbox, MailAction::MoveTo(sys(MailFolder::Trash)), 3);
    assert!(mailbox.undo().is_some());

    let finish = mailbox.finish_action(&request, Err(MailboxError::Service));

    assert_eq!(finish.result, Err(MailboxError::Service));
    assert_eq!(mailbox.action_error(), Some(MailboxError::Service));
    assert!(!mailbox.action_pending());
    // The move did not happen, so there is nothing to take back, and only
    // the list Proton sends can put the row back where it is.
    assert!(mailbox.undo().is_none());
    assert!(mailbox.took_out(&request));
    assert_eq!(ids(&mailbox), ["b"]);
}

#[test]
fn a_flag_shows_at_once_and_goes_back_if_proton_refuses_it() {
    let mut mailbox = selected_in(MailFolder::Inbox, &["a"], "a");

    let star = send(&mut mailbox, MailAction::SetStarred(true), 3);
    assert!(mailbox.selected_summary().unwrap().starred);
    let finish = mailbox.finish_action(&star, Err(MailboxError::Connection));
    assert_eq!(finish.result, Err(MailboxError::Connection));
    assert!(!mailbox.selected_summary().unwrap().starred);
    assert!(!mailbox.took_out(&star));

    // Asking for what the row already has changes nothing to put back.
    let unread = send(&mut mailbox, MailAction::SetUnread(false), 4);
    let _ = mailbox.finish_action(&unread, Err(MailboxError::Connection));
    assert!(!mailbox.selected_summary().unwrap().unread);
}

#[test]
fn an_action_asked_for_while_another_is_on_its_way_waits_its_turn() {
    let mut mailbox = selected_in(MailFolder::Inbox, &["a", "b", "c"], "a");
    let star = send(&mut mailbox, MailAction::SetStarred(true), 3);

    // The second press is neither lost nor sent alongside the first: it
    // shows at once and goes when the first has answered.
    let archive = mailbox
        .start_action(MailAction::MoveTo(sys(MailFolder::Archive)), 4)
        .unwrap();
    assert_eq!(archive.send, None);
    assert_eq!(archive.opened.as_deref(), Some("b"));
    assert_eq!(ids(&mailbox), ["b", "c"]);
    // The app opens the row the move left in its place.
    mailbox.start_conversation_load("b".into(), 6).unwrap();
    let trash = mailbox
        .start_action(MailAction::MoveTo(sys(MailFolder::Trash)), 5)
        .unwrap();
    assert_eq!(trash.opened.as_deref(), Some("c"));

    let next = mailbox.finish_action(&star, Ok(())).next_action.unwrap();
    assert_eq!((next.id, next.row_id.as_str()), (4, "a"));
    let last = mailbox.finish_action(&next, Ok(())).next_action.unwrap();
    assert_eq!((last.id, last.row_id.as_str()), (5, "b"));
    assert_eq!(mailbox.finish_action(&last, Ok(())).next_action, None);
    assert!(!mailbox.action_pending());
}

#[test]
fn taking_a_move_back_goes_after_the_move() {
    let mut mailbox = selected_in(MailFolder::Inbox, &["a", "b"], "a");
    let archive = send(
        &mut mailbox,
        MailAction::MoveTo(sys(MailFolder::Archive)),
        3,
    );

    let undo = mailbox
        .take_undo()
        .expect("a move can be taken back while it is on its way");
    let back = mailbox
        .start_action_on(undo.row_id, undo.kind, MailAction::MoveTo(undo.from), 4)
        .unwrap();
    assert_eq!(back.send, None, "the move back waits for the move");

    let next = mailbox.finish_action(&archive, Ok(())).next_action.unwrap();
    assert_eq!(next.id, 4);
    assert_eq!(next.action, MailAction::MoveTo(Folder::INBOX));
}

#[test]
fn a_reload_while_a_move_is_on_its_way_cannot_bring_the_row_back() {
    let mut mailbox = selected_in(MailFolder::Inbox, &["a", "b"], "b");
    let archive = send(
        &mut mailbox,
        MailAction::MoveTo(sys(MailFolder::Archive)),
        3,
    );
    assert_eq!(ids(&mailbox), ["a"]);

    // Proton had not moved it yet when the page was read.
    let refresh = mailbox.refresh(4).unwrap();
    mailbox.finish_page(refresh.id, page(&["a", "b"], 2));
    assert_eq!(ids(&mailbox), ["a"]);

    let _ = mailbox.finish_action(&archive, Ok(()));
    assert_eq!(ids(&mailbox), ["a"]);
}

#[test]
fn stale_or_moved_action_responses_change_nothing() {
    let mut mailbox = selected_in(MailFolder::Inbox, &["a", "b"], "a");
    let request = send(
        &mut mailbox,
        MailAction::MoveTo(sys(MailFolder::Archive)),
        3,
    );
    let stale = ActionRequest {
        id: NEVER_ISSUED,
        ..request.clone()
    };

    let finish = mailbox.finish_action(&stale, Ok(()));
    assert_eq!(
        (finish.result, finish.next_action, finish.opened),
        (Ok(()), None, None)
    );
    assert_eq!(ids(&mailbox), ["b"]);
    assert!(mailbox.action_pending());

    mailbox.select_folder(sys(MailFolder::Sent), 4);
    mailbox.finish_page(4, page(&["a"], 1));
    assert_eq!(mailbox.finish_action(&request, Ok(())).result, Ok(()));
    assert_eq!(ids(&mailbox), ["a"]);
}

#[test]
fn taking_back_a_move_lists_a_row_from_a_deeper_page_again() {
    // p60 is on the second page, which a reload does not read.
    let mut mailbox = archived_from_a_deeper_page("p60");

    let undo = mailbox.take_undo().expect("the move can be taken back");
    let back = mailbox
        .start_move_back(undo, 6)
        .and_then(|start| start.send)
        .unwrap();
    // Listed again at once, where its date puts it.
    let ids = visible_ids(&mailbox);
    let at = ids
        .iter()
        .position(|id| *id == "p60")
        .expect("listed again");
    assert_eq!((ids[at - 1], ids[at + 1]), ("p59", "p61"));

    let _ = mailbox.finish_action(&back, Ok(()));
    assert!(mailbox.has_row("p60"));
    assert!(
        !mailbox.brought_back(&back),
        "no reload is needed to show it"
    );

    // Confirmed, it is listed for good: a later refusal leaves it be.
    let star = mailbox
        .start_action_on(
            "p60".into(),
            SummaryKind::Conversation,
            MailAction::SetStarred(true),
            7,
        )
        .and_then(|start| start.send)
        .unwrap();
    let _ = mailbox.finish_action(&star, Err(MailboxError::Service));
    assert!(mailbox.has_row("p60"));
}

#[test]
fn a_refused_move_back_takes_its_row_away_again() {
    let mut mailbox = archived_from_a_deeper_page("p60");
    let undo = mailbox.take_undo().unwrap();
    let back = mailbox
        .start_move_back(undo, 6)
        .and_then(|start| start.send)
        .unwrap();
    assert!(mailbox.has_row("p60"));

    let _ = mailbox.finish_action(&back, Err(MailboxError::Service));
    assert!(!mailbox.has_row("p60"), "the row is still in Archive");
}

#[test]
fn a_move_back_behind_its_move_keeps_the_row_when_the_move_ends() {
    let mut mailbox = selected_in(MailFolder::Inbox, &["a", "b"], "a");
    let archive = send(
        &mut mailbox,
        MailAction::MoveTo(sys(MailFolder::Archive)),
        3,
    );
    let undo = mailbox.take_undo().unwrap();
    let back = mailbox.start_move_back(undo, 4).unwrap();
    assert_eq!(back.send, None, "the move back waits for the move");
    assert!(mailbox.has_row("a"));

    let next = mailbox.finish_action(&archive, Ok(())).next_action.unwrap();
    assert!(mailbox.has_row("a"), "the move's answer took it away again");
    // The move back, refused, leaves it in Archive after all.
    let _ = mailbox.finish_action(&next, Err(MailboxError::Service));
    assert!(!mailbox.has_row("a"));
}
