//! Choosing several rows with Command and Shift, and acting on all of them.

use super::*;

/// The Inbox with rows "a" to "e", "a" open.
fn inbox_with_a_open() -> Mailbox {
    let mut mailbox = loaded_inbox(&["a", "b", "c", "d", "e"], 5);
    load_detail(&mut mailbox, "a", detail("a", &["a1"]), 3);
    mailbox
}

fn selected(mailbox: &Mailbox) -> Vec<&str> {
    mailbox
        .selected_rows()
        .iter()
        .map(|row| row.id.as_str())
        .collect()
}

#[test]
fn a_command_click_chooses_rows_beside_the_open_one_and_closes_the_reader() {
    let mut mailbox = inbox_with_a_open();

    assert_eq!(mailbox.toggle_selected("c"), None);
    assert_eq!(selected(&mailbox), ["a", "c"]);
    assert_eq!(mailbox.selected_conversation(), None, "the reader closes");
    assert!(mailbox.is_chosen("a") && mailbox.is_chosen("c"));
    assert!(!mailbox.is_chosen("b"));

    assert_eq!(mailbox.toggle_selected("e"), None);
    assert_eq!(selected(&mailbox), ["a", "c", "e"]);
    // Taken out again down to one row, that one is for the app to open.
    assert_eq!(mailbox.toggle_selected("a"), None);
    assert_eq!(mailbox.toggle_selected("e"), Some("c".to_owned()));
    assert!(selected(&mailbox).is_empty());
}

#[test]
fn a_command_click_on_the_open_row_alone_closes_it() {
    let mut mailbox = inbox_with_a_open();

    assert_eq!(mailbox.toggle_selected("a"), None);
    assert_eq!(mailbox.selected_conversation(), None);
    assert!(!mailbox.is_chosen("a"));
}

#[test]
fn a_shift_click_chooses_every_row_from_the_last_one_clicked() {
    let mut mailbox = inbox_with_a_open();

    assert_eq!(mailbox.select_through("c"), None);
    assert_eq!(selected(&mailbox), ["a", "b", "c"]);
    // The range keeps its start, so the next one replaces it.
    assert_eq!(mailbox.select_through("b"), None);
    assert_eq!(selected(&mailbox), ["a", "b"]);
    // A Command click moves the start to its row.
    assert_eq!(mailbox.toggle_selected("e"), None);
    assert_eq!(mailbox.select_through("d"), None);
    assert_eq!(selected(&mailbox), ["d", "e"]);
    // Back to the row it starts from, that one row opens.
    assert_eq!(mailbox.select_through("e"), Some("e".to_owned()));
    assert!(selected(&mailbox).is_empty());
}

#[test]
fn a_shift_click_with_nothing_open_opens_that_row() {
    let mut mailbox = loaded_inbox(&["a", "b"], 2);

    assert_eq!(mailbox.select_through("b"), Some("b".to_owned()));
    assert!(selected(&mailbox).is_empty());
}

#[test]
fn shift_and_an_arrow_move_the_far_end_of_the_choice() {
    let mut mailbox = loaded_inbox(&["a", "b", "c", "d"], 4);
    load_detail(&mut mailbox, "b", detail("b", &["b1"]), 3);

    assert_eq!(mailbox.extend_selection(Step::Next), None);
    assert_eq!(selected(&mailbox), ["b", "c"]);
    assert_eq!(mailbox.extend_selection(Step::Next), None);
    assert_eq!(selected(&mailbox), ["b", "c", "d"]);
    // The last row stops it.
    assert_eq!(mailbox.extend_selection(Step::Next), None);
    assert_eq!(selected(&mailbox), ["b", "c", "d"]);
    assert_eq!(mailbox.extend_selection(Step::Previous), None);
    assert_eq!(
        mailbox.extend_selection(Step::Previous),
        Some("b".to_owned())
    );

    // Opened again, from the reader's cache, the next range starts from it,
    // the other way too.
    assert_eq!(mailbox.start_conversation_load("b".into(), 4), None);
    assert_eq!(mailbox.selected_conversation(), Some("b"));
    assert_eq!(mailbox.extend_selection(Step::Previous), None);
    assert_eq!(selected(&mailbox), ["a", "b"]);
}

#[test]
fn shift_and_an_arrow_with_nothing_open_open_the_first_row() {
    let mut mailbox = loaded_inbox(&["a", "b"], 2);

    assert_eq!(mailbox.extend_selection(Step::Next), Some("a".to_owned()));
}

#[test]
fn a_plain_arrow_after_a_choice_steps_from_its_far_end() {
    let mut mailbox = inbox_with_a_open();
    mailbox.select_through("c");

    assert_eq!(mailbox.neighbour(Step::Next), Some("d".to_owned()));
    assert_eq!(mailbox.neighbour(Step::Previous), Some("b".to_owned()));
}

#[test]
fn a_refresh_keeps_the_chosen_rows_it_still_lists() {
    let mut mailbox = inbox_with_a_open();
    mailbox.select_through("c");

    let refresh = mailbox.refresh(4).unwrap();
    mailbox.finish_page(refresh.id, page(&["a", "c", "d"], 3));
    assert_eq!(selected(&mailbox), ["a", "c"]);

    // One row left is no choice of several, and it does not open by itself.
    let refresh = mailbox.refresh(5).unwrap();
    mailbox.finish_page(refresh.id, page(&["c", "d"], 2));
    assert!(selected(&mailbox).is_empty());
    assert!(!mailbox.is_chosen("c"));
}

#[test]
fn opening_a_row_ends_the_choice_and_starts_the_next_range_there() {
    let mut mailbox = inbox_with_a_open();
    mailbox.select_through("c");

    load_detail(&mut mailbox, "d", detail("d", &["d1"]), 4);
    assert!(selected(&mailbox).is_empty());
    assert!(mailbox.is_chosen("d") && !mailbox.is_chosen("a"));

    mailbox.select_through("e");
    assert_eq!(selected(&mailbox), ["d", "e"]);
}

#[test]
fn clearing_the_choice_leaves_nothing_open() {
    let mut mailbox = inbox_with_a_open();
    assert!(
        !mailbox.clear_selection(),
        "the open row alone is no choice"
    );
    assert_eq!(mailbox.selected_conversation(), Some("a"));

    mailbox.select_through("b");
    assert!(mailbox.clear_selection());
    assert!(selected(&mailbox).is_empty());
    assert!(!mailbox.is_chosen("a"));
}

#[test]
fn an_action_acts_on_the_chosen_rows_in_list_order_or_on_the_open_one() {
    let mut mailbox = inbox_with_a_open();
    assert_eq!(mailbox.rows_acted_on(), ["a"]);

    mailbox.toggle_selected("d");
    mailbox.toggle_selected("b");
    assert_eq!(mailbox.rows_acted_on(), ["a", "b", "d"]);
}

/// Settles every action of the request on its way, in order, as the app
/// does when Proton answers it, and returns the request sent next.
fn answer(
    mailbox: &mut Mailbox,
    head: &ActionRequest,
    result: Result<(), MailboxError>,
) -> Option<ActionRequest> {
    let mut next = None;
    for request in mailbox.request_group(head) {
        next = mailbox.finish_action(&request, result).next_action.or(next);
    }
    next
}

#[test]
fn a_move_of_several_rows_shows_at_once_and_goes_in_one_request() {
    let mut mailbox = inbox_with_a_open();
    mailbox.select_through("c");
    let rows = mailbox.rows_acted_on();

    let starts = mailbox.start_actions_on(
        &rows,
        MailAction::MoveTo(sys(MailFolder::Archive)),
        &[10, 11, 12],
    );

    assert_eq!(starts.len(), 3);
    let first = starts[0].send.clone().expect("the request goes now");
    let group: Vec<_> = mailbox
        .request_group(&first)
        .into_iter()
        .map(|request| request.row_id)
        .collect();
    assert_eq!(group, ["a", "b", "c"], "one request acts on all three");
    assert!(
        starts[1..]
            .iter()
            .all(|start| start.send.is_none() && start.opened.is_none()),
        "nothing else is sent, and nothing opens"
    );
    assert_eq!(ids(&mailbox), ["d", "e"]);
    assert!(selected(&mailbox).is_empty());
    let undo = mailbox.undo().expect("one offer for the three");
    assert_eq!(undo.to, sys(MailFolder::Archive));
    let moved: Vec<_> = undo
        .rows
        .iter()
        .map(|row| (row.row_id.as_str(), &row.from))
        .collect();
    assert_eq!(
        moved,
        [
            ("a", &Folder::INBOX),
            ("b", &Folder::INBOX),
            ("c", &Folder::INBOX)
        ]
    );

    // One answer settles all three, and nothing is left to send.
    assert_eq!(answer(&mut mailbox, &first, Ok(())), None);
    assert_eq!(mailbox.unsent_changes(), 0);
    assert_eq!(mailbox.undo().map(|offer| offer.rows.len()), Some(3));
}

#[test]
fn a_refused_request_for_several_rows_takes_all_of_them_back() {
    let mut mailbox = inbox_with_a_open();
    mailbox.select_through("b");
    let rows = mailbox.rows_acted_on();
    let starts = mailbox.start_actions_on(
        &rows,
        MailAction::MoveTo(sys(MailFolder::Archive)),
        &[10, 11],
    );
    let first = starts[0].send.clone().unwrap();

    assert_eq!(
        answer(&mut mailbox, &first, Err(MailboxError::Connection)),
        None
    );

    assert!(mailbox.undo().is_none(), "nothing moved to take back");
    assert_eq!(mailbox.action_error(), Some(MailboxError::Connection));
}

#[test]
fn a_page_of_rows_goes_in_each_request() {
    let ids: Vec<String> = (0..60).map(|n| format!("r{n}")).collect();
    let names: Vec<&str> = ids.iter().map(String::as_str).collect();
    let mut mailbox = loaded_inbox(&names, 60);
    let requests: Vec<RequestId> = (100..160).collect();

    let starts = mailbox.start_actions_on(&ids, MailAction::SetStarred(true), &requests);

    let first = starts[0].send.clone().unwrap();
    assert_eq!(mailbox.request_group(&first).len(), 50);
    let second = answer(&mut mailbox, &first, Ok(())).expect("the rest go next");
    let rest: Vec<_> = mailbox
        .request_group(&second)
        .into_iter()
        .map(|request| request.row_id)
        .collect();
    assert_eq!(rest, ids[50..]);
    assert_eq!(answer(&mut mailbox, &second, Ok(())), None);
}

#[test]
fn rows_chosen_behind_an_action_on_its_way_go_together_after_it() {
    let mut mailbox = inbox_with_a_open();
    let star = send(&mut mailbox, MailAction::SetStarred(true), 10);
    mailbox.select_through("c");
    let rows = mailbox.rows_acted_on();

    let starts = mailbox.start_actions_on(&rows, MailAction::SetUnread(true), &[11, 12, 13]);

    assert!(starts.iter().all(|start| start.send.is_none()), "they wait");
    let next = answer(&mut mailbox, &star, Ok(())).expect("then they go");
    let group: Vec<_> = mailbox
        .request_group(&next)
        .into_iter()
        .map(|request| request.row_id)
        .collect();
    assert_eq!(group, ["a", "b", "c"]);
}

#[test]
fn actions_asked_for_one_by_one_still_go_one_by_one() {
    let mut mailbox = inbox_with_a_open();
    let archive = MailAction::MoveTo(sys(MailFolder::Archive));
    let first = mailbox
        .start_action_on("b".into(), SummaryKind::Conversation, archive.clone(), 10)
        .and_then(|start| start.send)
        .unwrap();
    let _ = mailbox.start_action_on("c".into(), SummaryKind::Conversation, archive, 11);

    assert_eq!(mailbox.request_group(&first).len(), 1);
    let second = answer(&mut mailbox, &first, Ok(())).expect("c goes after b");
    assert_eq!(second.row_id, "c");
}

#[test]
fn choosing_all_chooses_every_row_listed() {
    let mut mailbox = inbox_with_a_open();

    assert_eq!(mailbox.select_all(), None);
    assert_eq!(selected(&mailbox), ["a", "b", "c", "d", "e"]);
    // Shift and an arrow go on from the last row.
    assert_eq!(mailbox.extend_selection(Step::Previous), None);
    assert_eq!(selected(&mailbox), ["a", "b", "c", "d"]);

    // What a search narrows the list to is all there is to choose.
    mailbox.set_search_query("no row matches this".into());
    assert_eq!(mailbox.select_all(), None);
    assert!(selected(&mailbox).is_empty());
}

#[test]
fn choosing_all_of_a_single_row_opens_it() {
    let mut mailbox = loaded_inbox(&["a"], 1);

    assert_eq!(mailbox.select_all(), Some("a".to_owned()));
}

#[test]
fn taking_back_a_move_of_several_rows_puts_each_back() {
    let mut mailbox = inbox_with_a_open();
    mailbox.select_through("c");
    let rows = mailbox.rows_acted_on();
    let starts = mailbox.start_actions_on(
        &rows,
        MailAction::MoveTo(sys(MailFolder::Archive)),
        &[10, 11, 12],
    );
    let mut request = starts[0].send.clone();
    while let Some(sent) = request {
        request = answer(&mut mailbox, &sent, Ok(()));
    }

    let undo = mailbox.take_undo().expect("the offer");
    let mut backs = undo
        .rows
        .into_iter()
        .zip(20..)
        .map(|(row, request)| mailbox.start_move_back(row, request).unwrap());
    let first = backs.next().unwrap().send.expect("the first goes now");
    assert!(backs.all(|start| start.send.is_none()));
    let _ = answer(&mut mailbox, &first, Ok(()));

    assert_eq!(first.action, MailAction::MoveTo(Folder::INBOX));
    for row in ["a", "b", "c"] {
        assert!(mailbox.has_row(row), "{row} is listed again");
    }
}

#[test]
fn a_flag_on_several_rows_keeps_them_chosen() {
    let mut mailbox = inbox_with_a_open();
    mailbox.select_through("b");
    let rows = mailbox.rows_acted_on();

    let starts = mailbox.start_actions_on(&rows, MailAction::SetStarred(true), &[10, 11]);

    assert_eq!(starts.len(), 2);
    assert!(mailbox.undo().is_none(), "a flag has nothing to take back");
    assert_eq!(selected(&mailbox), ["a", "b"]);
    assert!(mailbox.selected_rows().iter().all(|row| row.starred));
}
