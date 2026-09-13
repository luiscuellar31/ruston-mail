//! What the app does, exercised through `App::update`.
//!
//! These reach across the whole state machine rather than one part of
//! it, which is why they sit beside the module instead of inside one of
//! its files.

use super::*;
use crate::mail::SendError;
use crate::mail::demo;
use crate::settings::{ComposePlacement, Reading};

use crate::mail::MailFolder;

/// One of Proton's own folders, as a place to list from.
fn sys(folder: MailFolder) -> Folder {
    Folder::System(folder)
}

const NOW: i64 = 1_789_000_000;

fn authenticated_app() -> App {
    let mut app = App::new(Settings::default());
    app.auth_state = AuthState::Authenticated { email: None };
    app.mailbox = Some(Mailbox::open(Folder::INBOX, 50, 1, 2).0);
    app.last_request = 2;
    app
}

/// Delivers the demo page a request would produce, as the async task would.
fn deliver_demo_page(app: &mut App, request: RequestId, page: u32) {
    let folder = app.mailbox().unwrap().folder();
    let result = demo_service(app).list_conversations(folder, page, demo::PAGE_SIZE, NOW);
    let _ = app.update(Message::ConversationsLoaded(request, result));
}

fn deliver_latest_demo_page(app: &mut App, page: u32) {
    let request = app.last_request;
    deliver_demo_page(app, request, page);
}

fn demo_service(app: &App) -> demo::DemoMailbox {
    match app.backend.as_ref() {
        Some(MailBackend::Demo(service)) => service.clone(),
        _ => panic!("expected demo backend"),
    }
}

fn pending_reader_request(app: &App) -> ReaderRequest {
    match app.mailbox().unwrap().reader_state() {
        ReaderState::Loading {
            conversation_id,
            request,
        } => ReaderRequest {
            id: *request,
            conversation_id: conversation_id.clone(),
            kind: crate::mail::SummaryKind::Conversation,
        },
        _ => panic!("expected pending reader request"),
    }
}

fn deliver_selected_demo_detail(app: &mut App) {
    let request = pending_reader_request(app);
    let result = demo_service(app).conversation_detail(&request.conversation_id, NOW);
    let _ = app.update(Message::ConversationLoaded(request, result));
}

/// Runs what is typed against the demo backend, the way pressing Enter
/// asks the server for everything matching it.
fn deliver_demo_search(app: &mut App, query: &str) {
    let _ = app.update(Message::SearchChanged(query.to_owned()));
    let _ = app.update(Message::SearchSubmitted);
    let request = SearchRequest {
        id: app.last_request,
        query: query.to_owned(),
    };
    let result = demo_service(app).search(query, demo::PAGE_SIZE, NOW);
    let _ = app.update(Message::SearchLoaded(request, result));
}

#[test]
fn only_a_real_change_counts_as_one() {
    // A drag reports the same value over and over. The revision is what
    // the shell watches to know a value has stopped moving, so repeating
    // a value must not keep resetting that.
    let mut app = loaded_demo_app();
    assert_eq!(app.settings_revision(), 0);
    assert!(!app.settings_unsaved());

    let _ = app.update(Message::SetZoom(1.3));
    assert_eq!(app.settings_revision(), 1);

    let _ = app.update(Message::SetZoom(1.3));
    assert_eq!(
        app.settings_revision(),
        1,
        "the same value again is no change"
    );

    let _ = app.update(Message::SetZoom(1.5));
    assert_eq!(app.settings_revision(), 2);
}

#[test]
fn a_change_waits_to_be_written_and_is_written_once() {
    let mut app = loaded_demo_app();

    let _ = app.update(Message::SetConfirmLinks(false));
    // Keeping it applies at once, whether or not it has reached the file.
    assert!(!app.settings().confirm_links);
    assert!(app.settings_unsaved());

    app.save_settings();
    assert!(!app.settings_unsaved());

    // Nothing further to write until something changes again.
    app.save_settings();
    assert!(!app.settings_unsaved());
    assert_eq!(app.settings_revision(), 1);
}

#[test]
fn settings_changes_reach_the_app() {
    let mut app = loaded_demo_app();

    // Each starts the way the app behaved before it could be configured.
    assert!(app.settings().mark_read_on_open);
    assert!(app.settings().confirm_links);
    assert_eq!(app.settings().reading(), Reading::default());
    assert_eq!(
        app.settings().compose_placement,
        ComposePlacement::ReadingPane
    );

    let _ = app.update(Message::SetMarkReadOnOpen(false));
    let _ = app.update(Message::SetConfirmLinks(false));
    let _ = app.update(Message::SetExpandAllMessages(true));
    let _ = app.update(Message::SetShowQuotedText(true));
    let _ = app.update(Message::SetComposePlacement(ComposePlacement::Window));
    let _ = app.update(Message::SetZoom(1.3));

    assert!(!app.settings().mark_read_on_open);
    assert!(!app.settings().confirm_links);
    assert_eq!(app.settings().compose_placement, ComposePlacement::Window);
    assert_eq!(app.settings().zoom, 1.3);
    assert_eq!(
        app.settings().reading(),
        Reading {
            expand_all_messages: true,
            show_quoted_text: true,
        }
    );
}

#[test]
fn asking_for_every_message_reaches_the_open_conversation() {
    // The choice is read where a message is drawn, so it applies to the
    // conversation already open, not only the next one.
    let mut app = loaded_demo_app();
    let _ = app.update(Message::SelectConversation("demo-0".into()));
    deliver_selected_demo_detail(&mut app);

    let _ = app.update(Message::SetExpandAllMessages(true));

    let reading = app.settings().reading();
    let reader = app.mailbox().unwrap().reader().unwrap();
    assert!(reader.detail().messages.len() > 1);
    for message in &reader.detail().messages {
        assert!(
            reader.is_expanded(&message.id, reading),
            "{} stayed folded",
            message.id
        );
    }
}

#[test]
fn a_link_skips_the_prompt_only_when_confirmation_is_off() {
    let mut app = loaded_demo_app();

    let _ = app.update(Message::LinkClicked("https://example.com/a".into()));
    assert!(
        app.pending_link().is_some(),
        "confirmation is on by default"
    );
    let _ = app.update(Message::DismissLink);

    let _ = app.update(Message::SetConfirmLinks(false));
    let _ = app.update(Message::LinkClicked("https://example.com/a".into()));
    assert!(
        app.pending_link().is_none(),
        "the link opens without asking"
    );

    // Refusing what is not a web or mail link does not depend on the
    // setting: it is a rule, not a preference.
    let _ = app.update(Message::LinkClicked("javascript:alert(1)".into()));
    assert!(app.pending_link().is_none());
}

#[test]
fn the_settings_page_covers_the_mailbox_and_steps_back() {
    let mut app = loaded_demo_app();
    assert!(!app.showing_settings());

    let _ = app.update(Message::ShowSettings(true));
    assert!(app.showing_settings());

    // Escape backs out of the page before anything underneath it.
    press(&mut app, Key::Escape);

    assert!(!app.showing_settings());
}

#[test]
fn leaving_settings_keeps_the_message_open_underneath() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::OpenCompose);
    let _ = app.update(Message::ShowSettings(true));

    press(&mut app, Key::Escape);

    assert!(!app.showing_settings());
    assert!(app.compose().is_some());
}

/// Sends a plain key press, as the window would when no field took it.
fn press(app: &mut App, key: Key) {
    let _ = app.update(Message::KeyPressed(KeyPress {
        key,
        command: false,
        other_modifier: false,
    }));
}

#[test]
fn a_save_in_flight_does_not_outlive_the_session() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::SaveAttachment("demo-0".into(), "file".into()));
    assert_eq!(app.saving_attachment(), Some("file"));

    let _ = app.update(Message::Logout);
    assert_eq!(app.saving_attachment(), None);
    assert!(app.saved_attachment().is_none());

    // The fetch was already running and still answers. There is no
    // mailbox left to show it in, and the next one must not open with a
    // banner about a file from the last session.
    let _ = app.update(Message::AttachmentSaved(Ok(PathBuf::from("/tmp/x.pdf"))));
    assert!(app.saved_attachment().is_none());
}

#[test]
fn signing_out_takes_the_account_folders_with_it() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::FoldersLoaded(Ok(vec![
        Folder::custom("kZ9", "Invoices"),
        Folder::label("wN2", "Receipts"),
    ])));
    assert_eq!(app.folders().len(), 2);

    let _ = app.update(Message::Logout);

    // The next sign-in can be a different account, and its sidebar must
    // not open showing names that belong to the last one.
    assert!(app.folders().is_empty());
}

#[test]
fn signing_out_from_the_settings_page_comes_back_to_the_mailbox() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::ShowSettings(true));

    let _ = app.update(Message::Logout);

    // Otherwise signing back in lands on the settings page again, with
    // no sign that it was ever left open.
    assert!(!app.showing_settings());
}

#[test]
fn keys_do_not_reach_a_covered_mailbox() {
    let mut app = loaded_demo_app();
    press(&mut app, Key::Character('j'));
    let opened = app
        .mailbox()
        .unwrap()
        .selected_conversation()
        .map(str::to_owned);
    assert!(opened.is_some());

    let _ = app.update(Message::ShowSettings(true));
    // Stepping the list underneath the settings page would change the
    // selection out of sight, and ask Proton for a conversation nobody
    // can see.
    press(&mut app, Key::Character('j'));
    assert_eq!(
        app.mailbox().unwrap().selected_conversation(),
        opened.as_deref()
    );

    // Escape still backs out of the page, and the list answers again.
    press(&mut app, Key::Escape);
    assert!(!app.showing_settings());
    press(&mut app, Key::Character('j'));
    assert_ne!(
        app.mailbox().unwrap().selected_conversation(),
        opened.as_deref()
    );
}

#[test]
fn keys_do_not_reach_the_mailbox_under_a_link_prompt() {
    let mut app = loaded_demo_app();
    press(&mut app, Key::Character('j'));
    let opened = app
        .mailbox()
        .unwrap()
        .selected_conversation()
        .map(str::to_owned);

    let _ = app.update(Message::LinkClicked("https://example.com/".to_owned()));
    assert!(app.pending_link().is_some());

    press(&mut app, Key::Character('j'));
    assert_eq!(
        app.mailbox().unwrap().selected_conversation(),
        opened.as_deref()
    );

    press(&mut app, Key::Escape);
    assert!(app.pending_link().is_none());
}

#[test]
fn the_keyboard_walks_the_conversation_list() {
    let mut app = loaded_demo_app();

    // Nothing is open, so the first key opens the first conversation.
    press(&mut app, Key::Character('j'));
    let selected = |app: &App| {
        app.mailbox()
            .unwrap()
            .selected_conversation()
            .map(str::to_owned)
    };
    assert_eq!(selected(&app).as_deref(), Some("demo-0"));

    press(&mut app, Key::Character('j'));
    assert_eq!(selected(&app).as_deref(), Some("demo-1"));

    press(&mut app, Key::Character('k'));
    assert_eq!(selected(&app).as_deref(), Some("demo-0"));

    // The list has an end, and Escape closes the reader.
    press(&mut app, Key::Character('k'));
    assert_eq!(selected(&app).as_deref(), Some("demo-0"));

    press(&mut app, Key::Escape);
    assert_eq!(selected(&app), None);
}

#[test]
fn the_account_folders_reach_the_sidebar() {
    let mut app = loaded_demo_app();
    assert!(app.folders().is_empty(), "the demo account has none");

    let invoices = Folder::custom("kZ9", "Invoices");
    let receipts = Folder::label("wN2", "Receipts");
    let _ = app.update(Message::FoldersLoaded(Ok(vec![
        invoices.clone(),
        receipts.clone(),
    ])));

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

    // None of the demo's fictional mail is in a folder the real account
    // made, so opening one would show an empty list under a name the
    // demo sidebar does not even carry.
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
    let _ = app.update(Message::FoldersLoaded(Ok(vec![renamed.clone()])));
    assert_eq!(app.mailbox().unwrap().folder(), &renamed);

    // Once it is gone from the account there is nothing to show under it,
    // so reading falls back to the one folder that is always there.
    let _ = app.update(Message::FoldersLoaded(Ok(Vec::new())));
    assert_eq!(app.mailbox().unwrap().folder(), &Folder::INBOX);
}

#[test]
fn only_web_and_mail_links_are_offered() {
    let web = PendingLink::parse(" https://Example.com/path?x=1 ").unwrap();
    assert_eq!(web.target, "example.com");
    assert_eq!(web.url, "https://example.com/path?x=1");
    assert_eq!(
        PendingLink::parse("mailto:team@example.org")
            .unwrap()
            .target,
        "team@example.org"
    );
    // The real host shows even when the link hides it behind a user name.
    assert_eq!(
        PendingLink::parse("https://bank.example@evil.example/login")
            .unwrap()
            .target,
        "evil.example"
    );

    for rejected in [
        "javascript:alert(1)",
        "file:///etc/passwd",
        "data:text/html,x",
        "/relative",
        "not a link",
    ] {
        assert_eq!(PendingLink::parse(rejected), None, "{rejected}");
    }
}

#[test]
fn clicked_links_wait_for_confirmation_and_clear() {
    let mut app = loaded_demo_app();

    let _ = app.update(Message::LinkClicked("https://example.com/a".into()));
    assert_eq!(
        app.pending_link().map(|link| link.target.as_str()),
        Some("example.com")
    );
    let _ = app.update(Message::DismissLink);
    assert!(app.pending_link().is_none());

    let _ = app.update(Message::LinkClicked("javascript:alert(1)".into()));
    assert!(app.pending_link().is_none());

    let _ = app.update(Message::LinkClicked("https://example.com/a".into()));
    let _ = app.update(Message::OpenLink);
    assert!(app.pending_link().is_none());

    let _ = app.update(Message::LinkClicked("https://example.com/a".into()));
    let _ = app.update(Message::SelectFolder(sys(MailFolder::Sent)));
    assert!(app.pending_link().is_none());
}

#[test]
fn rerender_inputs_issue_no_mailbox_requests() {
    let mut app = loaded_demo_app();
    let requests = app.last_request;
    for message in [
        Message::SearchChanged("alex".into()),
        Message::PanelsResized(Panels {
            sidebar: 0.3,
            ..app.panels()
        }),
        Message::SearchChanged(String::new()),
    ] {
        assert_eq!(app.update(message).units(), 0);
    }

    assert_eq!(app.last_request, requests);
}

fn loaded_demo_app() -> App {
    let (mut app, _) = App::boot(true, Settings::default());
    deliver_demo_page(&mut app, 1, 0);
    let counts = demo_service(&app).counts();
    let _ = app.update(Message::CountsLoaded(2, Ok(counts)));
    app
}

#[test]
fn initial_state_checks_saved_session() {
    let app = App::new(Settings::default());

    assert_eq!(app.auth_state, AuthState::CheckingSession);
}

#[test]
fn normal_startup_checks_the_proton_session() {
    let (app, _) = App::boot(false, Settings::default());

    assert_eq!(app.auth_state, AuthState::CheckingSession);
    assert!(!app.is_demo());
    assert!(app.backend.is_none());
    assert!(app.mailbox.is_none());
}

#[test]
fn demo_startup_opens_mailbox_without_session_resume() {
    let (app, _) = App::boot(true, Settings::default());

    assert!(app.is_demo());
    assert_eq!(app.auth_state, AuthState::Authenticated { email: None });
    let mailbox = app.mailbox().unwrap();
    assert_eq!(mailbox.folder(), &sys(MailFolder::Inbox));
    assert_eq!(mailbox.status(), ListStatus::Loading(1));
}

#[test]
fn demo_mailbox_uses_production_pagination() {
    let (mut app, _) = App::boot(true, Settings::default());
    deliver_demo_page(&mut app, 1, 0);
    let counts = demo_service(&app).counts();
    let _ = app.update(Message::CountsLoaded(2, Ok(counts)));

    let mailbox = app.mailbox().unwrap();
    assert_eq!(mailbox.conversations().len(), 10);
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
fn demo_selection_is_local() {
    let (mut app, _) = App::boot(true, Settings::default());
    deliver_demo_page(&mut app, 1, 0);

    let _ = app.update(Message::SelectConversation("demo-3".into()));

    assert_eq!(
        app.mailbox().unwrap().selected_conversation(),
        Some("demo-3")
    );
}

#[test]
fn selecting_only_queues_a_detail_request() {
    let mut app = loaded_demo_app();
    let status = app.mailbox().unwrap().status();
    let counts = app.mailbox().unwrap().counts().cloned();
    let conversation_ids: Vec<_> = app
        .mailbox()
        .unwrap()
        .conversations()
        .iter()
        .map(|conversation| conversation.id.clone())
        .collect();

    let task = app.update(Message::SelectConversation("demo-3".into()));

    // The detail request and the scroll back to the top of the reader.
    // `last_request` is what proves only one of them reaches the backend.
    assert_eq!(task.units(), 2);
    assert_eq!(app.last_request, 3);
    let mailbox = app.mailbox().unwrap();
    assert_eq!(mailbox.status(), status);
    assert_eq!(mailbox.counts().cloned(), counts);
    assert_eq!(
        mailbox
            .conversations()
            .iter()
            .map(|conversation| conversation.id.clone())
            .collect::<Vec<_>>(),
        conversation_ids
    );
    assert!(matches!(
        mailbox.reader_state(),
        ReaderState::Loading {
            conversation_id,
            request: 3,
        } if conversation_id == "demo-3"
    ));
}

#[test]
fn no_selection_has_no_reader() {
    let (mut app, _) = App::boot(true, Settings::default());
    deliver_demo_page(&mut app, 1, 0);

    assert_eq!(app.mailbox().unwrap().reader_state(), &ReaderState::Empty);
}

#[test]
fn demo_selection_opens_reader_with_newest_message_expanded() {
    let (mut app, _) = App::boot(true, Settings::default());
    deliver_demo_page(&mut app, 1, 0);

    let _ = app.update(Message::SelectConversation("demo-0".into()));
    deliver_selected_demo_detail(&mut app);

    let reader = app.mailbox().unwrap().reader().unwrap();
    assert_eq!(reader.conversation_id(), "demo-0");
    assert_eq!(reader.detail().id, reader.conversation_id());
    assert_eq!(reader.detail().messages.len(), 3);
    assert!(reader.is_expanded("demo-0-2", Reading::default()));
    assert!(!reader.is_expanded("demo-0-0", Reading::default()));
}

#[test]
fn toggling_messages_and_switching_conversations() {
    let (mut app, _) = App::boot(true, Settings::default());
    deliver_demo_page(&mut app, 1, 0);
    let _ = app.update(Message::SelectConversation("demo-0".into()));
    deliver_selected_demo_detail(&mut app);

    let _ = app.update(Message::ToggleMessageExpanded("demo-0-0".into()));
    let reader = app.mailbox().unwrap().reader().unwrap();
    assert!(reader.is_expanded("demo-0-0", Reading::default()));
    assert!(reader.is_expanded("demo-0-2", Reading::default()));

    let _ = app.update(Message::SelectConversation("demo-2".into()));
    deliver_selected_demo_detail(&mut app);
    let _ = app.update(Message::SelectConversation("demo-0".into()));
    deliver_selected_demo_detail(&mut app);
    assert!(
        !app.mailbox()
            .unwrap()
            .reader()
            .unwrap()
            .is_expanded("demo-0-0", Reading::default())
    );
}

#[test]
fn folder_switch_closes_reader() {
    let (mut app, _) = App::boot(true, Settings::default());
    deliver_demo_page(&mut app, 1, 0);
    let _ = app.update(Message::SelectConversation("demo-0".into()));

    let _ = app.update(Message::SelectFolder(sys(MailFolder::Sent)));

    assert!(app.mailbox().unwrap().reader().is_none());
}

#[test]
fn detail_failure_and_retry_use_a_new_request() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::SelectConversation("demo-3".into()));
    let first = pending_reader_request(&app);
    let _ = app.update(Message::ConversationLoaded(
        first.clone(),
        Err(MailboxError::Connection),
    ));
    assert!(matches!(
        app.mailbox().unwrap().reader_state(),
        ReaderState::Failed {
            conversation_id,
            error: MailboxError::Connection,
        } if conversation_id == "demo-3"
    ));

    let task = app.update(Message::RetryConversation);
    let retry = pending_reader_request(&app);

    assert_eq!(task.units(), 1);
    assert!(retry.id > first.id);
    assert_eq!(retry.conversation_id, first.conversation_id);
}

#[test]
fn demo_read_actions_update_row_and_count_idempotently() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::SearchChanged("design review".into()));
    let _ = app.update(Message::SelectConversation("demo-0".into()));
    deliver_selected_demo_detail(&mut app);
    let detail = app.mailbox().unwrap().reader().unwrap().detail().clone();

    assert!(!app.mailbox().unwrap().conversations()[0].unread);
    assert_eq!(
        app.mailbox()
            .unwrap()
            .counts()
            .unwrap()
            .unread(&sys(MailFolder::Inbox)),
        Some(5)
    );

    let _ = app.update(Message::ApplyAction(MailAction::SetUnread(true)));
    let _ = app.update(Message::ApplyAction(MailAction::SetUnread(true)));
    assert_eq!(app.mailbox().unwrap().search_query(), "design review");
    assert_eq!(app.mailbox().unwrap().visible_conversations().count(), 1);
    assert!(app.mailbox().unwrap().conversations()[0].unread);
    assert_eq!(
        app.mailbox()
            .unwrap()
            .counts()
            .unwrap()
            .unread(&sys(MailFolder::Inbox)),
        Some(6)
    );

    let _ = app.update(Message::ApplyAction(MailAction::SetUnread(false)));
    let _ = app.update(Message::ApplyAction(MailAction::SetUnread(false)));
    let _ = app.update(Message::ApplyAction(MailAction::SetStarred(true)));
    assert_eq!(app.mailbox().unwrap().reader().unwrap().detail(), &detail);
    assert!(!app.mailbox().unwrap().conversations()[0].unread);
    assert_eq!(
        app.mailbox()
            .unwrap()
            .counts()
            .unwrap()
            .unread(&sys(MailFolder::Inbox)),
        Some(5)
    );
}

#[test]
fn demo_star_actions_update_starred_folder_and_selection() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::SelectConversation("demo-1".into()));
    deliver_selected_demo_detail(&mut app);
    let _ = app.update(Message::ApplyAction(MailAction::SetStarred(true)));
    let _ = app.update(Message::ApplyAction(MailAction::SetStarred(true)));
    assert!(app.mailbox().unwrap().conversations()[1].starred);

    let _ = app.update(Message::SelectFolder(sys(MailFolder::Starred)));
    deliver_latest_demo_page(&mut app, 0);
    assert!(
        app.mailbox()
            .unwrap()
            .conversations()
            .iter()
            .any(|conversation| conversation.id == "demo-1")
    );

    let _ = app.update(Message::SelectConversation("demo-1".into()));
    deliver_selected_demo_detail(&mut app);
    let _ = app.update(Message::ApplyAction(MailAction::SetStarred(false)));
    let mailbox = app.mailbox().unwrap();
    assert!(
        mailbox
            .conversations()
            .iter()
            .all(|conversation| conversation.id != "demo-1")
    );
    assert!(
        mailbox
            .selected_conversation()
            .is_some_and(|id| id != "demo-1")
    );
}

#[test]
fn starred_search_reflects_unstar_immediately() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::SelectFolder(sys(MailFolder::Starred)));
    deliver_latest_demo_page(&mut app, 0);
    let _ = app.update(Message::SearchChanged("offsite".into()));
    let _ = app.update(Message::SelectConversation("demo-2".into()));

    let _ = app.update(Message::ApplyAction(MailAction::SetStarred(false)));

    let mailbox = app.mailbox().unwrap();
    assert_eq!(mailbox.search_query(), "offsite");
    assert_eq!(mailbox.visible_conversations().count(), 0);
    assert_eq!(mailbox.selected_conversation(), None);
}

#[test]
fn one_attachment_is_saved_at_a_time() {
    let mut app = loaded_demo_app();

    // The demo carries no files, so the fetch itself fails; what matters
    // is that the second press never starts a fetch of its own, which is
    // how the same file ended up written twice under two names.
    let _ = app.update(Message::SaveAttachment("demo-0".into(), "file".into()));
    assert_eq!(app.saving_attachment(), Some("file"));

    let _ = app.update(Message::SaveAttachment("demo-0".into(), "other".into()));
    assert_eq!(app.saving_attachment(), Some("file"));

    let _ = app.update(Message::AttachmentSaved(Err(SaveError::NotFetched)));
    assert_eq!(app.saving_attachment(), None);
    // Once the first one is done, the next press is free to go.
    let _ = app.update(Message::SaveAttachment("demo-0".into(), "other".into()));
    assert_eq!(app.saving_attachment(), Some("other"));
}

#[test]
fn a_conversation_the_server_found_can_still_be_acted_on() {
    let mut app = loaded_demo_app();
    deliver_demo_search(&mut app, "offsite");
    let found = app
        .mailbox()
        .unwrap()
        .visible_conversations()
        .next()
        .expect("the search found something")
        .id
        .clone();
    let _ = app.update(Message::SelectConversation(found.clone()));
    deliver_selected_demo_detail(&mut app);

    // The row is not in the open folder's listing, which used to leave
    // the reader with no actions at all.
    assert_eq!(
        app.mailbox().unwrap().selected_summary().map(|row| &row.id),
        Some(&found)
    );

    let _ = app.update(Message::ApplyAction(MailAction::SetStarred(true)));

    // The results are what is on screen, so they carry the change.
    let mailbox = app.mailbox().unwrap();
    assert_eq!(mailbox.search_results(), Some("offsite"));
    assert!(
        mailbox
            .visible_conversations()
            .find(|row| row.id == found)
            .expect("the row is still listed")
            .starred
    );
}

#[test]
fn archiving_from_the_results_takes_the_row_out_of_them() {
    let mut app = loaded_demo_app();
    deliver_demo_search(&mut app, "the");
    let rows: Vec<String> = app
        .mailbox()
        .unwrap()
        .visible_conversations()
        .map(|row| row.id.clone())
        .collect();
    assert!(rows.len() > 1, "the search found more than one row");
    let _ = app.update(Message::SelectConversation(rows[0].clone()));
    deliver_selected_demo_detail(&mut app);

    let _ = app.update(Message::ApplyAction(MailAction::MoveTo(sys(
        MailFolder::Archive,
    ))));

    // The demo has no server to re-read, so nothing else would have taken
    // the row out of the results, and archiving would look like a no-op.
    let mailbox = app.mailbox().unwrap();
    assert!(
        !mailbox.visible_conversations().any(|row| row.id == rows[0]),
        "the archived row is still listed among the results"
    );
    assert_eq!(mailbox.selected_conversation(), Some(rows[1].as_str()));
}

#[test]
fn demo_archive_removes_selected_and_opens_next() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::SelectConversation("demo-0".into()));
    deliver_selected_demo_detail(&mut app);

    let _ = app.update(Message::ApplyAction(MailAction::MoveTo(sys(
        MailFolder::Archive,
    ))));

    let mailbox = app.mailbox().unwrap();
    assert!(
        mailbox
            .conversations()
            .iter()
            .all(|conversation| conversation.id != "demo-0")
    );
    assert_eq!(mailbox.selected_conversation(), Some("demo-1"));

    let _ = app.update(Message::SelectFolder(sys(MailFolder::Archive)));
    deliver_latest_demo_page(&mut app, 0);
    assert!(
        app.mailbox()
            .unwrap()
            .conversations()
            .iter()
            .any(|conversation| conversation.id == "demo-0")
    );
}

#[test]
fn moving_selected_conversation_invalidates_its_pending_detail() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::SelectConversation("demo-0".into()));
    let stale = pending_reader_request(&app);

    let task = app.update(Message::ApplyAction(MailAction::MoveTo(sys(
        MailFolder::Archive,
    ))));
    let current = pending_reader_request(&app);
    let stale_detail = demo_service(&app).conversation_detail("demo-0", NOW);
    let _ = app.update(Message::ConversationLoaded(stale, stale_detail));

    // Opening the next conversation: its detail request and the scroll
    // back to the top of the reader.
    assert_eq!(task.units(), 2);
    assert_eq!(
        app.mailbox().unwrap().selected_conversation(),
        Some("demo-1")
    );
    assert_eq!(pending_reader_request(&app), current);
}

#[test]
fn undoing_a_demo_move_puts_the_conversation_back() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::SelectConversation("demo-3".into()));
    deliver_selected_demo_detail(&mut app);

    let _ = app.update(Message::ApplyAction(MailAction::MoveTo(sys(
        MailFolder::Archive,
    ))));
    let has_row = |app: &App| {
        app.mailbox()
            .unwrap()
            .conversations()
            .iter()
            .any(|conversation| conversation.id == "demo-3")
    };
    assert!(!has_row(&app));
    assert!(app.mailbox().unwrap().undo().is_some());

    let _ = app.update(Message::UndoMove);

    assert!(has_row(&app));
    assert!(app.mailbox().unwrap().undo().is_none());
}

#[test]
fn demo_trash_and_spam_moves_update_folders() {
    for (message, folder) in [
        (
            Message::ApplyAction(MailAction::MoveTo(sys(MailFolder::Trash))),
            sys(MailFolder::Trash),
        ),
        (
            Message::ApplyAction(MailAction::MoveTo(sys(MailFolder::Spam))),
            sys(MailFolder::Spam),
        ),
    ] {
        let mut app = loaded_demo_app();
        let _ = app.update(Message::SelectConversation("demo-3".into()));
        deliver_selected_demo_detail(&mut app);
        let _ = app.update(message);
        assert!(
            app.mailbox()
                .unwrap()
                .conversations()
                .iter()
                .all(|conversation| conversation.id != "demo-3")
        );

        let _ = app.update(Message::SelectFolder(folder.clone()));
        deliver_latest_demo_page(&mut app, 0);
        assert!(
            app.mailbox()
                .unwrap()
                .conversations()
                .iter()
                .any(|conversation| conversation.id == "demo-3")
        );
    }
}

#[test]
fn inbox_search_reflects_folder_moves_immediately() {
    for message in [
        Message::ApplyAction(MailAction::MoveTo(sys(MailFolder::Archive))),
        Message::ApplyAction(MailAction::MoveTo(sys(MailFolder::Trash))),
        Message::ApplyAction(MailAction::MoveTo(sys(MailFolder::Spam))),
    ] {
        let mut app = loaded_demo_app();
        let _ = app.update(Message::SearchChanged("blue notebook".into()));
        assert_eq!(app.mailbox().unwrap().visible_conversations().count(), 1);
        let _ = app.update(Message::SelectConversation("demo-3".into()));
        deliver_selected_demo_detail(&mut app);

        let _ = app.update(message);

        let mailbox = app.mailbox().unwrap();
        assert_eq!(mailbox.search_query(), "blue notebook");
        assert_eq!(mailbox.visible_conversations().count(), 0);
        assert_eq!(mailbox.selected_conversation(), None);
    }
}

#[test]
fn moving_only_conversation_closes_reader_cleanly() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::SelectConversation("demo-3".into()));
    let _ = app.update(Message::ApplyAction(MailAction::MoveTo(sys(
        MailFolder::Trash,
    ))));
    let _ = app.update(Message::SelectFolder(sys(MailFolder::Trash)));
    deliver_latest_demo_page(&mut app, 0);
    let _ = app.update(Message::SelectConversation("demo-3".into()));

    let _ = app.update(Message::ApplyAction(MailAction::MoveTo(sys(
        MailFolder::Archive,
    ))));

    let mailbox = app.mailbox().unwrap();
    assert!(mailbox.conversations().is_empty());
    assert!(mailbox.reader().is_none());
    assert_eq!(mailbox.selected_conversation(), None);
}

#[test]
fn resizing_panels_only_changes_layout() {
    let (mut app, _) = App::boot(true, Settings::default());
    deliver_demo_page(&mut app, 1, 0);
    let _ = app.update(Message::SelectConversation("demo-0".into()));
    deliver_selected_demo_detail(&mut app);
    let _ = app.update(Message::ToggleMessageExpanded("demo-0-0".into()));
    let requests = app.last_request;
    let default_panels = app.panels();
    let task = app.update(Message::PanelsResized(Panels {
        sidebar: 0.35,
        conversations: 0.6,
    }));
    assert_eq!(task.units(), 0);

    assert_ne!(app.panels(), default_panels);
    assert_eq!(app.last_request, requests);
    assert!(app.is_demo());
    let mailbox = app.mailbox().unwrap();
    assert_eq!(mailbox.folder(), &sys(MailFolder::Inbox));
    assert_eq!(mailbox.status(), ListStatus::Loaded);
    assert_eq!(mailbox.conversations().len(), 10);
    let reader = mailbox.reader().unwrap();
    assert_eq!(reader.conversation_id(), "demo-0");
    assert!(reader.is_expanded("demo-0-0", Reading::default()));
}

#[test]
fn exiting_demo_clears_mailbox_and_opens_login() {
    let (mut app, _) = App::boot(true, Settings::default());
    deliver_demo_page(&mut app, 1, 0);

    let _ = app.update(Message::Logout);

    assert_eq!(app.auth_state, AuthState::SignedOut);
    assert_eq!(app.auth_error, None);
    assert!(app.backend.is_none());
    assert!(app.mailbox.is_none());
    assert!(!app.is_demo());
}

#[test]
fn absent_saved_session_opens_login() {
    let mut app = App::new(Settings::default());

    let _ = app.update(Message::SessionChecked(ResumeOutcome::SignedOut));

    assert_eq!(app.auth_state, AuthState::SignedOut);
    assert_eq!(app.auth_error, None);
}

fn signing_in_app() -> App {
    let mut app = App::new(Settings::default());
    app.auth_state = AuthState::SigningIn(SignInStep::Credentials);
    app.login_form.username = "username sentinel".into();
    app.login_form.password = "password sentinel".into();
    app
}

#[test]
fn totp_prompt_is_answered_within_the_same_sign_in() {
    let mut app = signing_in_app();
    let (reply, mut answer) = crate::mail::Reply::channel();

    let _ = app.update(Message::SignInPrompt(SignInPrompt::Totp(reply)));
    assert_eq!(app.auth_state, AuthState::NeedsTotp);
    assert_eq!(app.login_form.username, "username sentinel");
    assert_eq!(app.login_form.password, "password sentinel");

    let _ = app.update(Message::Submit);
    assert_eq!(app.auth_error, Some(AuthError::TotpRequired));
    assert_eq!(answer.try_recv(), Ok(None));

    let _ = app.update(Message::TotpChanged(" 123456 ".into()));
    let _ = app.update(Message::Submit);
    assert_eq!(answer.try_recv(), Ok(Some("123456".to_owned())));
    assert_eq!(app.auth_state, AuthState::SigningIn(SignInStep::Totp));
    assert!(app.login_form.totp.is_empty());
}

#[test]
fn human_verification_waits_for_confirmation() {
    let mut app = signing_in_app();
    let (done, mut answer) = crate::mail::Reply::channel();
    let url = "https://verify.proton.me/?methods=captcha&token=t".to_owned();

    let _ = app.update(Message::SignInPrompt(SignInPrompt::HumanVerification {
        url: url.clone(),
        done,
    }));
    assert_eq!(app.auth_state, AuthState::NeedsHumanVerification { url });
    assert_eq!(answer.try_recv(), Ok(None));

    let _ = app.update(Message::Submit);
    assert_eq!(answer.try_recv(), Ok(Some(())));
    assert!(matches!(app.auth_state, AuthState::SigningIn(_)));
}

#[test]
fn going_back_cancels_the_pending_prompt() {
    let mut app = signing_in_app();
    let (reply, mut answer) = crate::mail::Reply::<String>::channel();
    let _ = app.update(Message::SignInPrompt(SignInPrompt::Totp(reply)));

    let _ = app.update(Message::CancelChallenge);
    assert_eq!(app.auth_state, AuthState::SignedOut);
    assert!(answer.try_recv().is_err());

    let _ = app.update(Message::SignInFinished(SignInOutcome::Cancelled));
    assert_eq!(app.auth_state, AuthState::SignedOut);
    assert_eq!(app.auth_error, None);
}

#[test]
fn prompts_without_a_running_sign_in_are_cancelled() {
    let mut app = App::new(Settings::default());
    app.auth_state = AuthState::SignedOut;
    let (reply, mut answer) = crate::mail::Reply::<String>::channel();

    let _ = app.update(Message::SignInPrompt(SignInPrompt::Totp(reply)));

    assert_eq!(app.auth_state, AuthState::SignedOut);
    assert!(answer.try_recv().is_err());
}

#[test]
fn terminal_sign_in_failure_clears_secrets() {
    let mut app = App::new(Settings::default());
    app.login_form.password = "password sentinel".into();
    app.login_form.totp = "totp sentinel".into();
    app.login_form.mailbox_password = "mailbox password sentinel".into();

    let _ = app.update(Message::SignInFinished(SignInOutcome::Failed(
        AuthError::Connection,
    )));

    assert_eq!(app.auth_state, AuthState::SignedOut);
    assert!(app.login_form.password.is_empty());
    assert!(app.login_form.totp.is_empty());
    assert!(app.login_form.mailbox_password.is_empty());
}

#[test]
fn failed_logout_restores_authenticated_shell() {
    let mut app = App::new(Settings::default());
    app.auth_state = AuthState::SigningOut;

    let _ = app.update(Message::LogoutFinished(Err(AuthError::SessionUnavailable)));

    assert_eq!(app.auth_state, AuthState::Authenticated { email: None });
    assert_eq!(app.auth_error, Some(AuthError::SessionUnavailable));
}

#[test]
fn successful_logout_opens_login_and_clears_mailbox() {
    let mut app = authenticated_app();
    app.auth_state = AuthState::SigningOut;

    let _ = app.update(Message::LogoutFinished(Ok(())));

    assert_eq!(app.auth_state, AuthState::SignedOut);
    assert!(app.mailbox.is_none());
}

#[test]
fn expired_session_signs_out_and_clears_mailbox() {
    let mut app = authenticated_app();

    let _ = app.update(Message::ConversationsLoaded(
        1,
        Err(MailboxError::SessionExpired),
    ));

    assert_eq!(app.auth_state, AuthState::SignedOut);
    assert_eq!(app.auth_error, Some(AuthError::SessionExpired));
    assert!(app.mailbox.is_none());
}

#[test]
fn stale_expired_session_response_is_ignored() {
    let mut app = authenticated_app();

    let _ = app.update(Message::ConversationsLoaded(
        99,
        Err(MailboxError::SessionExpired),
    ));

    assert_eq!(app.auth_state, AuthState::Authenticated { email: None });
    assert!(app.mailbox.is_some());
}

#[test]
fn logout_result_after_session_expiry_is_ignored() {
    let mut app = authenticated_app();
    app.auth_state = AuthState::SigningOut;
    let _ = app.update(Message::CountsLoaded(2, Err(MailboxError::SessionExpired)));

    let _ = app.update(Message::LogoutFinished(Err(AuthError::Connection)));

    assert_eq!(app.auth_state, AuthState::SignedOut);
    assert_eq!(app.auth_error, Some(AuthError::SessionExpired));
}

#[test]
fn mailbox_actions_are_ignored_while_signing_out() {
    let mut app = authenticated_app();
    app.auth_state = AuthState::SigningOut;

    let _ = app.update(Message::SelectFolder(sys(MailFolder::Trash)));

    assert_eq!(app.mailbox().unwrap().folder(), &sys(MailFolder::Inbox));
}

/// Opens a message and fills it in.
fn write(app: &mut App, to: &str) {
    let _ = app.update(Message::OpenCompose);
    let _ = app.update(Message::ComposeChanged(ComposeField::To, to.to_owned()));
    let _ = app.update(Message::ComposeChanged(
        ComposeField::Subject,
        "Thursday".to_owned(),
    ));
    let _ = app.update(Message::ComposeChanged(
        ComposeField::Body,
        "See you then.".to_owned(),
    ));
}

#[test]
fn a_message_is_written_sent_and_put_away() {
    let mut app = loaded_demo_app();
    write(&mut app, "alex@example.com, sam@example.org");

    // Handing it over is work, and the window says so meanwhile.
    let task = app.update(Message::Send);
    assert_eq!(task.units(), 1);
    assert_eq!(app.compose().map(Compose::sending), Some(Sending::InFlight));

    let _ = app.update(Message::Sent(Ok(())));
    assert!(
        app.compose().is_none(),
        "the window stays open after sending"
    );
}

#[test]
fn a_message_with_nowhere_to_go_does_not_leave() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::OpenCompose);
    let _ = app.update(Message::ComposeChanged(
        ComposeField::Body,
        "Ready to go".to_owned(),
    ));

    // Pressing Send is not an instruction to guess.
    assert_eq!(app.update(Message::Send).units(), 0);
    assert_eq!(app.compose().map(Compose::sending), Some(Sending::Writing));
}

#[test]
fn a_refusal_keeps_the_message_and_says_why() {
    let mut app = loaded_demo_app();
    write(&mut app, "alex@example.com");
    let _ = app.update(Message::Send);

    let _ = app.update(Message::Sent(Err(SendError::DemoLimitReached)));

    let writing = app.compose().expect("the message is still here");
    assert_eq!(
        writing.sending(),
        Sending::Failed(SendError::DemoLimitReached)
    );
    // Nothing typed is lost, so it can be sent again once the cause is gone.
    assert_eq!(writing.field(ComposeField::Body), "See you then.");
}

#[test]
fn a_message_already_gone_cannot_be_sent_twice_or_dismissed() {
    // Once it is out of the sender's hands, the window is a report, not a
    // form: a second press must not put a second copy on the wire.
    let mut app = loaded_demo_app();
    write(&mut app, "alex@example.com");
    let _ = app.update(Message::Send);

    assert_eq!(app.update(Message::Send).units(), 0);
    let _ = app.update(Message::CloseCompose);
    assert_eq!(app.compose().map(Compose::sending), Some(Sending::InFlight));
    press(&mut app, Key::Escape);
    assert!(app.compose().is_some(), "escape dismissed a sent message");
}

#[test]
fn escape_puts_an_unsent_message_away() {
    let mut app = loaded_demo_app();
    write(&mut app, "alex@example.com");

    press(&mut app, Key::Escape);

    assert!(app.compose().is_none());
}

#[test]
fn a_reply_needs_nobody_named_because_proton_knows() {
    // Proton addresses a reply from the message being answered, so the window
    // must not refuse to send for want of a recipient.
    let mut app = loaded_demo_app();
    let _ = app.update(Message::SelectConversation("demo-0".into()));
    deliver_selected_demo_detail(&mut app);

    let _ = app.update(Message::Answer {
        message_id: "demo-0-2".into(),
        forward: false,
        everyone: false,
    });

    let writing = app.compose().expect("a reply is open");
    assert!(!writing.asks_for_recipients());
    assert!(!writing.asks_for_subject());
    assert_eq!(app.update(Message::Send).units(), 1);
}

#[test]
fn a_forward_still_has_to_be_addressed() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::SelectConversation("demo-0".into()));
    deliver_selected_demo_detail(&mut app);

    let _ = app.update(Message::Answer {
        message_id: "demo-0-2".into(),
        forward: true,
        everyone: false,
    });

    let writing = app.compose().expect("a forward is open");
    assert!(writing.asks_for_recipients());
    // Proton writes the subject, so the window does not ask for one.
    assert!(!writing.asks_for_subject());
    assert_eq!(app.update(Message::Send).units(), 0, "it left unaddressed");

    let _ = app.update(Message::ComposeChanged(
        ComposeField::To,
        "alex@example.com".into(),
    ));
    assert_eq!(app.update(Message::Send).units(), 1);
}

#[test]
fn answering_says_what_is_being_answered() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::SelectConversation("demo-0".into()));
    deliver_selected_demo_detail(&mut app);

    let _ = app.update(Message::Answer {
        message_id: "demo-0-2".into(),
        forward: false,
        everyone: true,
    });

    let answering = app
        .compose()
        .and_then(Compose::answering)
        .expect("it says what it answers");
    assert!(answering.everyone);
    assert!(!answering.subject.is_empty());
    assert!(!answering.sender.is_empty());
}

#[test]
fn answering_a_message_that_is_not_there_opens_nothing() {
    let mut app = loaded_demo_app();

    let _ = app.update(Message::Answer {
        message_id: "demo-0-2".into(),
        forward: false,
        everyone: false,
    });

    assert!(app.compose().is_none());
}
