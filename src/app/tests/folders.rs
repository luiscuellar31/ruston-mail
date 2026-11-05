//! The account's folders, the one the mailbox opens in, and loading its pages.

use super::*;

#[test]
fn signing_out_takes_the_account_folders_with_it() {
    let mut app = loaded_demo_app();
    let previous_epoch = app.session_epoch;
    deliver_folders(
        &mut app,
        Ok(vec![
            Folder::custom("kZ9", "Invoices"),
            Folder::label("wN2", "Receipts"),
        ]),
    );
    assert_eq!(app.folders().len(), 2);

    let _ = app.update(Message::Logout);

    // The next sign-in can be a different account, and its sidebar must
    // not open showing names that belong to the last one.
    assert!(app.folders().is_empty());

    let _ = app.open_mailbox(MailBackend::demo(), None, None);
    let _ = app.update(Message::FoldersLoaded(
        previous_epoch,
        Ok(crate::mail::OwnPlaces {
            folders: vec![Folder::custom("old", "Previous account")],
            ..Default::default()
        }),
    ));
    assert!(app.folders().is_empty());
}

#[test]
fn a_place_shows_as_its_list_says_until_the_session_ends() {
    let mut app = loaded_demo_app();
    let nested = Folder::custom("nested", "2024");
    let label = Folder::label("work", "Work");
    let looks = [
        (
            "nested",
            crate::mail::PlaceLook {
                depth: 1,
                color: None,
            },
        ),
        (
            "work",
            crate::mail::PlaceLook {
                depth: 0,
                color: Some([0, 102, 204]),
            },
        ),
    ]
    .into_iter()
    .map(|(id, look)| (id.to_owned(), look))
    .collect();
    let epoch = app.session_epoch;
    let _ = app.update(Message::FoldersLoaded(
        epoch,
        Ok(crate::mail::OwnPlaces {
            folders: vec![nested.clone(), label.clone()],
            looks,
        }),
    ));
    assert_eq!(app.place_look(&nested).depth, 1);
    assert_eq!(app.place_look(&label).color, Some([0, 102, 204]));
    assert_eq!(
        app.place_look(&Folder::INBOX),
        crate::mail::PlaceLook::default()
    );

    let _ = app.update(Message::Logout);
    assert_eq!(app.place_look(&label), crate::mail::PlaceLook::default());
}

#[test]
fn the_account_folders_reach_the_sidebar() {
    let mut app = loaded_demo_app();
    assert!(app.folders().is_empty(), "the demo account has none");

    let invoices = Folder::custom("kZ9", "Invoices");
    let receipts = Folder::label("wN2", "Receipts");
    deliver_folders(&mut app, Ok(vec![invoices.clone(), receipts.clone()]));

    assert_eq!(app.folders(), [invoices, receipts]);
}

#[test]
fn the_mailbox_opens_where_it_was_left() {
    let (app, _) = App::boot(true, Settings::opening(sys(MailFolder::Archive)));

    assert_eq!(app.mailbox().unwrap().folder(), &sys(MailFolder::Archive));
}

#[test]
fn a_pinned_folder_opens_instead_of_the_one_last_read() {
    let (app, _) = App::boot(
        true,
        Settings::opening(sys(MailFolder::Spam))
            .with_start(StartFolder::Always(MailFolder::Archive)),
    );

    assert_eq!(app.mailbox().unwrap().folder(), &sys(MailFolder::Archive));
}

#[test]
fn the_demo_opens_its_own_mailbox_and_leaves_the_setting_alone() {
    let invoices = Folder::custom("kZ9", "Invoices");
    let (mut app, _) = App::boot(true, Settings::opening(invoices.clone()));

    // Demo mode cannot open a real account folder.
    assert_eq!(app.mailbox().unwrap().folder(), &Folder::INBOX);

    let _ = app.update(Message::SelectFolder(sys(MailFolder::Archive)));
    // Reading demo mail says nothing about where the real mailbox was
    // left, so the remembered folder is untouched.
    assert_eq!(app.settings.folder, invoices);
}

#[test]
fn a_renamed_folder_is_taken_up_and_a_removed_one_falls_back() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::SelectFolder(Folder::custom("kZ9", "Invoices")));

    // The account renamed it since the settings file was written, so the
    // sidebar and the header agree on the name it has now.
    let renamed = Folder::custom("kZ9", "Facturas");
    deliver_folders(&mut app, Ok(vec![renamed.clone()]));
    assert_eq!(app.mailbox().unwrap().folder(), &renamed);

    // Once it is gone from the account there is nothing to show under it,
    // so reading falls back to the one folder that is always there.
    deliver_folders(&mut app, Ok(Vec::new()));
    assert_eq!(app.mailbox().unwrap().folder(), &Folder::INBOX);
}

#[test]
fn a_remembered_custom_folder_does_not_outlive_the_session() {
    let invoices = Folder::custom("kZ9", "Invoices");
    let mut app = authenticated_app();
    let _ = app.update(Message::SelectFolder(invoices.clone()));
    assert_eq!(app.settings.folder, invoices);
    app.close_mailbox(None);
    // The next account starts in its own Inbox.
    assert_eq!(app.settings.start_folder(), Folder::INBOX);
    assert!(app.settings_unsaved(), "settings.json forgets it too");

    // Proton's own folders are the same for every account.
    let mut app = authenticated_app();
    let _ = app.update(Message::SelectFolder(sys(MailFolder::Archive)));
    app.close_mailbox(None);
    assert_eq!(app.settings.folder, sys(MailFolder::Archive));

    // Leaving the demo keeps the real account's remembered folder.
    let (mut app, _) = App::boot(true, Settings::opening(invoices.clone()));
    app.close_mailbox(None);
    assert_eq!(app.settings.folder, invoices);
}

#[test]
fn a_folder_list_that_fails_to_load_keeps_the_open_folder_and_is_asked_again() {
    let invoices = Folder::custom("kZ9", "Invoices");

    // A list that never arrived says nothing about the folder being gone, so
    // neither the open folder nor the remembered one changes.
    let mut app = authenticated_app();
    let _ = app.update(Message::SelectFolder(invoices.clone()));
    deliver_folders(&mut app, Err(MailboxError::Connection));
    assert_eq!(app.mailbox().unwrap().folder(), &invoices);
    assert_eq!(app.settings.folder, invoices);

    // The next refresh asks for the list again, besides the page and counts,
    // and only until it arrives.
    let mut app = loaded_demo_app();
    deliver_folders(&mut app, Err(MailboxError::Connection));
    assert_eq!(
        app.update(Message::RefreshMailbox).units(),
        PAGE_AND_COUNTS + 1
    );
    let page_request = pending_list_request(&app);
    deliver_demo_page(&mut app, page_request, 0);
    deliver_folders(&mut app, Ok(vec![invoices.clone()]));
    assert_eq!(app.folders(), [invoices]);
    assert_eq!(app.update(Message::RefreshMailbox).units(), PAGE_AND_COUNTS);
}

#[test]
fn demo_mailbox_uses_production_pagination() {
    let mut app = loaded_demo_app();

    let mailbox = app.mailbox().unwrap();
    assert_eq!(mailbox.conversations().len(), demo::PAGE_SIZE as usize);
    assert!(mailbox.has_more());
    assert_eq!(
        mailbox.counts().unwrap().unread(&sys(MailFolder::Inbox)),
        Some(6)
    );

    for page in 1..=3 {
        let _ = app.update(Message::LoadMoreConversations);
        deliver_latest_demo_page(&mut app, page);
    }

    let mailbox = app.mailbox().unwrap();
    assert_eq!(mailbox.conversations().len(), 35);
    assert!(!mailbox.has_more());
    assert_eq!(mailbox.status(), ListStatus::Loaded);
}

#[test]
fn demo_empty_and_spam_folders_load() {
    let (mut app, _) = App::boot(true, Settings::default());

    let _ = app.update(Message::SelectFolder(sys(MailFolder::Trash)));
    deliver_latest_demo_page(&mut app, 0);
    let mailbox = app.mailbox().unwrap();
    assert_eq!(mailbox.status(), ListStatus::Loaded);
    assert!(mailbox.conversations().is_empty());

    let _ = app.update(Message::SelectFolder(sys(MailFolder::Spam)));
    deliver_latest_demo_page(&mut app, 0);
    let mailbox = app.mailbox().unwrap();
    assert_eq!(mailbox.status(), ListStatus::Loaded);
    assert_eq!(mailbox.conversations().len(), 2);
    assert!(app.is_demo());
}

#[test]
fn returning_to_a_loaded_folder_does_not_fetch_it_again() {
    let mut app = loaded_demo_app();
    let inbox_ids: Vec<_> = app
        .mailbox()
        .unwrap()
        .conversations()
        .iter()
        .map(|row| row.id.clone())
        .collect();

    assert_eq!(
        app.update(Message::SelectFolder(sys(MailFolder::Sent)))
            .units(),
        1
    );
    deliver_latest_demo_page(&mut app, 0);

    assert_eq!(
        app.update(Message::SelectFolder(sys(MailFolder::Inbox)))
            .units(),
        0
    );
    let mailbox = app.mailbox().unwrap();
    assert_eq!(mailbox.status(), ListStatus::Loaded);
    assert_eq!(
        mailbox
            .conversations()
            .iter()
            .map(|row| row.id.clone())
            .collect::<Vec<_>>(),
        inbox_ids
    );
}
