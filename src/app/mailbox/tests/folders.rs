//! Switching folders: listings kept for later, renames, and late answers.

use super::*;

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
fn returning_to_a_loaded_folder_restores_its_rows_and_pagination() {
    let mut mailbox = loaded_inbox(&["a", "b"], 120);
    let sent = mailbox.select_folder(sys(MailFolder::Sent), 3).unwrap();
    mailbox.finish_page(sent.id, page(&["sent"], 1));

    assert_eq!(mailbox.select_folder(sys(MailFolder::Inbox), 4), None);
    assert_eq!(mailbox.status(), ListStatus::Loaded);
    assert_eq!(ids(&mailbox), ["a", "b"]);
    assert!(mailbox.has_more());

    let more = mailbox.load_more(5).unwrap();
    assert_eq!(more.page, 1);
}

#[test]
fn a_changed_mailbox_restores_cached_rows_while_revalidating() {
    let mut mailbox = loaded_inbox(&["inbox"], 1);
    let sent = mailbox.select_folder(sys(MailFolder::Sent), 3).unwrap();
    mailbox.finish_page(sent.id, page(&["sent"], 1));
    load_detail(&mut mailbox, "sent", detail("sent", &["message"]), 4);
    let action = send(&mut mailbox, MailAction::SetStarred(true), 5);
    assert_eq!(mailbox.finish_action(&action, Ok(())).result, Ok(()));

    let refresh = mailbox
        .select_folder(sys(MailFolder::Inbox), 6)
        .expect("a mutation makes cached folders revalidate");
    assert_eq!(refresh.page, 0);
    assert_eq!(mailbox.status(), ListStatus::Refreshing(refresh.id));
    assert_eq!(ids(&mailbox), ["inbox"]);
}

#[test]
fn a_failed_cached_refresh_is_retried_on_the_next_visit() {
    let mut mailbox = loaded_inbox(&["inbox"], 1);
    let sent = mailbox.select_folder(sys(MailFolder::Sent), 3).unwrap();
    mailbox.finish_page(sent.id, page(&["sent"], 1));
    mailbox.invalidate_listings();

    let refresh = mailbox.select_folder(sys(MailFolder::Inbox), 4).unwrap();
    mailbox.finish_page(refresh.id, Err(MailboxError::Connection));
    let archive = mailbox.select_folder(sys(MailFolder::Archive), 5).unwrap();
    mailbox.finish_page(archive.id, page(&["archived"], 1));

    let retry = mailbox
        .select_folder(sys(MailFolder::Inbox), 6)
        .expect("a failed refresh does not make stale rows clean");
    assert_eq!(mailbox.status(), ListStatus::Refreshing(retry.id));
    assert_eq!(ids(&mailbox), ["inbox"]);
}

#[test]
fn a_custom_folder_cache_survives_a_rename() {
    let mut mailbox = loaded_inbox(&["inbox"], 1);
    let old_name = Folder::custom("folder-id", "Receipts");
    let custom = mailbox.select_folder(old_name, 3).unwrap();
    mailbox.finish_page(custom.id, page(&["receipt"], 1));
    assert_eq!(mailbox.select_folder(sys(MailFolder::Inbox), 4), None);

    let new_name = Folder::custom("folder-id", "Invoices");
    assert_eq!(mailbox.select_folder(new_name.clone(), 5), None);
    assert_eq!(mailbox.folder(), &new_name);
    assert_eq!(ids(&mailbox), ["receipt"]);
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
fn stale_classified_page_cannot_replace_newer_folder() {
    let (mut mailbox, inbox) = open();
    mailbox.select_folder(sys(MailFolder::Sent), 3);

    let stale = mailbox.finish_page(
        inbox.id,
        page_of(vec![message_row("m1"), message_row("m2")], 1),
    );

    assert_eq!(stale, None);
    assert_eq!(mailbox.status(), ListStatus::Loading(3));
    assert!(mailbox.conversations().is_empty());
}

#[test]
fn only_the_folders_left_last_stay_loaded() {
    let mut mailbox = loaded_inbox(&["a"], 1);
    let folders: Vec<_> = (0..9)
        .map(|index| Folder::custom(format!("f{index}"), format!("Folder {index}")))
        .collect();
    for (request, folder) in (10..).zip(&folders) {
        let page_request = mailbox.select_folder(folder.clone(), request).unwrap();
        mailbox.finish_page(page_request.id, page(&["x"], 1));
    }
    // Inbox and f0 to f7 were left behind; only eight stay.
    assert_eq!(mailbox.cached_listings.len(), LISTING_CACHE_CAPACITY);

    // f0 is still loaded and opens at once.
    assert!(mailbox.select_folder(folders[0].clone(), 30).is_none());
    // Inbox, left longest ago, was let go and loads again.
    assert!(mailbox.select_folder(Folder::INBOX, 31).is_some());
    assert!(matches!(mailbox.status(), ListStatus::Loading(31)));
}
