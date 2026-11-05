//! Conversations split into their messages, and the rows that stand for them.

use super::*;

/// The first inbox page with its last row, p49, split into two messages:
/// the inbox then shows one row more than a page.
fn inspected_last_inbox() -> (Mailbox, Vec<ConversationSummary>) {
    let (mut mailbox, first) = open();
    mailbox.finish_page(first.id, full_page(0, 120));
    let members = vec![
        split_message("m49-a", page_time(49)),
        split_message("m49-b", page_time(60)),
    ];
    mailbox.split_conversation(&Folder::INBOX, "p49", members.clone());
    (mailbox, members)
}

#[test]
fn load_more_and_read_completion_refresh_preserve_split_reader() {
    let (mut mailbox, first) = open();
    mailbox.finish_page(first.id, full_page(0, 120));
    let split = vec![
        ConversationSummary {
            unread: true,
            ..split_message("m40-a", page_time(40))
        },
        split_message("m40-b", page_time(50)),
    ];
    mailbox.split_conversation(&Folder::INBOX, "p40", split.clone());
    load_detail(&mut mailbox, "m40-a", detail("m40-a", &["m40-a"]), 9);
    mailbox.toggle_message("m40-a");
    let reader = mailbox.reader_state().clone();
    let read = mailbox.start_mark_read(10).unwrap();
    let next = mailbox.load_more(11).unwrap();

    mailbox.finish_mark_read(&read, Ok(()));
    mailbox.finish_page(next.id, full_page(PAGE_SIZE, 120));
    assert!(mailbox.needs_revalidation());
    let refresh = mailbox.refresh(12).unwrap();
    mailbox.finish_page(refresh.id, full_page(0, 120));

    assert_eq!(mailbox.reader_state(), &reader);
    assert_eq!(mailbox.selected_conversation(), Some("m40-a"));
    assert!(mailbox.selected_summary().is_some());
    assert!(mailbox.reader_stamp.is_some());
    mailbox.split_conversation(&Folder::INBOX, "p40", split);
    assert!(mailbox.selected_summary().is_some());
    assert_eq!(mailbox.reader_state(), &reader);
}

#[test]
fn split_rows_are_searchable_and_leave_counts_alone() {
    let (mut mailbox, request) = open();
    let counts: MailboxCounts = [(sys(MailFolder::Inbox), 2)].into_iter().collect();
    mailbox.finish_counts(OPENING_COUNTS, Ok(counts.clone()));
    mailbox.finish_page(
        request.id,
        page_of(vec![message_row("m1"), message_row("m2"), summary("c1")], 2),
    );

    mailbox.set_search_query("bank".into());

    let visible: Vec<_> = mailbox
        .visible_conversations()
        .map(|row| row.id.as_str())
        .collect();
    assert_eq!(visible, ["m1", "m2"]);
    assert_eq!(mailbox.counts(), Some(&counts));
}

#[test]
fn split_conversation_replaces_conversation_with_message_rows_and_updates_visible() {
    let (mut mailbox, request) = open();
    mailbox.finish_page(
        request.id,
        Ok(ConversationPage {
            conversations: vec![dated("c1", 50), dated("c2", 20)],
            total: 2,
            inspect_candidates: vec!["c1".into()],
        }),
    );

    assert_eq!(visible_ids(&mailbox), ["c1", "c2"]);

    let split_rows = vec![split_message("m1", 60), split_message("m2", 40)];

    mailbox.split_conversation(&Folder::INBOX, "c1", split_rows);

    assert_eq!(visible_ids(&mailbox), ["m1", "m2", "c2"]);
    assert_eq!(mailbox.visible_count(), 3);
}

#[test]
fn work_asked_under_a_folder_s_old_name_still_lands_there() {
    let invoices = Folder::custom("kZ9", "Invoices");
    let mut mailbox = loaded_inbox(&["a"], 1);
    let request = mailbox.select_folder(invoices.clone(), 2).unwrap();
    mailbox.finish_page(
        request.id,
        Ok(ConversationPage {
            conversations: vec![dated("c1", 50), dated("c2", 20)],
            total: 2,
            inspect_candidates: vec!["c1".into()],
        }),
    );
    mailbox.rename_folder(Folder::custom("kZ9", "Facturas"));

    // Proton knows the folder by its id, so a split asked for under the
    // old name is still this folder's.
    assert!(mailbox.shows(&invoices));
    mailbox.split_conversation(&invoices, "c1", vec![split_message("m1", 60)]);
    assert_eq!(visible_ids(&mailbox), ["m1", "c2"]);
    // Another folder is not this one.
    assert!(!mailbox.shows(&Folder::custom("wN2", "Facturas")));
}

#[test]
fn split_conversation_ignores_nonexistent_or_empty() {
    let (mut mailbox, request) = open();
    mailbox.finish_page(request.id, page(&["c1"], 1));

    mailbox.split_conversation(&Folder::INBOX, "c1", Vec::new());
    assert_eq!(visible_ids(&mailbox), ["c1"]);

    mailbox.split_conversation(&Folder::INBOX, "nonexistent", vec![summary("m1")]);
    assert_eq!(visible_ids(&mailbox), ["c1"]);
}

#[test]
fn split_conversation_updates_cached_listing_when_folder_not_active() {
    let (mut mailbox, request) = open();
    mailbox.finish_page(request.id, page(&["c1"], 1));

    // Navigating to Sent moves Inbox to cached_listings
    let sent_request = mailbox.select_folder(sys(MailFolder::Sent), 2).unwrap();
    mailbox.finish_page(sent_request.id, page(&["s1"], 1));
    assert_eq!(visible_ids(&mailbox), ["s1"]);

    // Background inspection finishes for Inbox while Sent is active
    let split_rows = vec![split_message("m1", 60), split_message("m2", 40)];
    mailbox.split_conversation(&Folder::INBOX, "c1", split_rows);

    // Sent is still visible and unaffected
    assert_eq!(visible_ids(&mailbox), ["s1"]);

    // Navigating back to Inbox restores cached listing with split rows already applied!
    mailbox.select_folder(sys(MailFolder::Inbox), 3);
    assert_eq!(visible_ids(&mailbox), ["m1", "m2"]);
}

#[test]
fn cached_split_rows_are_not_added_again_by_the_next_page() {
    let mut mailbox = loaded_inbox(&["c1", "c2"], 120);
    let sent = mailbox.select_folder(sys(MailFolder::Sent), 3).unwrap();
    mailbox.finish_page(sent.id, page(&["sent"], 1));
    mailbox.split_conversation(&Folder::INBOX, "c1", vec![dated("m1", 50), dated("m2", 40)]);

    assert_eq!(mailbox.select_folder(Folder::INBOX, 4), None);
    let next = mailbox.load_more(5).unwrap();
    mailbox.finish_page(next.id, page(&["m1", "c3"], 120));

    assert_eq!(ids(&mailbox), ["m1", "m2", "c2", "c3"]);
}

#[test]
fn repeated_refresh_reuses_split_rows_and_reinspection_updates_without_duplicates() {
    let (mut mailbox, split) = inspected_last_inbox();
    let original = visible_ids(&mailbox)
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    for request in 10..14 {
        let refresh = mailbox.refresh(request).unwrap();
        mailbox.finish_page(refresh.id, full_page(0, 120));
        assert_eq!(visible_ids(&mailbox), original);
        assert!(mailbox.has_inspection_target("p49"));
        let mut updated = split.clone();
        updated[1].subject = Some(format!("fresh {request}"));
        mailbox.split_conversation(&Folder::INBOX, "p49", updated);
        assert_eq!(visible_ids(&mailbox), original);
        assert_eq!(mailbox.visible_count(), PAGE_SIZE as usize + 1);
        assert_eq!(mailbox.row_ids.len(), PAGE_SIZE as usize + 1);
        assert_eq!(
            mailbox.row("m49-b").unwrap().subject.as_deref(),
            Some(format!("fresh {request}").as_str())
        );
    }
}

#[test]
fn changed_conversation_invalidates_its_split_members_even_below_the_page_boundary() {
    let (mut mailbox, _) = inspected_last_inbox();
    let refresh = mailbox.refresh(10).unwrap();
    let mut changed = full_page(0, 120).unwrap();
    changed.conversations.last_mut().unwrap().message_count += 1;
    mailbox.finish_page(refresh.id, Ok(changed));
    assert_eq!(mailbox.visible_count(), PAGE_SIZE as usize);
    assert!(mailbox.has_row("p49"));
    assert!(!mailbox.has_row("m49-a"));
    assert!(!mailbox.has_row("m49-b"));
    assert!(mailbox.groups.splits.is_empty());
    assert!(mailbox.groups.parents.is_empty());
}

#[test]
fn refreshed_read_and_star_flags_keep_grouping_until_fresh_inspection_updates_members() {
    let (mut mailbox, mut split) = inspected_last_inbox();
    let refresh = mailbox.refresh(10).unwrap();
    let mut page = full_page(0, 120).unwrap();
    let last = page.conversations.last_mut().unwrap();
    last.unread = true;
    last.starred = true;
    mailbox.finish_page(refresh.id, Ok(page));
    assert_eq!(mailbox.visible_count(), PAGE_SIZE as usize + 1);
    assert!(!mailbox.has_row("p49"));
    assert!(mailbox.groups.splits["p49"].source.unread);
    assert!(mailbox.groups.splits["p49"].source.starred);
    split[1].unread = true;
    split[1].starred = true;
    mailbox.split_conversation(&Folder::INBOX, "p49", split);
    assert!(mailbox.row("m49-b").unwrap().unread);
    assert!(mailbox.row("m49-b").unwrap().starred);
    assert_eq!(mailbox.row_ids.len(), PAGE_SIZE as usize + 1);
}

#[test]
fn overlapping_pages_keep_verified_groups_and_deduplicate_new_rows() {
    let (mut mailbox, _) = inspected_last_inbox();
    let next = mailbox.load_more(10).unwrap();
    let mut page = full_page(PAGE_SIZE, 120).unwrap();
    page.conversations.insert(0, dated("p49", page_time(49)));
    page.conversations.push(dated("p50", page_time(50)));
    mailbox.finish_page(next.id, Ok(page));
    assert_eq!(mailbox.visible_count(), 2 * PAGE_SIZE as usize + 1);
    assert_eq!(mailbox.row_ids.len(), 2 * PAGE_SIZE as usize + 1);
    assert!(!mailbox.has_row("p49"));
    assert!(mailbox.has_row("m49-b"));
}

#[test]
fn split_refresh_decisions_follow_cached_folders_and_are_pruned_with_their_rows() {
    let (mut mailbox, _) = inspected_last_inbox();
    let sent = mailbox.select_folder(sys(MailFolder::Sent), 10).unwrap();
    mailbox.finish_page(sent.id, page(&["sent"], 1));
    assert!(mailbox.groups.splits.is_empty());
    assert_eq!(mailbox.select_folder(Folder::INBOX, 11), None);
    let refresh = mailbox.refresh(12).unwrap();
    mailbox.finish_page(refresh.id, full_page(0, 120));
    assert!(mailbox.has_row("m49-b"));
    assert_eq!(mailbox.groups.splits.len(), 1);
    let refresh = mailbox.refresh(13).unwrap();
    mailbox.finish_page(refresh.id, page(&[], 0));
    assert!(mailbox.groups.splits.is_empty());
    assert!(mailbox.groups.parents.is_empty());
}

#[test]
fn deeper_split_conversations_keep_all_members_by_parent_time() {
    let (mut mailbox, first) = open();
    mailbox.finish_page(first.id, full_page(0, 120));
    let next = mailbox.load_more(10).unwrap();
    mailbox.finish_page(next.id, full_page(PAGE_SIZE, 120));
    mailbox.split_conversation(
        &Folder::INBOX,
        "p60",
        vec![
            split_message("m60-a", page_time(60)),
            split_message("m60-b", page_time(200)),
        ],
    );
    let refresh = mailbox.refresh(11).unwrap();
    mailbox.finish_page(refresh.id, full_page(0, 120));
    assert!(mailbox.has_row("m60-a") && mailbox.has_row("m60-b"));
    assert!(mailbox.has_inspection_target("p60"));
    assert_eq!(mailbox.visible_count(), 2 * PAGE_SIZE as usize + 1);
}

#[test]
fn inspection_replaces_old_members_and_duplicate_input_with_fresh_metadata() {
    let mut mailbox = loaded_inbox(&["c1", "m2", "other"], 3);
    let mut fresh = split_message("m2", 50);
    fresh.subject = Some("fresh metadata".into());
    mailbox.split_conversation(
        &Folder::INBOX,
        "c1",
        vec![split_message("m1", 60), fresh.clone(), fresh],
    );
    assert_eq!(ids(&mailbox), ["m1", "m2", "other"]);
    assert_eq!(
        mailbox.row("m2").unwrap().subject.as_deref(),
        Some("fresh metadata")
    );
    mailbox.split_conversation(&Folder::INBOX, "c1", vec![split_message("m3", 70)]);
    assert_eq!(ids(&mailbox), ["m3", "other"]);
    assert_eq!(mailbox.groups.parents.len(), 1);
    assert!(!mailbox.groups.parents.contains_key("m1"));
}

#[test]
fn a_locally_removed_member_cannot_be_restored_from_a_cached_group() {
    let mut mailbox = loaded_inbox(&["c1"], 1);
    mailbox.split_conversation(
        &Folder::INBOX,
        "c1",
        vec![split_message("m1", 60), split_message("m2", 50)],
    );
    mailbox.remove_row("m1");
    let refresh = mailbox.refresh(10).unwrap();
    mailbox.finish_page(refresh.id, page(&["c1"], 1));
    assert_eq!(ids(&mailbox), ["c1"]);
    assert!(mailbox.groups.splits.is_empty());
}
