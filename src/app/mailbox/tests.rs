//! What an open mailbox does as pages, searches, actions and the reader
//! move through it.

use super::*;
use crate::mail::{MailAddress, MailMessage, MessageBody};
use crate::settings::Reading;

const PAGE_SIZE: u32 = 50;

/// One of Proton's own folders, as a place to list from.
fn sys(folder: MailFolder) -> Folder {
    Folder::System(folder)
}

fn summary(id: &str) -> ConversationSummary {
    ConversationSummary {
        id: id.to_owned(),
        kind: SummaryKind::Conversation,
        subject: None,
        correspondents: None,
        participants: Vec::new(),
        preview: None,
        time: None,
        unread: false,
        starred: false,
        message_count: 1,
        has_attachments: false,
    }
}

fn searchable_summary(
    id: &str,
    name: &str,
    address: &str,
    subject: &str,
    preview: &str,
) -> ConversationSummary {
    ConversationSummary {
        id: id.to_owned(),
        kind: SummaryKind::Conversation,
        subject: Some(subject.to_owned()),
        correspondents: Some(name.to_owned()),
        participants: vec![MailAddress {
            name: Some(name.to_owned()),
            address: address.to_owned(),
        }],
        preview: Some(preview.to_owned()),
        time: None,
        unread: false,
        starred: false,
        message_count: 1,
        has_attachments: false,
    }
}

fn page(ids: &[&str], total: u32) -> Result<ConversationPage, MailboxError> {
    Ok(ConversationPage {
        conversations: ids.iter().map(|id| summary(id)).collect(),
        total,
    })
}

fn detail(id: &str, message_ids: &[&str]) -> ConversationDetail {
    ConversationDetail {
        id: id.to_owned(),
        subject: None,
        labels: Vec::new(),
        messages: message_ids
            .iter()
            .enumerate()
            .map(|(time, message_id)| MailMessage {
                id: (*message_id).to_owned(),
                sender: MailAddress::default(),
                recipients: Vec::new(),
                time: Some(time as i64),
                body: MessageBody::PlainText(String::new()),
                attachments: Vec::new(),
            })
            .collect(),
    }
}

fn ids(mailbox: &Mailbox) -> Vec<&str> {
    mailbox
        .conversations()
        .iter()
        .map(|c| c.id.as_str())
        .collect()
}

fn open() -> (Mailbox, PageRequest) {
    Mailbox::open(Folder::INBOX, PAGE_SIZE, 1, 2)
}

fn loaded_inbox(ids: &[&str], total: u32) -> Mailbox {
    let (mut mailbox, request) = open();
    mailbox.finish_page(request.id, page(ids, total));
    mailbox
}

fn load_detail(
    mailbox: &mut Mailbox,
    conversation_id: &str,
    conversation: ConversationDetail,
    request: RequestId,
) {
    let request = mailbox
        .start_conversation_load(conversation_id.into(), request)
        .unwrap();
    assert_eq!(
        mailbox.finish_conversation(&request, Ok(conversation)),
        None
    );
}

fn searchable_mailbox() -> Mailbox {
    let (mut mailbox, request) = open();
    let conversations = vec![
        searchable_summary(
            "sender",
            "Alice Stone",
            "alice@example.com",
            "Quarterly roadmap",
            "Budget approval is ready",
        ),
        searchable_summary(
            "subject",
            "Bob Chen",
            "bob@example.com",
            "Rust project plan",
            "Milestones for next month",
        ),
        searchable_summary(
            "preview",
            "Carol Diaz",
            "carol@example.com",
            "Release notes",
            "Launch checklist is complete",
        ),
    ];
    mailbox.finish_page(
        request.id,
        Ok(ConversationPage {
            total: conversations.len() as u32,
            conversations,
        }),
    );
    mailbox
}

fn visible_ids(mailbox: &Mailbox) -> Vec<&str> {
    mailbox
        .visible_conversations()
        .map(|conversation| conversation.id.as_str())
        .collect()
}

fn dated(id: &str, time: i64) -> ConversationSummary {
    ConversationSummary {
        time: Some(time),
        ..summary(id)
    }
}

fn dated_page(rows: &[(&str, i64)], total: u32) -> Result<ConversationPage, MailboxError> {
    Ok(ConversationPage {
        conversations: rows.iter().map(|(id, time)| dated(id, *time)).collect(),
        total,
    })
}

/// A full page of dated rows named `p<n>`, newest first.
fn full_page(start: u32, total: u32) -> Result<ConversationPage, MailboxError> {
    let conversations = (0..PAGE_SIZE)
        .map(|index| {
            let number = start + index;
            dated(&format!("p{number}"), i64::from(1_000_000 - number))
        })
        .collect();

    Ok(ConversationPage {
        conversations,
        total,
    })
}

#[test]
fn server_results_replace_the_folder_listing() {
    let mut mailbox = loaded_inbox(&["a", "b"], 120);
    mailbox.set_search_query("invoice".into());

    let request = mailbox.start_search(3).expect("a query reaches the server");
    assert_eq!(request.query, "invoice");

    // Results stand on their own: what came back is not narrowed again.
    assert_eq!(mailbox.finish_search(&request, page(&["found"], 1)), None);
    assert_eq!(visible_ids(&mailbox), ["found"]);
    assert_eq!(mailbox.search_results(), Some("invoice"));

    // The server answers in one batch, so there is no next page to ask
    // for, even though the folder behind the results has more.
    assert_eq!(mailbox.load_more(4), None);

    // Emptying the field puts the folder back.
    mailbox.set_search_query(String::new());
    assert_eq!(visible_ids(&mailbox), ["a", "b"]);
    assert_eq!(mailbox.search_results(), None);
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
    assert!(mailbox.search_results().is_none());
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
fn refreshing_drops_the_offer_to_undo() {
    let mut mailbox = loaded_inbox(&["a", "b"], 2);
    load_detail(&mut mailbox, "a", detail("a", &["a1"]), 3);
    let request = mailbox
        .start_action(MailAction::MoveTo(sys(MailFolder::Archive)), 4)
        .unwrap();
    let _ = mailbox.finish_action(&request, Ok(()));
    assert!(mailbox.undo().is_some());

    // The reloaded list no longer shows what the offer talked about.
    let _ = mailbox.refresh(5);

    assert!(mailbox.undo().is_none());
}

#[test]
fn a_move_out_of_a_folder_can_be_taken_back() {
    let mut mailbox = loaded_inbox(&["a", "b"], 2);
    load_detail(&mut mailbox, "a", detail("a", &["a1"]), 3);
    let request = mailbox
        .start_action(MailAction::MoveTo(sys(MailFolder::Archive)), 4)
        .unwrap();
    assert!(mailbox.undo().is_none());

    assert_eq!(
        mailbox.finish_action(&request, Ok(())),
        Ok(Some("b".into()))
    );

    let undo = mailbox.undo().expect("a move out of the Inbox undoes");
    assert_eq!(undo.row_id, "a");
    assert_eq!(undo.from, sys(MailFolder::Inbox));
    assert_eq!(undo.to, sys(MailFolder::Archive));

    // The offer is claimed once, and the next action replaces it.
    assert!(mailbox.take_undo().is_some());
    assert!(mailbox.undo().is_none());
}

/// An Inbox holding `listed`, showing the results of a server search that
/// returned `found`, with `opened` open in the reader.
fn searched(listed: &[&str], found: &[&str], opened: &str) -> Mailbox {
    let (mut mailbox, request) = open();
    let rows: Vec<ConversationSummary> = listed
        .iter()
        .map(|id| ConversationSummary {
            unread: true,
            ..summary(id)
        })
        .collect();
    let total = rows.len() as u32;
    mailbox.finish_page(
        request.id,
        Ok(ConversationPage {
            conversations: rows,
            total,
        }),
    );

    mailbox.set_search_query("report".to_owned());
    let search = mailbox.start_search(90).unwrap();
    let results: Vec<ConversationSummary> = found
        .iter()
        .map(|id| ConversationSummary {
            unread: true,
            ..summary(id)
        })
        .collect();
    let total = results.len() as u32;
    assert_eq!(
        mailbox.finish_search(
            &search,
            Ok(ConversationPage {
                conversations: results,
                total,
            }),
        ),
        None
    );
    load_detail(&mut mailbox, opened, detail(opened, &["message"]), 91);

    mailbox
}

#[test]
fn a_conversation_a_search_found_can_be_acted_on() {
    let mut mailbox = searched(&["a"], &["found"], "found");

    // The row is nowhere in the folder listing, so Proton is told to read
    // it across every folder rather than in the open one.
    let request = mailbox
        .start_action(MailAction::SetStarred(true), 3)
        .expect("a search result takes actions like any other row");
    assert_eq!(request.row_id, "found");
    assert_eq!(request.context, None);

    assert_eq!(mailbox.finish_action(&request, Ok(())), Ok(None));
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

    assert_eq!(mailbox.finish_mark_read(&request, Ok(())), Ok(true));
    assert!(!mailbox.selected_summary().unwrap().unread);
}

#[test]
fn a_row_in_both_listings_changes_in_both() {
    // The search found a row the open folder also lists.
    let mut mailbox = searched(&["a"], &["a"], "a");

    // It is listed in the folder, so the open folder is the context.
    let request = mailbox
        .start_action(MailAction::SetStarred(true), 3)
        .unwrap();
    assert_eq!(request.context.as_ref(), Some(&sys(MailFolder::Inbox)));
    assert_eq!(mailbox.finish_action(&request, Ok(())), Ok(None));

    // Both copies agree, so the folder listing holds no stale row waiting
    // for the search to be left behind.
    assert!(mailbox.selected_summary().unwrap().starred);
    assert!(mailbox.conversations().iter().all(|row| row.starred));
}

#[test]
fn a_row_moved_out_leaves_the_search_results_too() {
    let mut mailbox = searched(&["a"], &["a", "b"], "a");

    let request = mailbox
        .start_action(MailAction::MoveTo(sys(MailFolder::Archive)), 3)
        .unwrap();

    // Reading goes on with the next result, and the moved row is gone
    // from the results and from the folder listing alike.
    assert_eq!(
        mailbox.finish_action(&request, Ok(())),
        Ok(Some("b".into()))
    );
    assert!(!mailbox.has_row("a"));
    assert!(mailbox.conversations().is_empty());
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
    assert!(contains_ignoring_case("ÉCOLE", "école"));
    assert!(!contains_ignoring_case("Ángel", "ünsal"));
    // And an ASCII line cannot hold a word that is not ASCII.
    assert!(!contains_ignoring_case("Angel", "ángel"));
}

#[test]
fn taking_up_a_new_name_keeps_the_list_and_the_reader() {
    let mut mailbox = loaded_inbox(&["a"], 1);
    let request = mailbox
        .select_folder(Folder::custom("kZ9", "Invoices"), 2)
        .unwrap();
    mailbox.finish_page(request.id, page(&["i1", "i2"], 2));
    load_detail(&mut mailbox, "i1", detail("i1", &["m1"]), 3);

    mailbox.rename_folder(Folder::custom("kZ9", "Facturas"));

    // Proton knows the place by its id, so the same mail is listed under
    // the new name and nothing had to be fetched again.
    assert_eq!(mailbox.folder().name(), "Facturas");
    assert_eq!(mailbox.conversations().len(), 2);
    assert_eq!(mailbox.selected_conversation(), Some("i1"));

    // Another place is not this one under a new name.
    mailbox.rename_folder(Folder::custom("wN2", "Recibos"));
    assert_eq!(mailbox.folder().name(), "Facturas");
    // Nor is one of Proton's own, which carries no id of its own.
    mailbox.rename_folder(sys(MailFolder::Archive));
    assert_eq!(mailbox.folder().name(), "Facturas");
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

    let request = mailbox
        .start_action(MailAction::MoveTo(sys(MailFolder::Archive)), 3)
        .unwrap();
    assert_eq!(
        mailbox.finish_action(&request, Ok(())),
        Ok(Some("next".into()))
    );

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
        .unwrap();
    assert_eq!(mailbox.finish_action(&request, Ok(())), Ok(None));

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

    let request = mailbox
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
    assert_eq!(
        mailbox.finish_action(&request, Ok(())),
        Ok(Some("r2".into()))
    );
    assert!(!mailbox.has_row("r1"));
}

#[test]
fn flags_and_labels_offer_nothing_to_undo() {
    // A flag change moves nothing.
    let mut mailbox = loaded_inbox(&["a"], 1);
    load_detail(&mut mailbox, "a", detail("a", &["a1"]), 2);
    let request = mailbox
        .start_action(MailAction::SetStarred(true), 3)
        .unwrap();
    let _ = mailbox.finish_action(&request, Ok(()));
    assert!(mailbox.undo().is_none());

    // Starred is a label, so there is no folder to put the row back into.
    let page_request = mailbox.select_folder(sys(MailFolder::Starred), 4).unwrap();
    mailbox.finish_page(page_request.id, page(&["s1"], 1));
    load_detail(&mut mailbox, "s1", detail("s1", &["m1"]), 5);
    let request = mailbox
        .start_action(MailAction::MoveTo(sys(MailFolder::Archive)), 6)
        .unwrap();
    let _ = mailbox.finish_action(&request, Ok(()));

    assert!(mailbox.undo().is_none());

    // A label the account made behaves the same: the row keeps the label
    // and moves out of whatever folder it was really in, which this view
    // never knew.
    let page_request = mailbox
        .select_folder(Folder::label("wN2", "Receipts"), 7)
        .unwrap();
    mailbox.finish_page(page_request.id, page(&["r1"], 1));
    load_detail(&mut mailbox, "r1", detail("r1", &["m1"]), 8);
    let request = mailbox
        .start_action(MailAction::MoveTo(sys(MailFolder::Archive)), 9)
        .unwrap();
    let _ = mailbox.finish_action(&request, Ok(()));

    assert!(mailbox.undo().is_none());
}

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

    // A short reload means the folder itself is short now.
    let refresh = mailbox.refresh(7).unwrap();
    mailbox.finish_page(refresh.id, dated_page(&[("p0", 1_000_000)], 1));
    assert_eq!(visible_ids(&mailbox), ["p0"]);
}

#[test]
fn a_lost_counts_response_never_blocks_later_refreshes() {
    let mut mailbox = loaded_inbox(&["a"], 1);
    let counts =
        |unread: u32| -> MailboxCounts { [(sys(MailFolder::Inbox), unread)].into_iter().collect() };

    // The response for the first request never arrives.
    assert_eq!(mailbox.refresh_counts(3), Some(3));
    assert_eq!(mailbox.refresh_counts(4), Some(4));

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
        ListStatus::Failed(MailboxError::Connection)
    );
    assert!(mailbox.conversations().is_empty());
}

#[test]
fn folder_switch_resets_pagination() {
    let mut mailbox = loaded_inbox(&["a"], 200);
    mailbox.start_conversation_load("a".into(), 9);

    let request = mailbox.select_folder(sys(MailFolder::Sent), 3).unwrap();

    assert_eq!(request.folder, sys(MailFolder::Sent));
    assert_eq!(request.page, 0);
    assert_eq!(mailbox.status(), ListStatus::Loading(3));
    assert!(mailbox.conversations().is_empty());
    assert!(!mailbox.has_more());
    assert_eq!(mailbox.selected_conversation(), None);
    assert!(mailbox.reader().is_none());
}

#[test]
fn selecting_current_folder_is_a_no_op() {
    let mut mailbox = loaded_inbox(&["a"], 1);

    assert_eq!(mailbox.select_folder(sys(MailFolder::Inbox), 3), None);
    assert_eq!(ids(&mailbox), ["a"]);
}

#[test]
fn reselecting_keeps_expansion_and_switching_resets_it() {
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
    load_detail(&mut mailbox, "a", detail("a", &["a1", "a2"]), 6);
    let reader = mailbox.reader().unwrap();
    assert!(!reader.is_expanded("a1", Reading::default()));
    assert!(reader.is_expanded("a2", Reading::default()));
}

#[test]
fn stale_response_does_not_replace_current_folder() {
    let (mut mailbox, inbox) = open();
    let sent = mailbox.select_folder(sys(MailFolder::Sent), 3).unwrap();

    assert_eq!(mailbox.finish_page(inbox.id, page(&["inbox"], 1)), None);
    assert_eq!(mailbox.status(), ListStatus::Loading(sent.id));
    assert!(mailbox.conversations().is_empty());

    mailbox.finish_page(sent.id, page(&["sent"], 1));
    assert_eq!(mailbox.folder(), &sys(MailFolder::Sent));
    assert_eq!(ids(&mailbox), ["sent"]);
}

#[test]
fn stale_error_is_ignored() {
    let (mut mailbox, inbox) = open();
    mailbox.select_folder(sys(MailFolder::Sent), 3);

    assert_eq!(
        mailbox.finish_page(inbox.id, Err(MailboxError::SessionExpired)),
        None
    );
    assert_eq!(mailbox.status(), ListStatus::Loading(3));
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
fn page_size_controls_whether_more_pages_exist() {
    let (mut mailbox, request) = Mailbox::open(Folder::INBOX, 2, 1, 2);
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
fn refresh_closes_reader_when_conversation_disappears() {
    let mut mailbox = loaded_inbox(&["a", "b"], 2);
    mailbox.start_conversation_load("b".into(), 9);

    let request = mailbox.refresh(3).unwrap();
    mailbox.finish_page(request.id, page(&["a"], 1));

    assert!(mailbox.reader().is_none());
}

#[test]
fn failed_refresh_keeps_previous_list() {
    let mut mailbox = loaded_inbox(&["a"], 1);
    let request = mailbox.refresh(3).unwrap();

    mailbox.finish_page(request.id, Err(MailboxError::Service));

    assert_eq!(ids(&mailbox), ["a"]);
    assert_eq!(mailbox.status(), ListStatus::Failed(MailboxError::Service));
}

#[test]
fn counts_apply_once() {
    let (mut mailbox, _) = open();

    // `open` left request 2 in flight; its response is the one that counts.
    let counts: MailboxCounts = [(sys(MailFolder::Inbox), 4)].into_iter().collect();
    mailbox.finish_counts(2, Ok(counts));
    assert_eq!(
        mailbox.counts().unwrap().unread(&sys(MailFolder::Inbox)),
        Some(4)
    );
    assert_eq!(
        mailbox.counts().unwrap().unread(&sys(MailFolder::Sent)),
        None
    );

    assert_eq!(mailbox.finish_counts(2, Ok(MailboxCounts::default())), None);
    assert_eq!(
        mailbox.counts().unwrap().unread(&sys(MailFolder::Inbox)),
        Some(4)
    );
    assert_eq!(mailbox.refresh_counts(3), Some(3));
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

fn message_row(id: &str) -> ConversationSummary {
    ConversationSummary {
        kind: SummaryKind::Message,
        ..searchable_summary(id, "Bank", "bank@example.com", "Statement", "")
    }
}

#[test]
fn message_rows_load_as_single_messages() {
    let (mut mailbox, request) = open();
    mailbox.finish_page(
        request.id,
        Ok(ConversationPage {
            conversations: vec![message_row("m1"), summary("c1")],
            total: 2,
        }),
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

#[test]
fn stale_classified_page_cannot_replace_newer_folder() {
    let (mut mailbox, inbox) = open();
    mailbox.select_folder(sys(MailFolder::Sent), 3);

    let stale = mailbox.finish_page(
        inbox.id,
        Ok(ConversationPage {
            conversations: vec![message_row("m1"), message_row("m2")],
            total: 1,
        }),
    );

    assert_eq!(stale, None);
    assert_eq!(mailbox.status(), ListStatus::Loading(3));
    assert!(mailbox.conversations().is_empty());
}

#[test]
fn split_rows_are_searchable_and_leave_counts_alone() {
    let (mut mailbox, request) = open();
    let counts: MailboxCounts = [(sys(MailFolder::Inbox), 2)].into_iter().collect();
    mailbox.finish_counts(2, Ok(counts.clone()));
    mailbox.finish_page(
        request.id,
        Ok(ConversationPage {
            conversations: vec![message_row("m1"), message_row("m2"), summary("c1")],
            total: 2,
        }),
    );

    mailbox.set_search_query("bank".into());

    let visible: Vec<_> = mailbox
        .visible_conversations()
        .map(|row| row.id.as_str())
        .collect();
    assert_eq!(visible, ["m1", "m2"]);
    assert_eq!(mailbox.counts(), Some(&counts));
}

fn selected_in(folder: MailFolder, ids: &[&str], selected: &str) -> Mailbox {
    let (mut mailbox, inbox) = open();
    let request = if folder == MailFolder::Inbox {
        inbox.id
    } else {
        mailbox.select_folder(sys(folder), 80).unwrap().id
    };
    mailbox.finish_page(request, page(ids, ids.len() as u32));
    let request = mailbox
        .start_conversation_load(selected.into(), 82)
        .unwrap();
    mailbox.finish_conversation(&request, Ok(detail(selected, &["message"])));
    mailbox
}

fn opened_unread(ids: &[&str], opened: &str) -> Mailbox {
    let (mut mailbox, request) = open();
    let rows = ids
        .iter()
        .map(|id| ConversationSummary {
            unread: true,
            ..summary(id)
        })
        .collect();
    mailbox.finish_page(
        request.id,
        Ok(ConversationPage {
            conversations: rows,
            total: ids.len() as u32,
        }),
    );
    let load = mailbox.start_conversation_load(opened.into(), 82).unwrap();
    mailbox.finish_conversation(&load, Ok(detail(opened, &["message"])));
    mailbox
}

#[test]
fn opened_unread_rows_are_marked_read_once_confirmed() {
    let mut mailbox = opened_unread(&["a", "b"], "a");

    let request = mailbox.start_mark_read(3).unwrap();
    assert_eq!(request.row_id, "a");
    assert_eq!(request.action, MailAction::SetUnread(false));
    assert_eq!(mailbox.start_mark_read(4), None);
    assert!(mailbox.selected_summary().unwrap().unread);

    assert_eq!(mailbox.finish_mark_read(&request, Ok(())), Ok(true));
    assert!(!mailbox.selected_summary().unwrap().unread);
    assert_eq!(mailbox.start_mark_read(5), None);
}

#[test]
fn reads_wait_for_the_content_to_load() {
    let (mut mailbox, request) = open();
    mailbox.finish_page(
        request.id,
        Ok(ConversationPage {
            conversations: vec![ConversationSummary {
                unread: true,
                ..summary("a")
            }],
            total: 1,
        }),
    );
    let load = mailbox.start_conversation_load("a".into(), 3).unwrap();

    assert_eq!(mailbox.start_mark_read(4), None);

    mailbox.finish_conversation(&load, Err(MailboxError::Connection));
    assert_eq!(mailbox.start_mark_read(5), None);
}

#[test]
fn failed_or_stale_reads_leave_the_row_unread() {
    let mut mailbox = opened_unread(&["a"], "a");
    let request = mailbox.start_mark_read(3).unwrap();
    let stale = ActionRequest {
        id: 99,
        ..request.clone()
    };

    assert_eq!(mailbox.finish_mark_read(&stale, Ok(())), Ok(false));
    assert_eq!(
        mailbox.finish_mark_read(&request, Err(MailboxError::Connection)),
        Err(MailboxError::Connection)
    );
    assert!(mailbox.selected_summary().unwrap().unread);
    assert!(mailbox.start_mark_read(4).is_some());
}

#[test]
fn explicit_unread_wins_over_an_automatic_read() {
    let mut mailbox = opened_unread(&["a"], "a");
    let read = mailbox.start_mark_read(3).unwrap();
    let unread = mailbox
        .start_action(MailAction::SetUnread(true), 4)
        .unwrap();

    assert_eq!(mailbox.finish_mark_read(&read, Ok(())), Ok(false));
    assert!(mailbox.selected_summary().unwrap().unread);
    assert_eq!(mailbox.finish_action(&unread, Ok(())), Ok(None));
    assert!(mailbox.selected_summary().unwrap().unread);
}

#[test]
fn reads_after_a_folder_switch_change_nothing() {
    let mut mailbox = opened_unread(&["a"], "a");
    let request = mailbox.start_mark_read(3).unwrap();
    mailbox.select_folder(sys(MailFolder::Sent), 4);
    mailbox.finish_page(
        4,
        Ok(ConversationPage {
            conversations: vec![ConversationSummary {
                unread: true,
                ..summary("a")
            }],
            total: 1,
        }),
    );

    assert_eq!(mailbox.finish_mark_read(&request, Ok(())), Ok(false));
    assert!(mailbox.conversations()[0].unread);
}

#[test]
fn one_action_runs_at_a_time_on_the_selected_row() {
    let mut mailbox = selected_in(MailFolder::Inbox, &["a", "b"], "a");

    let request = mailbox
        .start_action(MailAction::SetStarred(true), 3)
        .unwrap();

    assert_eq!(request.row_id, "a");
    assert_eq!(request.folder, sys(MailFolder::Inbox));
    assert!(mailbox.action_pending());
    assert_eq!(
        mailbox.start_action(MailAction::MoveTo(sys(MailFolder::Archive)), 4),
        None
    );
}

#[test]
fn actions_wait_for_the_list_to_load() {
    let mut mailbox = selected_in(MailFolder::Inbox, &["a"], "a");
    mailbox.refresh(3);

    assert_eq!(
        mailbox.start_action(MailAction::MoveTo(sys(MailFolder::Archive)), 4),
        None
    );
    assert!(!mailbox.action_pending());
}

#[test]
fn confirmed_flag_changes_update_the_row() {
    let mut mailbox = selected_in(MailFolder::Inbox, &["a", "b"], "a");

    let unread = mailbox
        .start_action(MailAction::SetUnread(true), 3)
        .unwrap();
    assert_eq!(mailbox.finish_action(&unread, Ok(())), Ok(None));
    let star = mailbox
        .start_action(MailAction::SetStarred(true), 4)
        .unwrap();
    assert_eq!(mailbox.finish_action(&star, Ok(())), Ok(None));

    let row = mailbox.selected_summary().unwrap();
    assert!(row.unread && row.starred);
    assert!(!mailbox.action_pending());
}

#[test]
fn confirmed_moves_remove_the_row_and_open_the_next() {
    let mut mailbox = selected_in(MailFolder::Inbox, &["a", "b", "c"], "b");

    let request = mailbox
        .start_action(MailAction::MoveTo(sys(MailFolder::Archive)), 3)
        .unwrap();

    assert_eq!(
        mailbox.finish_action(&request, Ok(())),
        Ok(Some("c".into()))
    );
    assert_eq!(ids(&mailbox), ["a", "c"]);
    assert_eq!(mailbox.reader_state(), &ReaderState::Empty);
}

#[test]
fn moving_into_the_current_folder_keeps_the_row() {
    let mut mailbox = selected_in(MailFolder::Archive, &["a"], "a");

    let request = mailbox
        .start_action(MailAction::MoveTo(sys(MailFolder::Archive)), 3)
        .unwrap();

    assert_eq!(mailbox.finish_action(&request, Ok(())), Ok(None));
    assert_eq!(ids(&mailbox), ["a"]);
}

#[test]
fn unstarring_in_starred_removes_the_row() {
    let mut mailbox = selected_in(MailFolder::Starred, &["a", "b"], "b");

    let request = mailbox
        .start_action(MailAction::SetStarred(false), 3)
        .unwrap();

    assert_eq!(
        mailbox.finish_action(&request, Ok(())),
        Ok(Some("a".into()))
    );
    assert_eq!(ids(&mailbox), ["a"]);
}

#[test]
fn failed_actions_keep_rows_and_report_the_error() {
    let mut mailbox = selected_in(MailFolder::Inbox, &["a", "b"], "a");
    let request = mailbox
        .start_action(MailAction::MoveTo(sys(MailFolder::Trash)), 3)
        .unwrap();

    let result = mailbox.finish_action(&request, Err(MailboxError::Service));

    assert_eq!(result, Err(MailboxError::Service));
    assert_eq!(ids(&mailbox), ["a", "b"]);
    assert_eq!(mailbox.action_error(), Some(MailboxError::Service));
    assert!(!mailbox.action_pending());
}

#[test]
fn stale_or_moved_action_responses_change_nothing() {
    let mut mailbox = selected_in(MailFolder::Inbox, &["a", "b"], "a");
    let request = mailbox
        .start_action(MailAction::MoveTo(sys(MailFolder::Archive)), 3)
        .unwrap();
    let stale = ActionRequest {
        id: 99,
        ..request.clone()
    };

    assert_eq!(mailbox.finish_action(&stale, Ok(())), Ok(None));
    assert_eq!(ids(&mailbox), ["a", "b"]);
    assert!(mailbox.action_pending());

    mailbox.select_folder(sys(MailFolder::Sent), 4);
    mailbox.finish_page(4, page(&["a"], 1));
    assert_eq!(mailbox.finish_action(&request, Ok(())), Ok(None));
    assert_eq!(ids(&mailbox), ["a"]);
}

#[test]
fn rows_without_a_time_keep_their_place() {
    let timed = |id: &str, time| ConversationSummary {
        time: Some(time),
        ..summary(id)
    };
    let (mut mailbox, request) = open();

    mailbox.finish_page(
        request.id,
        Ok(ConversationPage {
            conversations: vec![
                timed("c1", 50),
                summary("undated"),
                timed("m1", 10),
                timed("c2", 30),
            ],
            total: 4,
        }),
    );

    assert_eq!(ids(&mailbox), ["c1", "undated", "c2", "m1"]);
}

#[test]
fn rows_stay_newest_first_across_pages() {
    let timed = |id: &str, time| ConversationSummary {
        time: Some(time),
        ..summary(id)
    };
    let (mut mailbox, request) = open();
    mailbox.finish_page(
        request.id,
        Ok(ConversationPage {
            conversations: vec![
                timed("c1", 50),
                timed("m1", 40),
                timed("m2", 10),
                timed("c2", 30),
            ],
            total: 120,
        }),
    );
    assert_eq!(ids(&mailbox), ["c1", "m1", "c2", "m2"]);

    let more = mailbox.load_more(3).unwrap();
    mailbox.finish_page(
        more.id,
        Ok(ConversationPage {
            conversations: vec![timed("c3", 20)],
            total: 120,
        }),
    );

    assert_eq!(ids(&mailbox), ["c1", "m1", "c2", "c3", "m2"]);
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
        Ok(ConversationPage {
            conversations: vec![searchable_summary(
                "archived",
                "Archived Sender",
                "archive@example.com",
                "Archived Rust notes",
                "Old project",
            )],
            total: 1,
        }),
    );

    assert_eq!(mailbox.folder(), &sys(MailFolder::Archive));
    assert_eq!(visible_ids(&mailbox), ["archived"]);
}

#[test]
fn clearing_search_restores_normal_results() {
    let mut mailbox = searchable_mailbox();
    mailbox.set_search_query("rust".into());

    mailbox.set_search_query(String::new());

    assert_eq!(visible_ids(&mailbox), ["sender", "subject", "preview"]);
}

#[test]
fn no_match_clears_hidden_reader_selection() {
    let mut mailbox = searchable_mailbox();
    mailbox.start_conversation_load("sender".into(), 3);

    mailbox.set_search_query("rust".into());

    assert_eq!(mailbox.selected_conversation(), None);
    assert_eq!(visible_ids(&mailbox), ["subject"]);
}

#[test]
fn search_does_not_change_mailbox_counts() {
    let mut mailbox = searchable_mailbox();
    let counts: MailboxCounts = [(sys(MailFolder::Inbox), 7)].into_iter().collect();
    mailbox.finish_counts(2, Ok(counts.clone()));

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
