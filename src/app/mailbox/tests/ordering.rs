//! The order of rows: newest first, across pages, and rows without a date.

use super::*;

#[test]
fn a_row_without_a_date_keeps_its_place_among_dated_mail() {
    let mut rows = vec![
        summary("undated"),
        dated("older", 100),
        dated("newest", 300),
    ];

    sort_by_date(&mut rows, ListOrder::NewestFirst);

    // It borrows the time of the row it sits next to instead of floating
    // to the top of the mailbox.
    assert_eq!(
        rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
        ["newest", "undated", "older"]
    );
}

#[test]
fn later_pages_merge_interleaved_dates_and_keep_earlier_rows_first_on_ties() {
    let (mut mailbox, first) = open_with_page_size(3);
    mailbox.finish_page(first.id, dated_page(&[("a", 100), ("b", 80), ("c", 50)], 7));

    let next = mailbox.load_more(3).unwrap();
    mailbox.finish_page(
        next.id,
        dated_page(&[("d", 90), ("e", 80), ("c", 50), ("f", 40)], 7),
    );

    assert_eq!(ids(&mailbox), ["a", "d", "b", "e", "c", "f"]);
    assert_eq!(visible_ids(&mailbox), ids(&mailbox));
}

#[test]
fn interleaved_pages_keep_undated_rows_with_their_neighbours() {
    let (mut mailbox, first) = open_with_page_size(3);
    mailbox.finish_page(
        first.id,
        page_of(
            vec![dated("a", 100), summary("undated-a"), dated("b", 50)],
            6,
        ),
    );

    let next = mailbox.load_more(3).unwrap();
    mailbox.finish_page(
        next.id,
        page_of(
            vec![dated("c", 75), summary("undated-c"), dated("d", 40)],
            6,
        ),
    );

    assert_eq!(
        ids(&mailbox),
        ["a", "undated-a", "c", "undated-c", "b", "d"]
    );
}

#[test]
fn older_pages_preserve_undated_rows_and_extend_local_filter() {
    let (mut mailbox, first) = open_with_page_size(3);
    let mut match_a = dated("a", 100);
    match_a.subject = Some("receipt".into());
    let mut match_b = dated("b", 70);
    match_b.subject = Some("receipt".into());
    mailbox.finish_page(
        first.id,
        page_of(vec![match_a, summary("undated-a"), dated("other", 80)], 6),
    );
    mailbox.set_search_query("receipt".into());

    let next = mailbox.load_more(3).unwrap();
    mailbox.finish_page(
        next.id,
        page_of(vec![summary("undated-b"), match_b, dated("last", 60)], 6),
    );

    assert_eq!(
        ids(&mailbox),
        ["a", "undated-a", "other", "undated-b", "b", "last"]
    );
    assert_eq!(visible_ids(&mailbox), ["a", "b"]);
}

#[test]
fn rows_without_a_time_keep_their_place() {
    let (mut mailbox, request) = open();

    mailbox.finish_page(
        request.id,
        page_of(
            vec![
                dated("c1", 50),
                summary("undated"),
                dated("m1", 10),
                dated("c2", 30),
            ],
            4,
        ),
    );

    assert_eq!(ids(&mailbox), ["c1", "undated", "c2", "m1"]);
}

#[test]
fn rows_stay_newest_first_across_pages() {
    let (mut mailbox, request) = open();
    mailbox.finish_page(
        request.id,
        page_of(
            vec![
                dated("c1", 50),
                dated("m1", 40),
                dated("m2", 10),
                dated("c2", 30),
            ],
            120,
        ),
    );
    assert_eq!(ids(&mailbox), ["c1", "m1", "c2", "m2"]);

    let more = mailbox.load_more(3).unwrap();
    mailbox.finish_page(more.id, page_of(vec![dated("c3", 20)], 120));

    assert_eq!(ids(&mailbox), ["c1", "m1", "c2", "c3", "m2"]);
}

/// An Inbox asked for oldest first, three rows a page, and the request for
/// its first page.
fn oldest_first() -> (Mailbox, PageRequest) {
    let (mut mailbox, _) = open_with_page_size(3);
    let request = mailbox
        .sort_by(ListOrder::OldestFirst, 50)
        .expect("a first page, oldest first");
    (mailbox, request)
}

#[test]
fn oldest_first_sorts_the_other_way_and_keeps_an_undated_row_beside_its_neighbour() {
    let mut rows = vec![
        summary("undated"),
        dated("newest", 300),
        dated("older", 100),
    ];

    sort_by_date(&mut rows, ListOrder::OldestFirst);

    // As newest first, it borrows the time of the row it sits next to.
    assert_eq!(
        rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
        ["older", "undated", "newest"]
    );
}

#[test]
fn oldest_first_later_pages_merge_interleaved_dates_with_earlier_rows_first_on_ties() {
    let (mut mailbox, first) = oldest_first();
    mailbox.finish_page(first.id, dated_page(&[("a", 50), ("b", 80), ("c", 100)], 7));

    let next = mailbox.load_more(51).unwrap();
    assert_eq!(next.order, ListOrder::OldestFirst);
    mailbox.finish_page(
        next.id,
        dated_page(&[("d", 60), ("e", 80), ("c", 100), ("f", 110)], 7),
    );

    assert_eq!(ids(&mailbox), ["a", "d", "b", "e", "c", "f"]);
    assert_eq!(visible_ids(&mailbox), ids(&mailbox));
}

#[test]
fn oldest_first_a_refresh_keeps_the_newer_pages_already_loaded() {
    let (mut mailbox, first) = oldest_first();
    mailbox.finish_page(first.id, dated_page(&[("a", 10), ("b", 20), ("c", 30)], 6));
    let next = mailbox.load_more(51).unwrap();
    mailbox.finish_page(next.id, dated_page(&[("d", 40), ("e", 50), ("f", 60)], 6));

    let refresh = mailbox.refresh(52).unwrap();
    assert_eq!(refresh.order, ListOrder::OldestFirst);
    mailbox.finish_page(
        refresh.id,
        dated_page(&[("a", 10), ("b", 20), ("c", 30)], 6),
    );

    // The second page lies after the first here, where its dates are later.
    assert_eq!(ids(&mailbox), ["a", "b", "c", "d", "e", "f"]);
}

#[test]
fn oldest_first_a_move_taken_back_lists_its_row_where_its_date_puts_it() {
    let (mut mailbox, first) = oldest_first();
    mailbox.finish_page(first.id, dated_page(&[("a", 10), ("b", 20), ("c", 30)], 3));
    load_detail(&mut mailbox, "b", detail("b", &["b1"]), 51);
    let archive = mailbox
        .start_action(MailAction::MoveTo(sys(MailFolder::Archive)), 52)
        .and_then(|start| start.send)
        .unwrap();
    let _ = mailbox.finish_action(&archive, Ok(()));
    assert_eq!(ids(&mailbox), ["a", "c"]);

    let undo = mailbox.take_undo().expect("the move can be taken back");
    let back = mailbox.start_move_back(undo.rows.into_iter().next().unwrap(), 53);
    assert!(back.is_some());

    assert_eq!(ids(&mailbox), ["a", "b", "c"]);
}
