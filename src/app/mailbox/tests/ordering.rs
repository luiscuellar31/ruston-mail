//! The order of rows: newest first, across pages, and rows without a date.

use super::*;

#[test]
fn a_row_without_a_date_keeps_its_place_among_dated_mail() {
    let mut rows = vec![
        summary("undated"),
        dated("older", 100),
        dated("newest", 300),
    ];

    sort_newest_first(&mut rows);

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
