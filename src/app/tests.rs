//! End-to-end state-machine tests through `App::update`.

use super::*;
use crate::mail::demo;
use crate::mail::{INSPECTION_CONCURRENCY, SendError};
use crate::settings::{ComposePlacement, Reading};
use secrecy::{ExposeSecret, SecretString};

use crate::mail::MailFolder;

/// One of Proton's own folders, as a place to list from.
fn sys(folder: MailFolder) -> Folder {
    Folder::System(folder)
}

const NOW: i64 = 1_789_000_000;

/// What a refresh asks for: the open folder's page and its counts.
const PAGE_AND_COUNTS: usize = 2;

/// The requests a mailbox opens with, at boot and in `authenticated_app`:
/// its first page, then its counts.
const OPENING_PAGE: RequestId = 1;
const OPENING_COUNTS: RequestId = 2;
/// A request ID no test issues: a reply to it answers nothing pending.
const NEVER_ISSUED: RequestId = 99;

fn authenticated_app() -> App {
    let mut app = App::new(Settings::default());
    app.auth_state = AuthState::Authenticated { email: None };
    app.mailbox = Some(Mailbox::open(Folder::INBOX, 50, OPENING_PAGE, OPENING_COUNTS).0);
    app.last_request = OPENING_COUNTS;
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
            messages: 1,
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
        page: 0,
        page_size: demo::PAGE_SIZE,
    };
    let result = demo_service(app).search(query, demo::PAGE_SIZE, NOW);
    let _ = app.update(Message::SearchLoaded(request, result));
}

fn deliver_folders(app: &mut App, result: Result<Vec<Folder>, MailboxError>) {
    let epoch = app.session_epoch;
    let _ = app.update(Message::FoldersLoaded(epoch, result));
}

fn deliver_attachment(app: &mut App, result: Result<PathBuf, SaveError>) {
    let request = app.saving_attachment.as_ref().unwrap().request.clone();
    let _ = app.update(Message::AttachmentSaved(request, result));
}

fn deliver_logout(app: &mut App, result: Result<(), AuthError>) {
    let epoch = app.session_epoch;
    let _ = app.update(Message::LogoutFinished(epoch, result));
}

fn deliver_send(app: &mut App, result: Result<(), SendError>) {
    let epoch = app.session_epoch;
    let _ = app.update(Message::Sent(epoch, result));
}

#[test]
fn only_a_real_change_counts_as_one() {
    // Repeated drag values must not advance the revision.
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

/// Runs a settings write as the runtime does, off the interface's thread,
/// and hands the app its result.
async fn finish_settings_write(app: &mut App, write: Effects) {
    for effect in write.into_iter() {
        let Effect::Future(future) = effect else {
            panic!("a settings write runs as a future");
        };
        let _ = app.update(future.await);
    }
}

#[test]
fn a_failed_settings_write_is_tried_again_with_the_next_change_or_on_quitting() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::SetConfirmLinks(false));
    assert_eq!(app.save_settings().units(), 1);
    let _ = app.update(Message::SettingsWritten(app.settings_revision(), false));
    assert!(app.settings_unsaved(), "nothing reached the file");

    // Not again on every frame...
    assert_eq!(app.save_settings().units(), 0);
    // ...but with the next change,
    let _ = app.update(Message::SetConfirmLinks(true));
    assert_eq!(app.save_settings().units(), 1);
    let _ = app.update(Message::SettingsWritten(app.settings_revision(), false));
    // and on quitting, the last chance, which writes here and now.
    app.save_settings_before_quitting();
    assert!(!app.settings_unsaved());
}

#[tokio::test]
async fn a_change_is_written_off_the_interface_once() {
    let mut app = loaded_demo_app();

    let _ = app.update(Message::SetConfirmLinks(false));
    // Keeping it applies at once, whether or not it has reached the file.
    assert!(!app.settings().confirm_links);
    assert!(app.settings_unsaved());

    let write = app.save_settings();
    assert_eq!(write.units(), 1);
    // One write at a time: asking again while it runs starts nothing.
    assert_eq!(app.save_settings().units(), 0);
    finish_settings_write(&mut app, write).await;
    assert!(!app.settings_unsaved());

    // Nothing further to write until something changes again.
    assert_eq!(app.save_settings().units(), 0);
    assert_eq!(app.settings_revision(), 1);
}

#[tokio::test]
async fn a_change_made_while_a_write_runs_is_written_after_it() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::SetConfirmLinks(false));
    let first = app.save_settings();

    let _ = app.update(Message::SetConfirmLinks(true));
    finish_settings_write(&mut app, first).await;
    assert!(
        app.settings_unsaved(),
        "the second change is still to write"
    );

    let second = app.save_settings();
    assert_eq!(second.units(), 1);
    finish_settings_write(&mut app, second).await;
    assert!(!app.settings_unsaved());
}

#[tokio::test]
async fn quitting_while_a_write_runs_keeps_the_newest_revision_counted() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::SetConfirmLinks(false));
    let first = app.save_settings();
    let _ = app.update(Message::SetConfirmLinks(true));

    app.save_settings_before_quitting();
    assert!(!app.settings_unsaved());
    // The first write ends after quitting wrote the newer revision.
    finish_settings_write(&mut app, first).await;
    assert!(!app.settings_unsaved(), "the late write undid the count");
}

#[test]
fn a_settings_write_that_ends_late_never_undoes_a_newer_one() {
    let on_disk = Mutex::new(0);
    let mut written = Vec::new();

    // Quitting writes revision 2 while the write of revision 1 waits.
    assert!(super::write_settings_revision(&on_disk, 2, || {
        written.push(2);
        true
    }));
    // Revision 1 then finds the file newer and leaves it, reporting it done.
    assert!(super::write_settings_revision(&on_disk, 1, || {
        written.push(1);
        true
    }));
    assert_eq!(written, [2]);

    // A failed write leaves the file's revision where it was.
    assert!(!super::write_settings_revision(&on_disk, 3, || false));
    assert!(super::write_settings_revision(&on_disk, 3, || {
        written.push(3);
        true
    }));
    assert_eq!(written, [2, 3]);
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
    let _ = app.update(Message::SetAppearance(Appearance::Light));

    assert!(!app.settings().mark_read_on_open);
    assert!(!app.settings().confirm_links);
    assert_eq!(app.settings().compose_placement, ComposePlacement::Window);
    assert_eq!(app.settings().zoom, 1.3);
    assert_eq!(app.settings().appearance, Appearance::Light);
    // Following the system keeps the scheme chosen; choosing one stops it.
    let _ = app.update(Message::SetFollowSystemAppearance(true));
    assert!(app.settings().follow_system_appearance);
    assert_eq!(app.settings().appearance, Appearance::Light);
    let _ = app.update(Message::SetAppearance(Appearance::Light));
    assert!(!app.settings().follow_system_appearance);
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
    open_conversation(&mut app, "demo-0");

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
fn the_settings_pane_opens_and_steps_back() {
    let mut app = loaded_demo_app();
    assert!(!app.showing_settings());

    let _ = app.update(Message::ShowSettings(true));
    assert!(app.showing_settings());

    // Escape closes the settings pane first.
    press(&mut app, Key::Escape);

    assert!(!app.showing_settings());
}

#[test]
fn closing_settings_keeps_the_message_open() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::OpenCompose);
    let _ = app.update(Message::ShowSettings(true));

    press(&mut app, Key::Escape);

    assert!(!app.showing_settings());
    assert!(app.compose().is_some());
}

#[test]
fn escape_key_closes_untouched_compose_without_prompt() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::OpenCompose);
    assert!(app.compose().is_some());

    press(&mut app, Key::Escape);
    assert!(app.compose().is_none());
}

#[test]
fn escape_key_prompts_to_discard_when_message_has_content() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::OpenCompose);
    let _ = app.update(Message::ComposeChanged(
        ComposeField::Body,
        "Drafting...".into(),
    ));

    // First escape asks for confirmation instead of discarding
    press(&mut app, Key::Escape);
    assert!(app.compose().is_some());
    assert!(app.compose().unwrap().confirming_discard());

    // Second escape cancels confirmation and keeps the draft
    press(&mut app, Key::Escape);
    assert!(app.compose().is_some());
    assert!(!app.compose().unwrap().confirming_discard());

    // Discard button dispatches DiscardCompose
    let _ = app.update(Message::CloseCompose);
    assert!(app.compose().unwrap().confirming_discard());
    let _ = app.update(Message::DiscardCompose);
    assert!(app.compose().is_none());
}

#[test]
fn typing_clears_discard_prompt() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::OpenCompose);
    let _ = app.update(Message::ComposeChanged(
        ComposeField::Body,
        "Drafting...".into(),
    ));
    let _ = app.update(Message::CloseCompose);
    assert!(app.compose().unwrap().confirming_discard());

    let _ = app.update(Message::ComposeChanged(
        ComposeField::Body,
        "Drafting more...".into(),
    ));
    assert!(!app.compose().unwrap().confirming_discard());
}

#[test]
fn picking_compose_attachments_emits_ui_effect() {
    let mut app = loaded_demo_app();
    // When compose is closed, picking does nothing
    let effects = app.update(Message::PickComposeAttachments);
    assert_eq!(effects.units(), 0);

    let _ = app.update(Message::OpenCompose);
    let compose_id = app.compose().unwrap().id();
    let effects = app.update(Message::PickComposeAttachments);
    assert_eq!(effects.units(), 1);
    let effect = effects.into_iter().next().unwrap();
    assert!(matches!(
        effect,
        Effect::Ui(UiEffect::PickComposeAttachments(id)) if id == compose_id
    ));
}

#[test]
fn compose_attachments_can_be_added_and_removed_through_messages() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::OpenCompose);
    let compose_id = app.compose().unwrap().id();

    let file1 = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let file2 = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("README.md");

    let _ = app.update(Message::AddComposeAttachments(
        compose_id,
        vec![file1.clone(), file2.clone()],
    ));
    let compose = app.compose().unwrap();
    assert_eq!(compose.attachments(), &[file1, file2.clone()]);
    assert!(!compose.is_untouched());

    let _ = app.update(Message::RemoveComposeAttachment(0));
    let compose = app.compose().unwrap();
    assert_eq!(compose.attachments(), &[file2]);
}

#[tokio::test]
async fn attached_files_are_measured_away_from_the_interface() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::OpenCompose);
    let draft = app.compose().unwrap().id();
    let file = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let length = std::fs::metadata(&file).unwrap().len();

    let measure = app.update(Message::AddComposeAttachments(draft, vec![file.clone()]));
    assert_eq!(app.compose().unwrap().attachment_size(&file), None);
    for effect in measure.into_iter() {
        let Effect::Future(future) = effect else {
            panic!("the sizes are read by a future");
        };
        let _ = app.update(future.await);
    }
    assert_eq!(app.compose().unwrap().attachment_size(&file), Some(length));
    // A file taken off forgets its size, which may differ when it comes back.
    let _ = app.update(Message::RemoveComposeAttachment(0));
    assert_eq!(app.compose().unwrap().attachment_size(&file), None);

    // Sizes read for a draft that is gone change nothing in the next one.
    let _ = app.update(Message::DiscardCompose);
    let _ = app.update(Message::OpenCompose);
    let _ = app.update(Message::AddComposeAttachments(
        app.compose().unwrap().id(),
        vec![file.clone()],
    ));
    let _ = app.update(Message::AttachmentSizesRead(
        draft,
        [(file.clone(), Some(1))].into_iter().collect(),
    ));
    assert_eq!(app.compose().unwrap().attachment_size(&file), None);
}

#[test]
fn attachments_from_discarded_compose_are_ignored_by_new_compose() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::OpenCompose);
    let first_id = app.compose().unwrap().id();

    let effects = app.update(Message::PickComposeAttachments);
    assert_eq!(effects.units(), 1);
    let effect = effects.into_iter().next().unwrap();
    assert!(matches!(
        effect,
        Effect::Ui(UiEffect::PickComposeAttachments(id)) if id == first_id
    ));

    // Discard the first compose draft and open a second one
    let _ = app.update(Message::DiscardCompose);
    assert!(app.compose().is_none());

    let _ = app.update(Message::OpenCompose);
    let second_id = app.compose().unwrap().id();
    assert_ne!(first_id, second_id);

    // If the file dialog for first_id finishes now, its attachments must be ignored
    let file = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let _ = app.update(Message::AddComposeAttachments(first_id, vec![file.clone()]));

    let compose = app.compose().unwrap();
    assert!(compose.attachments().is_empty());
    assert!(compose.is_untouched());

    // But attachments targeting second_id are accepted
    let _ = app.update(Message::AddComposeAttachments(
        second_id,
        vec![file.clone()],
    ));
    let compose = app.compose().unwrap();
    assert_eq!(compose.attachments(), &[file]);
}

#[test]
fn unread_badge_reflects_inbox_unread_and_preference() {
    let mut app = loaded_demo_app();
    assert_eq!(crate::ui::unread_badge_count(&app), Some(6));

    let _ = app.update(Message::SetShowUnreadBadge(false));
    assert_eq!(crate::ui::unread_badge_count(&app), None);

    let _ = app.update(Message::SetShowUnreadBadge(true));
    assert_eq!(crate::ui::unread_badge_count(&app), Some(6));
}

/// Sends a plain key press, as the window would when no field took it.
fn press(app: &mut App, key: Key) {
    let _ = app.update(Message::KeyPressed(KeyPress {
        key,
        command: false,
        other_modifier: false,
    }));
}

/// Sends a command key press (Cmd on macOS, Ctrl on other platforms).
fn press_command(app: &mut App, key: Key) {
    let _ = app.update(Message::KeyPressed(KeyPress {
        key,
        command: true,
        other_modifier: false,
    }));
}

#[test]
fn command_comma_toggles_settings() {
    let mut app = loaded_demo_app();
    assert!(!app.showing_settings());

    press_command(&mut app, Key::Character(','));
    assert!(app.showing_settings());

    press_command(&mut app, Key::Character(','));
    assert!(!app.showing_settings());
}

#[test]
fn command_n_opens_compose() {
    let mut app = loaded_demo_app();
    assert!(app.compose().is_none());

    press_command(&mut app, Key::Character('n'));
    assert!(app.compose().is_some());
}

#[test]
fn command_enter_sends_compose() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::OpenCompose);
    let _ = app.update(Message::ComposeChanged(
        ComposeField::To,
        "recipient@example.com".into(),
    ));
    let _ = app.update(Message::ComposeChanged(ComposeField::Body, "Hello".into()));
    assert_eq!(app.compose().unwrap().sending(), Sending::Writing);

    press_command(&mut app, Key::Enter);

    assert!(app.compose().unwrap().in_flight());
}

#[test]
fn the_mailbox_keys_leave_a_draft_in_its_own_window_alone() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::SetComposePlacement(ComposePlacement::Window));
    open_conversation(&mut app, "demo-3");
    let _ = app.update(Message::OpenCompose);
    let _ = app.update(Message::ComposeChanged(
        ComposeField::To,
        "recipient@example.com".into(),
    ));
    let _ = app.update(Message::ComposeChanged(ComposeField::Body, "Hello".into()));

    // Escape and Send in the mailbox window are not meant for the draft:
    // its own window handles them.
    press_command(&mut app, Key::Enter);
    press(&mut app, Key::Escape);
    let draft = app.compose().unwrap();
    assert_eq!(draft.sending(), Sending::Writing);
    assert!(!draft.confirming_discard());
    // Escape went on to what the mailbox window shows.
    assert!(app.mailbox().unwrap().reader().is_none());
}

#[test]
fn trash_shortcuts_move_selected_conversation() {
    let mut app = loaded_demo_app();
    open_conversation(&mut app, "demo-3");

    press_command(&mut app, Key::Backspace);

    assert!(!is_listed(&app, "demo-3"));
    assert!(app.mailbox().unwrap().undo().is_some());

    let _ = app.update(Message::UndoMove);
    assert!(is_listed(&app, "demo-3"));

    // Plain Delete key also moves to trash
    open_conversation(&mut app, "demo-3");
    press(&mut app, Key::Delete);
    assert!(!is_listed(&app, "demo-3"));
}

#[test]
fn a_save_in_flight_does_not_outlive_the_session() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::SaveAttachment("demo-0".into(), "file".into()));
    let previous_request = app.saving_attachment.as_ref().unwrap().request.clone();
    assert_eq!(app.saving_attachment(), Some("file"));

    let _ = app.update(Message::Logout);
    assert_eq!(app.saving_attachment(), None);
    assert!(app.saved_attachment().is_none());

    let _ = app.open_mailbox(MailBackend::demo(), None, None);
    let _ = app.update(Message::SaveAttachment("demo-1".into(), "new-file".into()));

    // A late save from the previous session must not replace the new one.
    let _ = app.update(Message::AttachmentSaved(
        previous_request,
        Ok(PathBuf::from("/tmp/x.pdf")),
    ));
    assert!(app.saved_attachment().is_none());
    assert_eq!(app.saving_attachment(), Some("new-file"));
}

#[test]
fn revealing_an_attachment_starts_a_background_task() {
    let mut app = loaded_demo_app();
    let effects = app.update(Message::RevealAttachment(PathBuf::from("/tmp/report.pdf")));
    assert_eq!(effects.units(), 1);
}

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
        Ok(vec![Folder::custom("old", "Previous account")]),
    ));
    assert!(app.folders().is_empty());
}

#[test]
fn signing_out_closes_the_settings_pane() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::ShowSettings(true));

    let _ = app.update(Message::Logout);

    // Signing back in must not restore the previous account's Settings view.
    assert!(!app.showing_settings());
}

#[test]
fn settings_shortcuts_do_not_reach_the_hidden_mailbox_panes() {
    let mut app = loaded_demo_app();
    press(&mut app, Key::Character('j'));
    let opened = app
        .mailbox()
        .unwrap()
        .selected_conversation()
        .map(str::to_owned);
    assert!(opened.is_some());

    let _ = app.update(Message::ShowSettings(true));
    // Stepping the hidden list would change the selection unexpectedly.
    press(&mut app, Key::Character('j'));
    assert_eq!(
        app.mailbox().unwrap().selected_conversation(),
        opened.as_deref()
    );

    // Escape closes settings, and the list answers again.
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
fn escape_answers_the_link_prompt_before_the_draft_behind_it() {
    let mut app = loaded_demo_app();
    open_conversation(&mut app, "demo-0");
    let _ = app.update(Message::OpenCompose);
    let _ = app.update(Message::LinkClicked("https://example.com/".to_owned()));

    press(&mut app, Key::Escape);
    assert!(app.pending_link().is_none());
    assert!(app.compose().is_some(), "the draft stays");
}

#[test]
fn the_apps_own_refresh_goes_unannounced_until_someone_asks_for_one() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::AutoRefreshMailbox);
    assert!(matches!(
        app.mailbox().unwrap().status(),
        ListStatus::Refreshing(_)
    ));
    assert!(app.refreshing_on_its_own());

    // Asking now shows the refresh already on its way.
    let _ = app.update(Message::RefreshMailbox);
    assert!(matches!(
        app.mailbox().unwrap().status(),
        ListStatus::Refreshing(_)
    ));
    assert!(!app.refreshing_on_its_own());
}

#[test]
fn a_rows_menu_acts_on_that_row_and_leaves_the_open_one_open() {
    let mut app = loaded_demo_app();
    open_conversation(&mut app, "demo-0");
    let listed = |app: &App, id: &str| app.mailbox().unwrap().has_row(id);
    let unread = |app: &App, id: &str| {
        app.mailbox()
            .unwrap()
            .conversations()
            .iter()
            .find(|row| row.id == id)
            .map(|row| row.unread)
    };

    let before = unread(&app, "demo-2").unwrap();
    let _ = app.update(Message::ApplyRowAction(
        "demo-2".into(),
        MailAction::SetUnread(!before),
    ));
    assert_eq!(unread(&app, "demo-2"), Some(!before));

    let _ = app.update(Message::ApplyRowAction(
        "demo-1".into(),
        MailAction::MoveTo(sys(MailFolder::Archive)),
    ));
    assert!(!listed(&app, "demo-1"));
    let mailbox = app.mailbox().unwrap();
    assert_eq!(mailbox.selected_conversation(), Some("demo-0"));
    let undo = mailbox.undo().expect("its folder is known");
    assert_eq!(undo.row_id, "demo-1");
    assert_eq!(undo.from, Folder::INBOX);
}

#[test]
fn copying_a_links_address_needs_no_prompt_and_only_takes_web_and_mail_links() {
    let mut app = loaded_demo_app();
    let effects = app.update(Message::CopyLinkAddress("https://example.com/a".into()));
    let effect = effects.into_iter().next().expect("a copy");
    assert!(matches!(
        effect,
        Effect::Ui(UiEffect::CopyText(text)) if text == "https://example.com/a"
    ));
    assert!(app.pending_link().is_none());

    let effects = app.update(Message::CopyLinkAddress("javascript:alert(1)".into()));
    assert_eq!(effects.units(), 0);
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
    let page_request = app.last_request - 1;
    deliver_demo_page(&mut app, page_request, 0);
    deliver_folders(&mut app, Ok(vec![invoices.clone()]));
    assert_eq!(app.folders(), [invoices]);
    assert_eq!(app.update(Message::RefreshMailbox).units(), PAGE_AND_COUNTS);
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
    // So does every address a mail link writes to, past its `?` too.
    let target = |link| PendingLink::parse(link).unwrap().target;
    assert_eq!(
        target("mailto:support@bank.example?to=x@evil.example&subject=Hi"),
        "support@bank.example, x@evil.example"
    );
    assert_eq!(target("mailto:?to=x@evil.example"), "x@evil.example");
    assert_eq!(
        target("mailto:a@example.org,b@example.org?to=c@example.org,d@example.org,e@example.org"),
        "a@example.org, b@example.org, c@example.org and 2 more"
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
fn email_links_wait_through_login_and_open_only_an_editable_new_message() {
    let mut app = App::new(Settings::default());
    let url = "mailto:team@example.org?subject=Hello&body=Hola%0D%0Amundo";
    assert_eq!(
        app.update(Message::ReceiveMailto(
            crate::mailto::Request::parse(url).unwrap()
        ))
        .units(),
        0
    );
    for auth in [
        AuthState::CheckingSession,
        AuthState::SignedOut,
        AuthState::NeedsTotp,
    ] {
        app.auth_state = auth.clone();
        assert_eq!(
            app.update(Message::OpenPendingMailto).units(),
            0,
            "{auth:?}"
        );
        assert!(app.compose().is_none(), "{auth:?}");
        assert_eq!(app.pending_mailto_count(), 1, "{auth:?}");
    }
    app.open_mailbox(MailBackend::demo(), None, None);
    assert_eq!(app.update(Message::OpenPendingMailto).units(), 0);
    let compose = app.compose().unwrap();
    assert_eq!(compose.field(ComposeField::To), "team@example.org");
    assert_eq!(compose.field(ComposeField::Subject), "Hello");
    assert_eq!(compose.field(ComposeField::Body), "Hola\nmundo");
    assert_eq!(compose.sending(), Sending::Writing);
    assert!(compose.attachments().is_empty());
    assert_eq!(app.pending_mailto_count(), 0);
}

#[test]
fn email_links_never_replace_a_blank_written_or_sending_draft() {
    let mut app = loaded_demo_app();
    app.update(Message::OpenCompose);
    let id = app.compose().unwrap().id();
    app.update(Message::ReceiveMailto(
        crate::mailto::Request::parse("mailto:next@example.org?body=Next").unwrap(),
    ));
    app.update(Message::OpenPendingMailto);
    assert_eq!(app.compose().unwrap().id(), id);
    app.update(Message::ComposeChanged(
        ComposeField::To,
        "current@example.org".into(),
    ));
    app.update(Message::ComposeChanged(
        ComposeField::Body,
        "My draft".into(),
    ));
    app.update(Message::OpenPendingMailto);
    assert_eq!(app.compose().unwrap().field(ComposeField::Body), "My draft");
    let sending = app.update(Message::Send);
    assert!(sending.units() > 0);
    assert!(app.compose().unwrap().in_flight());
    app.update(Message::DiscardCompose);
    app.update(Message::OpenPendingMailto);
    assert_eq!(app.compose().unwrap().id(), id);
    assert_eq!(app.pending_mailto_count(), 1);
    app.update(Message::Sent(
        app.session_epoch,
        Err(SendError::Unconfirmed),
    ));
    app.update(Message::DiscardCompose);
    app.update(Message::OpenPendingMailto);
    assert_ne!(app.compose().unwrap().id(), id);
    assert_eq!(
        app.compose().unwrap().field(ComposeField::To),
        "next@example.org"
    );
}

#[test]
fn internal_email_links_use_confirmation_and_the_same_local_queue() {
    let mut app = loaded_demo_app();
    app.update(Message::LinkClicked(
        "mailto:team@example.org?subject=Confirmed".into(),
    ));
    assert!(app.pending_link().is_some());
    assert_eq!(app.pending_mailto_count(), 0);
    assert_eq!(app.update(Message::OpenLink).units(), 0);
    app.update(Message::OpenPendingMailto);
    assert_eq!(
        app.compose().unwrap().field(ComposeField::Subject),
        "Confirmed"
    );
    app.update(Message::DiscardCompose);
    app.update(Message::SetConfirmLinks(false));
    assert_eq!(
        app.update(Message::LinkClicked(
            "mailto:team@example.org?subject=Direct".into()
        ))
        .units(),
        0
    );
    app.update(Message::OpenPendingMailto);
    assert_eq!(
        app.compose().unwrap().field(ComposeField::Subject),
        "Direct"
    );
    app.update(Message::DiscardCompose);
    app.update(Message::LinkClicked(
        "mailto:team@example.org?subject=%0AInjected".into(),
    ));
    assert!(app.compose().is_none());
    assert_eq!(app.pending_mailto_count(), 0);
    assert!(app.mailto_error().is_some());
}

#[test]
fn email_links_are_bounded_dismissible_and_do_not_cross_a_closed_session() {
    let mut app = loaded_demo_app();
    app.update(Message::ShowSettings(true));
    for index in 0..crate::mailto::MAX_PENDING {
        app.update(Message::ReceiveMailto(
            crate::mailto::Request::parse(&format!("mailto:team@example.org?subject={index}"))
                .unwrap(),
        ));
    }
    app.update(Message::ReceiveMailto(
        crate::mailto::Request::parse("mailto:overflow@example.org").unwrap(),
    ));
    assert!(app.mailto_error().is_some());
    assert_eq!(app.pending_mailto_count(), crate::mailto::MAX_PENDING);
    app.update(Message::OpenPendingMailto);
    assert!(app.compose().is_none());
    app.update(Message::DismissPendingMailto);
    app.update(Message::DismissMailtoError);
    assert!(app.mailto_error().is_none());
    app.update(Message::ShowSettings(false));
    app.update(Message::OpenPendingMailto);
    assert_eq!(app.compose().unwrap().field(ComposeField::Subject), "1");
    app.close_mailbox(None);
    assert_eq!(app.pending_mailto_count(), 0);
    assert!(app.compose().is_none());
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
    for (case, message) in [
        ("search typed", Message::SearchChanged("alex".into())),
        (
            "panels resized",
            Message::PanelsResized(Panels {
                sidebar: 0.3,
                ..app.panels()
            }),
        ),
        ("search cleared", Message::SearchChanged(String::new())),
    ] {
        assert_eq!(app.update(message).units(), 0, "{case}");
    }

    assert_eq!(app.last_request, requests);
}

fn loaded_demo_app() -> App {
    let (mut app, _) = App::boot(true, Settings::default());
    deliver_demo_page(&mut app, OPENING_PAGE, 0);
    let counts = demo_service(&app).counts();
    let _ = app.update(Message::CountsLoaded(OPENING_COUNTS, Ok(counts)));
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
fn demo_selection_is_local() {
    let (mut app, _) = App::boot(true, Settings::default());
    deliver_demo_page(&mut app, OPENING_PAGE, 0);

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
    deliver_demo_page(&mut app, OPENING_PAGE, 0);

    assert_eq!(app.mailbox().unwrap().reader_state(), &ReaderState::Empty);
}

#[test]
fn demo_selection_opens_reader_with_newest_message_expanded() {
    let (mut app, _) = App::boot(true, Settings::default());
    deliver_demo_page(&mut app, OPENING_PAGE, 0);

    open_conversation(&mut app, "demo-0");

    let reader = app.mailbox().unwrap().reader().unwrap();
    assert_eq!(reader.conversation_id(), "demo-0");
    assert_eq!(reader.detail().id, reader.conversation_id());
    assert_eq!(reader.detail().messages.len(), 3);
    assert!(reader.is_expanded("demo-0-2", Reading::default()));
    assert!(!reader.is_expanded("demo-0-0", Reading::default()));
}

#[test]
fn returning_to_a_conversation_restores_its_reader_without_loading() {
    let (mut app, _) = App::boot(true, Settings::default());
    deliver_demo_page(&mut app, OPENING_PAGE, 0);
    open_conversation(&mut app, "demo-0");

    let _ = app.update(Message::ToggleMessageExpanded("demo-0-0".into()));
    let reader = app.mailbox().unwrap().reader().unwrap();
    assert!(reader.is_expanded("demo-0-0", Reading::default()));
    assert!(reader.is_expanded("demo-0-2", Reading::default()));

    open_conversation(&mut app, "demo-2");
    let effects = app.update(Message::SelectConversation("demo-0".into()));
    // The only work is scrolling the restored reader to its top.
    assert_eq!(effects.units(), 1);
    assert!(
        app.mailbox()
            .unwrap()
            .reader()
            .unwrap()
            .is_expanded("demo-0-0", Reading::default())
    );
}

#[test]
fn folder_switch_closes_reader() {
    let (mut app, _) = App::boot(true, Settings::default());
    deliver_demo_page(&mut app, OPENING_PAGE, 0);
    let _ = app.update(Message::SelectConversation("demo-0".into()));

    let _ = app.update(Message::SelectFolder(sys(MailFolder::Sent)));

    assert!(app.mailbox().unwrap().reader().is_none());
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

#[test]
fn automatic_refresh_updates_the_open_folder_and_invalidates_other_caches() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::SelectFolder(sys(MailFolder::Sent)));
    deliver_latest_demo_page(&mut app, 0);
    assert_eq!(
        app.update(Message::SelectFolder(sys(MailFolder::Inbox)))
            .units(),
        0
    );

    assert_eq!(
        app.update(Message::AutoRefreshMailbox).units(),
        PAGE_AND_COUNTS
    );
    let page_request = app.last_request - 1;
    deliver_demo_page(&mut app, page_request, 0);

    assert_eq!(
        app.update(Message::SelectFolder(sys(MailFolder::Sent)))
            .units(),
        1
    );
    assert!(matches!(
        app.mailbox().unwrap().status(),
        ListStatus::Refreshing(_)
    ));
}

#[test]
fn an_undo_offer_that_expires_while_signing_out_still_goes() {
    let mut app = loaded_demo_app();
    open_conversation(&mut app, "demo-3");
    press_command(&mut app, Key::Backspace);
    let offer = app.mailbox().unwrap().undo().cloned().expect("an offer");

    // The list stays on screen while signing out, and so does the notice,
    // whose timer asks for it to go.
    app.set_auth_state(AuthState::SigningOut);
    let _ = app.update(Message::DismissUndo(offer));
    assert!(app.mailbox().unwrap().undo().is_none());
}

#[test]
fn an_automatic_refresh_holds_back_neither_a_move_nor_its_undo() {
    let mut app = loaded_demo_app();
    open_conversation(&mut app, "demo-3");
    assert_eq!(
        app.update(Message::AutoRefreshMailbox).units(),
        PAGE_AND_COUNTS
    );
    press_command(&mut app, Key::Backspace);
    assert!(!is_listed(&app, "demo-3"));

    // The next poll starts while the notice is still up, and keeps it.
    assert_eq!(
        app.update(Message::AutoRefreshMailbox).units(),
        PAGE_AND_COUNTS
    );
    assert!(app.mailbox().unwrap().undo().is_some());
    let _ = app.update(Message::UndoMove);
    assert!(is_listed(&app, "demo-3"));
}

#[test]
fn automatic_refresh_does_not_overlap_a_list_request() {
    let mut app = loaded_demo_app();
    assert_eq!(app.update(Message::RefreshMailbox).units(), PAGE_AND_COUNTS);
    let requests = app.last_request;

    assert_eq!(app.update(Message::AutoRefreshMailbox).units(), 0);
    assert_eq!(app.last_request, requests);
}

#[test]
fn automatic_refresh_waits_while_a_message_is_being_sent() {
    let mut app = loaded_demo_app();
    write(&mut app, "alex@example.com");
    assert_eq!(app.update(Message::Send).units(), 1);
    let requests = app.last_request;

    assert!(!app.auto_refresh_available());
    assert_eq!(app.update(Message::AutoRefreshMailbox).units(), 0);
    assert_eq!(app.last_request, requests);
}

#[test]
fn inbox_arrivals_notify_independently_of_unread_counts() {
    let mut app = loaded_demo_app();
    let old = IncomingMail {
        id: "old".into(),
        time: 100,
        unread: true,
        sender: "Old sender".into(),
        subject: "Old mail".into(),
    };
    assert_eq!(deliver_new_mail(&mut app, Ok(vec![old.clone()])).units(), 0);
    let current = IncomingMail {
        id: "new".into(),
        time: 101,
        unread: true,
        sender: "Alice".into(),
        subject: "New mail".into(),
    };
    // Reading the previous mail leaves the unread count unchanged.
    let read_old = IncomingMail {
        unread: false,
        ..old
    };
    let effects = deliver_new_mail(&mut app, Ok(vec![current.clone(), read_old.clone()]));
    let notify_effect = effects.into_iter().next().expect("ui effect");
    assert!(matches!(
        notify_effect,
        Effect::Ui(UiEffect::NotifyNewMail { ref sender, ref subject })
            if sender == "Alice" && subject == "New mail"
    ));
    assert_eq!(
        deliver_new_mail(&mut app, Ok(vec![current, read_old])).units(),
        0
    );
}

fn deliver_new_mail(app: &mut App, result: Result<Vec<IncomingMail>, MailboxError>) -> Effects {
    assert_eq!(app.fetch_new_mail_notification().units(), 1);
    let request = app.new_mail.pending.unwrap();
    app.update(Message::NewMailLoaded(app.session_epoch, request, result))
}

#[test]
fn notification_polls_are_bounded_and_stale_or_disabled_results_are_ignored() {
    let mut app = loaded_demo_app();
    assert_eq!(app.fetch_new_mail_notification().units(), 1);
    let first = app.new_mail.pending.unwrap();
    assert_eq!(app.fetch_new_mail_notification().units(), 0);
    let _ = app.update(Message::SetDesktopNotifications(false));
    let _ = app.update(Message::SetDesktopNotifications(true));
    assert_eq!(app.fetch_new_mail_notification().units(), 1);
    let second = app.new_mail.pending.unwrap();
    assert_ne!(first, second);
    assert_eq!(
        app.update(Message::NewMailLoaded(
            app.session_epoch,
            first,
            Ok(Vec::new())
        ))
        .units(),
        0
    );
    assert_eq!(app.new_mail.pending, Some(second));
    assert_eq!(
        app.update(Message::NewMailLoaded(
            app.session_epoch,
            second,
            Err(MailboxError::Unavailable)
        ))
        .units(),
        0
    );
    assert!(app.new_mail.pending.is_none());
    assert_eq!(deliver_new_mail(&mut app, Ok(Vec::new())).units(), 0);
    assert_eq!(
        deliver_new_mail(&mut app, Err(MailboxError::SessionExpired)).units(),
        0
    );
    assert!(app.mailbox().is_none());
}

#[test]
fn late_new_mail_result_never_notifies_after_logout_or_in_another_session() {
    let mut app = loaded_demo_app();
    let effect = app
        .fetch_new_mail_notification()
        .into_iter()
        .next()
        .expect("notification request");
    let Effect::Future(future) = effect else {
        panic!("expected future effect");
    };
    let late_result = futures::executor::block_on(future);

    app.auth_state = AuthState::SigningOut;
    assert_eq!(app.update(late_result.clone()).units(), 0);
    app.auth_state = AuthState::Authenticated { email: None };

    let _ = app.update(Message::Logout);
    assert_eq!(app.update(late_result.clone()).units(), 0);

    let _ = app.open_mailbox(MailBackend::demo(), None, None);
    assert_eq!(app.update(late_result).units(), 0);
}

#[test]
fn automatic_refresh_does_not_notify_when_inbox_unread_stays_same() {
    let mut app = loaded_demo_app();
    let initial_unread = app
        .mailbox()
        .unwrap()
        .counts()
        .unwrap()
        .unread(&Folder::INBOX)
        .unwrap();

    let _ = app.update(Message::AutoRefreshMailbox);
    let counts_request = app.last_request;

    let new_counts: crate::mail::MailboxCounts =
        [(Folder::INBOX, initial_unread)].into_iter().collect();
    let effects = app.update(Message::CountsLoaded(counts_request, Ok(new_counts)));
    assert_eq!(effects.units(), 0);
}

#[test]
fn manual_refresh_does_not_trigger_notification_when_unread_increases() {
    let mut app = loaded_demo_app();
    let initial_unread = app
        .mailbox()
        .unwrap()
        .counts()
        .unwrap()
        .unread(&Folder::INBOX)
        .unwrap();

    let _ = app.update(Message::RefreshMailbox);
    let counts_request = app.last_request;

    let new_counts: crate::mail::MailboxCounts =
        [(Folder::INBOX, initial_unread + 1)].into_iter().collect();
    let effects = app.update(Message::CountsLoaded(counts_request, Ok(new_counts)));
    assert_eq!(effects.units(), 0);
}

#[test]
fn automatic_refresh_does_not_notify_when_desktop_notifications_disabled() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::SetDesktopNotifications(false));
    assert!(!app.settings().desktop_notifications);

    let initial_unread = app
        .mailbox()
        .unwrap()
        .counts()
        .unwrap()
        .unread(&Folder::INBOX)
        .unwrap();

    let _ = app.update(Message::AutoRefreshMailbox);
    let counts_request = app.last_request;

    let new_counts: crate::mail::MailboxCounts =
        [(Folder::INBOX, initial_unread + 1)].into_iter().collect();
    let effects = app.update(Message::CountsLoaded(counts_request, Ok(new_counts)));
    assert_eq!(effects.units(), 0);
}

#[test]
fn a_mutation_during_refresh_is_followed_by_a_fresh_request() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::RefreshMailbox);
    let page_request = app.last_request - 1;
    app.mailbox.as_mut().unwrap().invalidate_listings();
    let folder = app.mailbox().unwrap().folder();
    let page = demo_service(&app).list_conversations(folder, 0, demo::PAGE_SIZE, NOW);

    assert_eq!(
        app.update(Message::ConversationsLoaded(page_request, page))
            .units(),
        2
    );
    assert!(matches!(
        app.mailbox().unwrap().status(),
        ListStatus::Refreshing(_)
    ));
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
    open_conversation(&mut app, "demo-0");
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
fn abandoned_conversation_loads_are_stopped_and_ignored() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::SelectConversation("demo-0".into()));
    let first_request = pending_reader_request(&app);
    let first = app.reader_load.clone().expect("a load is in flight");

    // Moving on to the next row leaves the first load unwanted.
    let _ = app.update(Message::SelectConversation("demo-1".into()));
    assert!(first.is_aborted());
    let second = app.reader_load.clone().unwrap();
    assert!(!second.is_aborted());
    // What an aborted load answers changes nothing on screen.
    let _ = app.update(Message::ConversationLoaded(
        first_request,
        Err(MailboxError::Unavailable),
    ));
    assert_eq!(pending_reader_request(&app).conversation_id, "demo-1");

    let _ = app.update(Message::SelectFolder(sys(MailFolder::Archive)));
    assert!(second.is_aborted(), "a folder change closes the reader");

    let mut app = loaded_demo_app();
    let _ = app.update(Message::SelectConversation("demo-0".into()));
    let load = app.reader_load.clone().unwrap();
    let _ = app.update(Message::Logout);
    assert!(load.is_aborted(), "the session ended");
}

#[test]
fn demo_star_actions_update_starred_folder_and_selection() {
    let mut app = loaded_demo_app();
    open_conversation(&mut app, "demo-1");
    let _ = app.update(Message::ApplyAction(MailAction::SetStarred(true)));
    let _ = app.update(Message::ApplyAction(MailAction::SetStarred(true)));
    assert!(app.mailbox().unwrap().conversations()[1].starred);

    let _ = app.update(Message::SelectFolder(sys(MailFolder::Starred)));
    deliver_latest_demo_page(&mut app, 0);
    assert!(is_listed(&app, "demo-1"));

    let effects = app.update(Message::SelectConversation("demo-1".into()));
    assert_eq!(effects.units(), 1);
    let _ = app.update(Message::ApplyAction(MailAction::SetStarred(false)));
    let mailbox = app.mailbox().unwrap();
    assert!(!is_listed(&app, "demo-1"));
    assert!(
        mailbox
            .selected_conversation()
            .is_some_and(|id| id != "demo-1")
    );
}

#[test]
fn a_search_submitted_while_the_folder_loads_starts_after_it() {
    let (mut app, _) = App::boot(true, Settings::default());
    let _ = app.update(Message::SearchChanged("Avery".into()));

    // The first page is still on its way; a search now would drop it.
    assert_eq!(app.update(Message::SearchSubmitted).units(), 0);

    let folder = app.mailbox().unwrap().folder();
    let page = demo_service(&app).list_conversations(folder, 0, demo::PAGE_SIZE, NOW);
    let effects = app.update(Message::ConversationsLoaded(OPENING_PAGE, page));
    assert_eq!(effects.units(), 1, "the waiting search starts");
    assert!(app.mailbox().unwrap().loaded_count() > 0);

    let request = SearchRequest {
        id: app.last_request,
        query: "Avery".into(),
        page: 0,
        page_size: demo::PAGE_SIZE,
    };
    let result = demo_service(&app).search("Avery", demo::PAGE_SIZE, NOW);
    let _ = app.update(Message::SearchLoaded(request, result));
    assert_eq!(app.mailbox().unwrap().shown_search_query(), Some("Avery"));

    // Leaving the results shows the folder that loaded meanwhile.
    let _ = app.update(Message::SearchChanged(String::new()));
    assert!(app.mailbox().unwrap().visible_conversations().len() > 0);
}

#[test]
fn a_search_waiting_for_the_folder_is_dropped_when_the_query_changes() {
    let (mut app, _) = App::boot(true, Settings::default());
    let _ = app.update(Message::SearchChanged("Avery".into()));
    let _ = app.update(Message::SearchSubmitted);
    let _ = app.update(Message::SearchChanged("Avery Q".into()));

    let folder = app.mailbox().unwrap().folder();
    let page = demo_service(&app).list_conversations(folder, 0, demo::PAGE_SIZE, NOW);
    let effects = app.update(Message::ConversationsLoaded(OPENING_PAGE, page));

    assert_eq!(effects.units(), 0, "Enter was for another query");
    assert_eq!(app.mailbox().unwrap().shown_search_query(), None);
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

    // A second press must not start a duplicate fetch.
    let _ = app.update(Message::SaveAttachment("demo-0".into(), "file".into()));
    assert_eq!(app.saving_attachment(), Some("file"));

    let _ = app.update(Message::SaveAttachment("demo-0".into(), "other".into()));
    assert_eq!(app.saving_attachment(), Some("file"));

    deliver_attachment(&mut app, Err(SaveError::NotFetched));
    assert_eq!(app.saving_attachment(), None);
    // Once the first one is done, the next press is free to go.
    let _ = app.update(Message::SaveAttachment("demo-0".into(), "other".into()));
    assert_eq!(app.saving_attachment(), Some("other"));
}

fn choose_attachment_destination(app: &mut App) -> AttachmentSaveRequest {
    let effects = app.update(Message::SaveAttachmentAs(
        "demo-0".into(),
        "file".into(),
        "../../report.pdf".into(),
    ));
    let mut effects = effects.into_iter();
    let Some(Effect::Ui(UiEffect::PickAttachmentDestination {
        request,
        suggested_name,
    })) = effects.next()
    else {
        panic!("save as should only request a native destination dialog");
    };
    assert_eq!(suggested_name, "report.pdf");
    assert!(
        effects.next().is_none(),
        "no download before the destination is chosen"
    );
    request
}

#[test]
fn cancelling_save_as_does_not_download_or_change_a_later_save() {
    let mut app = loaded_demo_app();
    let first = choose_attachment_destination(&mut app);
    assert_eq!(
        app.update(Message::SaveAttachment("demo-0".into(), "other".into()))
            .units(),
        0
    );
    assert_eq!(
        app.update(Message::AttachmentDestinationChosen(first.clone(), None))
            .units(),
        0
    );
    assert!(app.saving_attachment().is_none());
    assert!(app.saved_attachment().is_none());

    let second = choose_attachment_destination(&mut app);
    assert_ne!(first, second);
    for (case, stale) in [
        (
            "a destination for the earlier download",
            Message::AttachmentDestinationChosen(
                first.clone(),
                Some(std::env::temp_dir().join("stale.pdf")),
            ),
        ),
        (
            "no destination for the earlier download",
            Message::AttachmentDestinationChosen(first.clone(), None),
        ),
        (
            "the earlier download's result",
            Message::AttachmentSaved(first, Err(SaveError::Failed)),
        ),
        // A completion cannot skip the destination-selection phase.
        (
            "a result before its destination",
            Message::AttachmentSaved(second, Err(SaveError::Failed)),
        ),
    ] {
        assert_eq!(app.update(stale).units(), 0, "{case}");
        assert_eq!(app.saving_attachment(), Some("file"), "{case}");
        assert!(app.saved_attachment().is_none(), "{case}");
    }
}

#[tokio::test]
async fn chosen_destination_starts_one_download_and_failure_creates_no_file() {
    let mut app = loaded_demo_app();
    let request = choose_attachment_destination(&mut app);
    let path = std::env::temp_dir().join(format!("ruston-unfetched-{}.pdf", std::process::id()));
    assert!(!path.exists());
    let effects = app.update(Message::AttachmentDestinationChosen(
        request.clone(),
        Some(path.clone()),
    ));
    assert_eq!(effects.units(), 1);
    assert_eq!(
        app.update(Message::AttachmentDestinationChosen(
            request,
            Some(path.clone())
        ))
        .units(),
        0
    );
    let Some(Effect::Future(task)) = effects.into_iter().next() else {
        panic!("expected attachment download");
    };
    let _ = app.update(task.await);
    assert!(app.saving_attachment().is_none());
    assert_eq!(app.saved_attachment(), Some(Err(SaveError::NotFetched)));
    assert!(
        !path.exists(),
        "failed downloads must not create the selected file"
    );
}

#[test]
fn save_as_dialog_results_after_sign_out_cannot_start_a_download() {
    let mut app = loaded_demo_app();
    let previous = choose_attachment_destination(&mut app);
    let _ = app.update(Message::Logout);
    let _ = app.open_mailbox(MailBackend::demo(), None, None);
    let current = choose_attachment_destination(&mut app);
    assert_ne!(previous.session, current.session);
    assert_eq!(
        app.update(Message::AttachmentDestinationChosen(
            previous,
            Some(std::env::temp_dir().join("stale.pdf"))
        ))
        .units(),
        0
    );
    assert_eq!(app.saving_attachment.as_ref().unwrap().request, current);
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
    open_conversation(&mut app, &found);

    // The row is not in the open folder's listing, which used to leave
    // the reader with no actions at all.
    assert_eq!(
        app.mailbox().unwrap().selected_summary().map(|row| &row.id),
        Some(&found)
    );

    let _ = app.update(Message::ApplyAction(MailAction::SetStarred(true)));

    // The results are what is on screen, so they carry the change.
    let mailbox = app.mailbox().unwrap();
    assert_eq!(mailbox.shown_search_query(), Some("offsite"));
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
    open_conversation(&mut app, &rows[0]);

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
    open_conversation(&mut app, "demo-0");

    let _ = app.update(Message::ApplyAction(MailAction::MoveTo(sys(
        MailFolder::Archive,
    ))));

    let mailbox = app.mailbox().unwrap();
    assert!(!is_listed(&app, "demo-0"));
    assert_eq!(mailbox.selected_conversation(), Some("demo-1"));

    let _ = app.update(Message::SelectFolder(sys(MailFolder::Archive)));
    deliver_latest_demo_page(&mut app, 0);
    assert!(is_listed(&app, "demo-0"));
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
    open_conversation(&mut app, "demo-3");

    let _ = app.update(Message::ApplyAction(MailAction::MoveTo(sys(
        MailFolder::Archive,
    ))));
    assert!(!is_listed(&app, "demo-3"));
    assert!(app.mailbox().unwrap().undo().is_some());

    let _ = app.update(Message::UndoMove);

    assert!(is_listed(&app, "demo-3"));
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
        open_conversation(&mut app, "demo-3");
        let _ = app.update(message);
        assert!(!is_listed(&app, "demo-3"), "{folder:?}");

        let _ = app.update(Message::SelectFolder(folder.clone()));
        deliver_latest_demo_page(&mut app, 0);
        assert!(is_listed(&app, "demo-3"), "{folder:?}");
    }
}

#[test]
fn a_move_proton_refuses_brings_the_row_back_with_the_list() {
    let mut app = loaded_demo_app();
    open_conversation(&mut app, "demo-0");
    // As the Proton path does: the move shows at once.
    let request = app
        .mailbox
        .as_mut()
        .unwrap()
        .start_action(MailAction::MoveTo(sys(MailFolder::Archive)), 90)
        .and_then(|start| start.send)
        .unwrap();
    let has_row = |app: &App| app.mailbox().unwrap().has_row("demo-0");
    assert!(!has_row(&app));

    let _ = app.update(Message::ActionFinished(request, Err(MailboxError::Service)));

    let mailbox = app.mailbox().unwrap();
    assert_eq!(mailbox.action_error(), Some(MailboxError::Service));
    let ListStatus::Refreshing(page) = mailbox.status() else {
        panic!("the folder is read again");
    };
    deliver_demo_page(&mut app, page, 0);
    assert!(has_row(&app), "Proton never moved it");
}

#[test]
fn list_keys_wait_while_the_draft_covers_the_reader() {
    let mut app = loaded_demo_app();
    open_conversation(&mut app, "demo-0");
    let _ = app.update(Message::OpenCompose);

    for key in [Key::Character('j'), Key::ArrowDown] {
        press(&mut app, key);
        assert_eq!(
            app.mailbox().unwrap().selected_conversation(),
            Some("demo-0"),
            "{key:?} opened a conversation behind the draft"
        );
    }

    // In its own window the draft leaves the reader in view.
    let _ = app.update(Message::SetComposePlacement(ComposePlacement::Window));
    press(&mut app, Key::Character('j'));
    assert_eq!(
        app.mailbox().unwrap().selected_conversation(),
        Some("demo-1")
    );
}

#[test]
fn the_row_a_list_key_opens_is_brought_into_view() {
    let mut app = loaded_demo_app();
    open_conversation(&mut app, "demo-0");

    let effects = app.update(Message::KeyPressed(KeyPress {
        key: Key::Character('j'),
        command: false,
        other_modifier: false,
    }));

    assert_eq!(
        app.mailbox().unwrap().selected_conversation(),
        Some("demo-1")
    );
    assert!(effects.into_iter().any(|effect| matches!(
        effect,
        Effect::Ui(UiEffect::RevealConversation(ref id)) if id == "demo-1"
    )));
}

fn is_unread(app: &App, id: &str) -> bool {
    app.mailbox()
        .unwrap()
        .conversations()
        .iter()
        .find(|row| row.id == id)
        .is_some_and(|row| row.unread)
}

fn unread_rows(app: &App) -> Vec<String> {
    app.mailbox()
        .unwrap()
        .conversations()
        .iter()
        .filter(|row| row.unread)
        .map(|row| row.id.clone())
        .collect()
}

/// Whether the open folder's list has a row for `id`.
fn is_listed(app: &App, id: &str) -> bool {
    app.mailbox()
        .unwrap()
        .conversations()
        .iter()
        .any(|row| row.id == id)
}

fn open_conversation(app: &mut App, id: &str) {
    let _ = app.update(Message::SelectConversation(id.to_owned()));
    deliver_selected_demo_detail(app);
}

#[test]
fn a_conversation_opened_behind_a_draft_stays_unread_until_it_shows() {
    let mut app = loaded_demo_app();
    let id = unread_rows(&app).remove(0);
    let _ = app.update(Message::OpenCompose);

    // A click in the list opens it behind the draft, where nobody reads it.
    open_conversation(&mut app, &id);
    assert_eq!(app.mailbox().unwrap().selected_conversation(), Some(&*id));
    assert!(is_unread(&app, &id), "marked read behind the draft");

    // Putting the untouched draft away shows it, and so reads it.
    let _ = app.update(Message::CloseCompose);
    assert!(app.compose().is_none());
    assert!(!is_unread(&app, &id));
}

#[test]
fn only_the_conversation_the_reader_shows_after_a_draft_is_read() {
    let mut app = loaded_demo_app();
    let unread = unread_rows(&app);
    let (passed, kept) = (&unread[0], &unread[1]);
    let _ = app.update(Message::OpenCompose);

    open_conversation(&mut app, passed);
    open_conversation(&mut app, kept);
    let _ = app.update(Message::CloseCompose);

    assert!(is_unread(&app, passed), "never shown, yet marked read");
    assert!(!is_unread(&app, kept));
}

#[test]
fn changes_on_their_way_to_proton_count_until_they_arrive() {
    let mut app = loaded_demo_app();
    assert_eq!(app.unsent_changes(), 0);
    // Proton's actions go one after another, the way the demo's never wait.
    let star = |app: &mut App, row: &str, request| {
        app.mailbox
            .as_mut()
            .unwrap()
            .start_action_on(
                row.to_owned(),
                crate::mail::SummaryKind::Conversation,
                MailAction::SetStarred(true),
                request,
            )
            .unwrap()
    };
    let sent = star(&mut app, "demo-0", 900).send.unwrap();
    assert!(
        star(&mut app, "demo-1", 901).send.is_none(),
        "it waits its turn"
    );
    assert_eq!(app.unsent_changes(), 2);

    let finish = |app: &mut App, request| {
        app.mailbox
            .as_mut()
            .unwrap()
            .finish_action(&request, Ok(()))
    };
    let next = finish(&mut app, sent).next_action.unwrap();
    assert_eq!(app.unsent_changes(), 1);
    let _ = finish(&mut app, next);
    assert_eq!(app.unsent_changes(), 0);
}

#[test]
fn a_conversation_left_behind_a_draft_is_never_marked_read() {
    let mut app = loaded_demo_app();
    let inbox_unread = |app: &App| demo_service(app).counts().unread(&Folder::INBOX);
    let before = inbox_unread(&app);
    let id = unread_rows(&app).remove(0);
    let _ = app.update(Message::OpenCompose);
    open_conversation(&mut app, &id);

    // Another folder replaces it before the draft is put away.
    let _ = app.update(Message::SelectFolder(sys(MailFolder::Archive)));
    deliver_latest_demo_page(&mut app, 0);
    let _ = app.update(Message::CloseCompose);

    assert_eq!(inbox_unread(&app), before);
}

#[test]
fn moving_the_draft_to_its_own_window_shows_what_it_covered() {
    let mut app = loaded_demo_app();
    let id = unread_rows(&app).remove(0);
    let _ = app.update(Message::OpenCompose);
    open_conversation(&mut app, &id);
    assert!(is_unread(&app, &id));

    let _ = app.update(Message::SetComposePlacement(ComposePlacement::Window));

    assert!(app.compose().is_some(), "the draft is still being written");
    assert!(!is_unread(&app, &id));
    // With the reader in view, what opens is read at once.
    let next = unread_rows(&app).remove(0);
    open_conversation(&mut app, &next);
    assert!(!is_unread(&app, &next));
}

#[test]
fn demo_mail_marked_not_spam_goes_back_to_the_inbox() {
    let mut app = loaded_demo_app();
    open_conversation(&mut app, "demo-3");
    let _ = app.update(Message::ApplyAction(MailAction::MoveTo(sys(
        MailFolder::Spam,
    ))));
    let _ = app.update(Message::SelectFolder(sys(MailFolder::Spam)));
    deliver_latest_demo_page(&mut app, 0);
    open_conversation(&mut app, "demo-3");

    let _ = app.update(Message::ApplyAction(MailAction::MoveTo(Folder::INBOX)));

    assert!(!is_listed(&app, "demo-3"));
    let undo = app.mailbox().unwrap().undo().expect("an offer to undo");
    assert_eq!(undo.from, sys(MailFolder::Spam));
    assert_eq!(undo.to, Folder::INBOX);

    let _ = app.update(Message::SelectFolder(Folder::INBOX));
    deliver_latest_demo_page(&mut app, 0);
    assert!(is_listed(&app, "demo-3"));
}

#[test]
fn inbox_search_reflects_folder_moves_immediately() {
    for folder in [MailFolder::Archive, MailFolder::Trash, MailFolder::Spam] {
        let mut app = loaded_demo_app();
        let _ = app.update(Message::SearchChanged("blue notebook".into()));
        assert_eq!(app.mailbox().unwrap().visible_conversations().count(), 1);
        open_conversation(&mut app, "demo-3");

        let _ = app.update(Message::ApplyAction(MailAction::MoveTo(sys(folder))));

        let mailbox = app.mailbox().unwrap();
        assert_eq!(mailbox.search_query(), "blue notebook", "{folder:?}");
        assert_eq!(mailbox.visible_conversations().count(), 0, "{folder:?}");
        assert_eq!(mailbox.selected_conversation(), None, "{folder:?}");
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
    deliver_demo_page(&mut app, OPENING_PAGE, 0);
    open_conversation(&mut app, "demo-0");
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
    assert_eq!(mailbox.conversations().len(), demo::PAGE_SIZE as usize);
    let reader = mailbox.reader().unwrap();
    assert_eq!(reader.conversation_id(), "demo-0");
    assert!(reader.is_expanded("demo-0-0", Reading::default()));
}

#[test]
fn resetting_layout_preserves_other_preferences() {
    let mut app = authenticated_app();
    app.settings.window = Window {
        width: 1_300.0,
        height: 900.0,
        ..Window::default()
    };
    app.settings.panels = Panels {
        sidebar: 0.35,
        conversations: 0.6,
    };
    app.settings.appearance = Appearance::Light;
    let revision = app.settings_revision();

    let effect = app.update(Message::ResetLayout).into_iter().next();

    assert!(matches!(effect, Some(Effect::Ui(UiEffect::ResetLayout))));
    assert_eq!(app.settings.window, Window::default());
    assert_eq!(app.settings.panels, Panels::default());
    assert_eq!(app.settings.appearance, Appearance::Light);
    assert_eq!(app.settings_revision(), revision + 1);
    assert!(app.mailbox().is_some());
}

#[test]
fn exiting_demo_clears_mailbox_and_opens_login() {
    let (mut app, _) = App::boot(true, Settings::default());
    deliver_demo_page(&mut app, OPENING_PAGE, 0);

    let _ = app.update(Message::Logout);

    assert_eq!(app.auth_state, AuthState::SignedOut);
    assert_eq!(app.auth_error, Some(AuthError::DemoSignInUnavailable));
    assert!(app.backend.is_none());
    assert!(app.mailbox.is_none());
    assert!(!app.is_demo());
}

#[test]
fn a_demo_process_never_starts_a_proton_sign_in() {
    let (mut app, _) = App::boot(true, Settings::default());
    deliver_demo_page(&mut app, OPENING_PAGE, 0);
    let _ = app.update(Message::Logout);
    app.login_form.username = "username sentinel".into();
    app.login_form.password = "password sentinel".to_owned().into();
    let attempt = app.auth_attempt;

    let effects = app.update(Message::Submit);

    assert_eq!(effects.units(), 0);
    assert_eq!(app.auth_state, AuthState::SignedOut);
    assert_eq!(app.auth_error, Some(AuthError::DemoSignInUnavailable));
    assert_eq!(app.auth_attempt, attempt);
    assert!(app.backend.is_none());
}

#[test]
fn a_demo_process_keeps_its_notice_while_the_form_is_edited() {
    let (mut app, _) = App::boot(true, Settings::default());
    deliver_demo_page(&mut app, OPENING_PAGE, 0);
    let _ = app.update(Message::Logout);

    app.login_form.username = "username sentinel".into();
    app.login_edited();

    assert_eq!(app.auth_error, Some(AuthError::DemoSignInUnavailable));
}

#[test]
fn a_normal_process_clears_its_error_when_the_form_is_edited() {
    let (mut app, _) = App::boot(false, Settings::default());
    let _ = app.update(Message::SessionChecked(ResumeOutcome::SignedOut));
    let _ = app.update(Message::Submit);
    assert_eq!(app.auth_error, Some(AuthError::UsernameRequired));

    app.login_form.username = "username sentinel".into();
    app.login_edited();

    assert_eq!(app.auth_error, None);
}

#[test]
fn a_normal_process_still_starts_a_proton_sign_in() {
    let (mut app, _) = App::boot(false, Settings::default());
    let _ = app.update(Message::SessionChecked(ResumeOutcome::SignedOut));
    app.login_form.username = "username sentinel".into();
    app.login_form.password = "password sentinel".to_owned().into();

    let effects = app.update(Message::Submit);

    assert_eq!(effects.units(), 1);
    assert_eq!(
        app.auth_state,
        AuthState::SigningIn(SignInStep::Credentials)
    );
    assert_eq!(app.auth_error, None);
}

#[test]
fn about_source_code_uses_the_background_browser_effect() {
    let (mut app, _) = App::boot(true, Settings::default());
    let _ = app.update(Message::ShowSettings(true));
    let settings = app.settings().clone();
    let effects = app.update(Message::OpenSourceCode);

    assert_eq!(effects.units(), 1);
    assert!(matches!(
        effects.into_iter().next(),
        Some(Effect::Background(_))
    ));
    assert!(app.showing_settings());
    assert_eq!(app.settings(), &settings);
}

#[test]
fn an_offline_start_keeps_the_saved_session_to_try_again() {
    let mut app = App::new(Settings::default());

    let _ = app.update(Message::SessionChecked(ResumeOutcome::Failed(
        AuthError::Connection,
    )));
    assert_eq!(app.auth_state, AuthState::SignedOut);
    assert_eq!(app.auth_error, Some(AuthError::Connection));
    assert!(app.can_retry_saved_session());

    let retry = app.update(Message::RetrySavedSession);
    assert_eq!(retry.units(), 1);
    assert_eq!(app.auth_state, AuthState::CheckingSession);
    assert_eq!(app.auth_error, None);
    assert!(!app.can_retry_saved_session());
    // A second press while it runs starts nothing more.
    assert_eq!(app.update(Message::RetrySavedSession).units(), 0);
}

#[test]
fn only_a_failure_that_may_pass_offers_the_saved_session_again() {
    for (outcome, retry) in [
        (ResumeOutcome::Failed(AuthError::TimedOut), true),
        (ResumeOutcome::Failed(AuthError::SessionUnavailable), true),
        (
            ResumeOutcome::Failed(AuthError::Service {
                http_status: 503,
                code: 0,
            }),
            true,
        ),
        (
            ResumeOutcome::Failed(AuthError::Service {
                http_status: 422,
                code: 2001,
            }),
            false,
        ),
        (
            ResumeOutcome::Failed(AuthError::SessionAddressMismatch),
            false,
        ),
        (ResumeOutcome::SignedOut, false),
    ] {
        let label = match &outcome {
            ResumeOutcome::Failed(error) => format!("{error:?}"),
            _ => "signed out".to_owned(),
        };
        let mut app = App::new(Settings::default());
        let _ = app.update(Message::SessionChecked(outcome));
        assert_eq!(app.can_retry_saved_session(), retry, "{label}");
    }
}

#[test]
fn signing_in_instead_leaves_nothing_to_retry_and_ignores_a_late_check() {
    let mut app = App::new(Settings::default());
    let _ = app.update(Message::SessionChecked(ResumeOutcome::Failed(
        AuthError::Connection,
    )));
    app.login_form.username = "username sentinel".into();
    app.login_form.password = "password sentinel".to_owned().into();

    let _ = app.update(Message::Submit);
    assert!(!app.can_retry_saved_session());
    assert_eq!(app.update(Message::RetrySavedSession).units(), 0);

    // A check that answers once a sign-in runs changes nothing.
    let _ = app.update(Message::SessionChecked(ResumeOutcome::SignedOut));
    assert_eq!(
        app.auth_state,
        AuthState::SigningIn(SignInStep::Credentials)
    );

    // Back on the form after that sign-in failed, its error is the one
    // shown, with no offer to try the old session beside it.
    let _ = app.update(Message::SignInFinished(
        app.auth_attempt,
        SignInOutcome::Failed(AuthError::Connection),
    ));
    assert_eq!(app.auth_state, AuthState::SignedOut);
    assert!(!app.can_retry_saved_session());
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
    app.login_form.password = "password sentinel".to_owned().into();
    app
}

#[test]
fn totp_prompt_is_answered_within_the_same_sign_in() {
    let mut app = signing_in_app();
    let (reply, mut answer) = crate::mail::PromptAnswer::channel();

    let _ = app.update(Message::SignInPrompt(
        app.auth_attempt,
        SignInPrompt::Totp(reply),
    ));
    assert_eq!(app.auth_state, AuthState::NeedsTotp);
    assert_eq!(app.login_form.username, "username sentinel");
    assert_eq!(app.login_form.password.as_str(), "password sentinel");

    let _ = app.update(Message::Submit);
    assert_eq!(app.auth_error, Some(AuthError::TotpRequired));
    assert!(matches!(answer.try_recv(), Ok(None)));

    *app.totp_mut() = " 123456 ".into();
    app.login_edited();
    let _ = app.update(Message::Submit);
    let code = answer.try_recv().unwrap().unwrap();
    assert_eq!(code.expose_secret(), "123456");
    assert_eq!(app.auth_state, AuthState::SigningIn(SignInStep::Totp));
    assert!(app.login_form.totp.is_empty());
}

#[test]
fn mailbox_password_prompt_is_answered_within_the_same_sign_in() {
    let mut app = signing_in_app();
    let attempt = app.auth_attempt;
    let (reply, mut answer) = crate::mail::PromptAnswer::channel();

    let _ = app.update(Message::SignInPrompt(
        attempt,
        SignInPrompt::MailboxPassword(reply),
    ));
    assert_eq!(app.auth_state, AuthState::NeedsMailboxPassword);
    assert_eq!(app.login_form.password.as_str(), "password sentinel");

    let _ = app.update(Message::Submit);
    assert_eq!(app.auth_error, Some(AuthError::MailboxPasswordRequired));
    assert!(matches!(answer.try_recv(), Ok(None)));

    *app.mailbox_password_mut() = " two words ".into();
    app.login_edited();
    let _ = app.update(Message::Submit);
    // Taken exactly as typed, spaces included.
    let password = answer.try_recv().unwrap().unwrap();
    assert_eq!(password.expose_secret(), " two words ");
    assert_eq!(
        app.auth_state,
        AuthState::SigningIn(SignInStep::MailboxPassword)
    );
    // The same sign-in goes on: no new attempt asks for the password or a
    // code again.
    assert_eq!(app.auth_attempt, attempt);
    assert!(app.login_form.mailbox_password.is_empty());
}

#[test]
fn human_verification_waits_for_confirmation() {
    let mut app = signing_in_app();
    let (done, mut answer) = crate::mail::PromptAnswer::channel();
    let url = "https://verify.proton.me/?methods=captcha&token=t".to_owned();

    let _ = app.update(Message::SignInPrompt(
        app.auth_attempt,
        SignInPrompt::HumanVerification {
            url: url.clone(),
            done,
        },
    ));
    assert_eq!(app.auth_state, AuthState::NeedsHumanVerification { url });
    assert_eq!(answer.try_recv(), Ok(None));

    let _ = app.update(Message::Submit);
    assert_eq!(answer.try_recv(), Ok(Some(())));
    assert!(matches!(app.auth_state, AuthState::SigningIn(_)));
}

/// A sign-in in its own `NeedsHumanVerification` step, and the answer the
/// browser check waits for.
fn verifying_app() -> (App, futures::channel::oneshot::Receiver<()>) {
    let mut app = signing_in_app();
    let (done, answer) = crate::mail::PromptAnswer::channel();
    let url = "https://verify.proton.me/?methods=captcha&token=t".to_owned();
    let _ = app.update(Message::SignInPrompt(
        app.auth_attempt,
        SignInPrompt::HumanVerification { url, done },
    ));
    (app, answer)
}

#[test]
fn a_sign_in_waiting_for_the_browser_check_is_still_running() {
    let (mut app, mut answer) = verifying_app();
    let effects = app.update(Message::CancelChallenge);
    assert_eq!(app.auth_state, AuthState::SignedOut);
    assert!(answer.try_recv().is_err());
    assert!(matches!(
        effects.into_iter().next(),
        Some(Effect::CancelSignIn)
    ));

    // Its outcome still applies while the check is open.
    let (mut app, _answer) = verifying_app();
    let _ = app.update(Message::SignInFinished(
        app.auth_attempt,
        SignInOutcome::Failed(AuthError::Connection),
    ));
    assert_eq!(app.auth_state, AuthState::SignedOut);
    assert_eq!(app.auth_error, Some(AuthError::Connection));
}

#[test]
fn going_back_cancels_the_pending_prompt() {
    let mut app = signing_in_app();
    let attempt = app.auth_attempt;
    let (reply, mut answer) = crate::mail::PromptAnswer::<SecretString>::channel();
    let _ = app.update(Message::SignInPrompt(attempt, SignInPrompt::Totp(reply)));

    let effects = app.update(Message::CancelChallenge);
    assert_eq!(app.auth_state, AuthState::SignedOut);
    assert!(app.login_form.password.is_empty());
    assert!(answer.try_recv().is_err());
    assert!(matches!(
        effects.into_iter().next(),
        Some(Effect::CancelSignIn)
    ));

    *app.password_mut() = "new password".into();
    let _ = app.update(Message::Submit);
    assert!(matches!(app.auth_state, AuthState::SigningIn(_)));
    assert_ne!(app.auth_attempt, attempt);

    let _ = app.update(Message::SignInFinished(
        attempt,
        SignInOutcome::Failed(AuthError::Connection),
    ));
    assert!(matches!(app.auth_state, AuthState::SigningIn(_)));
    assert_eq!(app.auth_error, None);
}

#[test]
fn prompts_without_a_running_sign_in_are_cancelled() {
    let mut app = App::signed_out();
    let (reply, mut answer) = crate::mail::PromptAnswer::<SecretString>::channel();

    let _ = app.update(Message::SignInPrompt(
        app.auth_attempt,
        SignInPrompt::Totp(reply),
    ));

    assert_eq!(app.auth_state, AuthState::SignedOut);
    assert!(answer.try_recv().is_err());
}

#[test]
fn terminal_sign_in_failure_clears_secrets() {
    let mut app = App::new(Settings::default());
    app.auth_state = AuthState::SigningIn(SignInStep::Credentials);
    app.login_form.password = "password sentinel".to_owned().into();
    app.login_form.totp = "totp sentinel".to_owned().into();
    app.login_form.mailbox_password = "mailbox password sentinel".to_owned().into();

    let _ = app.update(Message::SignInFinished(
        app.auth_attempt,
        SignInOutcome::Failed(AuthError::Connection),
    ));

    assert_eq!(app.auth_state, AuthState::SignedOut);
    assert!(app.login_form.password.is_empty());
    assert!(app.login_form.totp.is_empty());
    assert!(app.login_form.mailbox_password.is_empty());
}

#[test]
fn failed_local_logout_closes_mailbox_and_reports_remaining_credentials() {
    let mut app = authenticated_app();
    let _ = app.update(Message::OpenCompose);
    let _ = app.update(Message::ComposeChanged(
        ComposeField::Body,
        "private draft".to_owned(),
    ));
    app.auth_state = AuthState::SigningOut;

    deliver_logout(&mut app, Err(AuthError::LogoutUnavailable));

    assert_eq!(app.auth_state, AuthState::SignedOut);
    assert_eq!(app.auth_error, Some(AuthError::LogoutUnavailable));
    assert!(
        app.error_message()
            .unwrap()
            .contains("may remain on this device")
    );
    assert!(app.mailbox().is_none());
    assert!(app.compose().is_none());
}

#[test]
fn successful_logout_opens_login_and_clears_mailbox() {
    let mut app = authenticated_app();
    let _ = app.update(Message::OpenCompose);
    let _ = app.update(Message::ComposeChanged(
        ComposeField::Body,
        "private draft".to_owned(),
    ));
    app.auth_state = AuthState::SigningOut;

    deliver_logout(&mut app, Ok(()));

    assert_eq!(app.auth_state, AuthState::SignedOut);
    assert!(app.mailbox.is_none());
    assert!(app.compose().is_none());
}

#[test]
fn expired_session_signs_out_and_clears_mailbox() {
    let mut app = authenticated_app();
    let _ = app.update(Message::OpenCompose);
    let _ = app.update(Message::ComposeChanged(
        ComposeField::Body,
        "private draft".to_owned(),
    ));

    let _ = app.update(Message::ConversationsLoaded(
        OPENING_PAGE,
        Err(MailboxError::SessionExpired),
    ));

    assert_eq!(app.auth_state, AuthState::SignedOut);
    assert_eq!(app.auth_error, Some(AuthError::SessionExpired));
    assert!(app.mailbox.is_none());
    assert!(app.compose().is_none());
}

#[test]
fn a_session_that_cannot_be_read_keeps_the_mailbox_and_the_draft() {
    let mut app = authenticated_app();
    let _ = app.update(Message::OpenCompose);
    let _ = app.update(Message::ComposeChanged(
        ComposeField::Body,
        "private draft".to_owned(),
    ));

    let _ = app.update(Message::ConversationsLoaded(
        OPENING_PAGE,
        Err(MailboxError::SessionUnavailable),
    ));

    // Unlocking the password storage fixes the next request.
    assert_eq!(app.auth_state, AuthState::Authenticated { email: None });
    assert!(app.mailbox.is_some());
    assert!(app.compose().is_some());
}

/// Ends the session of account "user-1" the way Proton does, from its first
/// page request, while `app` holds whatever it was given.
fn session_ended_by_proton(app: &mut App) {
    let _ = app.update(Message::ConversationsLoaded(
        OPENING_PAGE,
        Err(MailboxError::SessionExpired),
    ));
}

/// Signs account `account` in again, with the demo mailbox standing in.
fn signed_in_again(app: &mut App, account: &str) -> Effects {
    app.open_mailbox(
        MailBackend::demo(),
        Some("alex@proton.me".into()),
        Some(account.into()),
    )
}

#[test]
fn an_expired_session_keeps_the_draft_for_its_own_account() {
    let mut app = App::signed_in_as("user-1", "alex@proton.me");
    write(&mut app, "sam@example.com");
    let written = app.compose().unwrap().id();
    session_ended_by_proton(&mut app);

    assert_eq!(app.auth_state, AuthState::SignedOut);
    assert_eq!(app.auth_error, Some(AuthError::SessionExpired));
    assert!(app.compose().is_none(), "no session shows it meanwhile");
    assert_eq!(app.parked_draft_address(), Some("alex@proton.me"));

    let _ = signed_in_again(&mut app, "user-1");
    let draft = app.compose().expect("its own account gets it back");
    assert_eq!(draft.field(ComposeField::To), "sam@example.com");
    assert_eq!(draft.field(ComposeField::Body), "See you then.");
    assert_ne!(
        draft.id(),
        written,
        "nothing from the ended session reaches it"
    );
    assert!(!app.has_parked_draft());
}

#[test]
fn another_account_or_none_never_gets_a_kept_draft() {
    for next in [Some("user-2"), None] {
        let mut app = App::signed_in_as("user-1", "alex@proton.me");
        write(&mut app, "sam@example.com");
        session_ended_by_proton(&mut app);

        let _ = app.open_mailbox(MailBackend::demo(), None, next.map(str::to_owned));
        assert!(app.compose().is_none(), "{next:?}");
        assert!(!app.has_parked_draft(), "{next:?}");
        // Nor does its own account afterwards.
        app.close_mailbox(None);
        let _ = signed_in_again(&mut app, "user-1");
        assert!(app.compose().is_none(), "{next:?}");
    }
}

#[test]
fn an_untouched_draft_or_a_sign_out_keeps_nothing() {
    let mut app = App::signed_in_as("user-1", "alex@proton.me");
    let _ = app.update(Message::OpenCompose);
    session_ended_by_proton(&mut app);
    assert!(!app.has_parked_draft(), "nothing was written");

    let _ = signed_in_again(&mut app, "user-1");
    write(&mut app, "sam@example.com");
    app.close_mailbox(Some(AuthError::Connection));
    assert!(
        !app.has_parked_draft(),
        "only Proton ending the session keeps it"
    );
}

#[test]
fn a_message_on_its_way_when_the_session_ends_comes_back_unconfirmed() {
    let mut app = App::signed_in_as("user-1", "alex@proton.me");
    write(&mut app, "sam@example.com");
    let ended = app.session_epoch;
    assert_eq!(app.update(Message::Send).units(), 1);
    session_ended_by_proton(&mut app);

    let _ = signed_in_again(&mut app, "user-1");
    // Its answer belonged to the session that ended, and changes nothing.
    let _ = app.update(Message::Sent(ended, Ok(())));
    let draft = app.compose().expect("it may not have gone");
    assert_eq!(draft.sending(), Sending::Failed(SendError::Unconfirmed));
}

#[test]
fn a_send_refused_for_an_expired_session_ends_it_and_keeps_the_draft() {
    let mut app = App::signed_in_as("user-1", "alex@proton.me");
    write(&mut app, "sam@example.com");
    let epoch = app.session_epoch;
    let _ = app.update(Message::Send);

    let _ = app.update(Message::Sent(
        epoch,
        Err(SendError::Mailbox(MailboxError::SessionExpired)),
    ));
    assert_eq!(app.auth_state, AuthState::SignedOut);
    assert_eq!(app.auth_error, Some(AuthError::SessionExpired));

    let _ = signed_in_again(&mut app, "user-1");
    let draft = app.compose().expect("signing in again brings it back");
    assert_eq!(draft.sending(), Sending::Writing, "ready to send again");
}

#[test]
fn discarding_drops_a_kept_draft() {
    let mut app = App::signed_in_as("user-1", "alex@proton.me");
    write(&mut app, "sam@example.com");
    session_ended_by_proton(&mut app);

    let _ = app.update(Message::DiscardCompose);
    assert!(!app.has_parked_draft());
    let _ = signed_in_again(&mut app, "user-1");
    assert!(app.compose().is_none());
}

#[test]
fn an_attachment_save_that_finds_the_session_expired_signs_out() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::SaveAttachment("demo-0".into(), "file".into()));

    deliver_attachment(&mut app, Err(SaveError::SessionExpired));

    assert_eq!(app.auth_state, AuthState::SignedOut);
    assert_eq!(app.auth_error, Some(AuthError::SessionExpired));
    assert!(app.mailbox.is_none());
}

#[test]
fn a_failed_download_says_why_the_file_was_not_saved() {
    for (error, saved) in [
        (
            MailboxError::AttachmentVerificationFailed,
            SaveError::VerificationFailed,
        ),
        (MailboxError::SessionExpired, SaveError::SessionExpired),
        (
            MailboxError::SessionUnavailable,
            SaveError::SessionUnavailable,
        ),
        (MailboxError::Connection, SaveError::NotFetched),
        (MailboxError::Service, SaveError::NotFetched),
        (MailboxError::Unavailable, SaveError::NotFetched),
    ] {
        assert_eq!(super::unsaved(error), saved, "{error:?}");
    }
}

#[test]
fn stale_expired_session_response_is_ignored() {
    let mut app = authenticated_app();

    let _ = app.update(Message::ConversationsLoaded(
        NEVER_ISSUED,
        Err(MailboxError::SessionExpired),
    ));

    assert_eq!(app.auth_state, AuthState::Authenticated { email: None });
    assert!(app.mailbox.is_some());
}

#[test]
fn logout_result_after_session_expiry_is_ignored() {
    let mut app = authenticated_app();
    let previous_epoch = app.session_epoch;
    app.auth_state = AuthState::SigningOut;
    let _ = app.update(Message::CountsLoaded(
        OPENING_COUNTS,
        Err(MailboxError::SessionExpired),
    ));

    let _ = app.update(Message::LogoutFinished(
        previous_epoch,
        Err(AuthError::Connection),
    ));

    assert_eq!(app.auth_state, AuthState::SignedOut);
    assert_eq!(app.auth_error, Some(AuthError::SessionExpired));
}

#[test]
fn logout_result_from_a_previous_session_is_ignored() {
    let mut app = authenticated_app();
    let previous_epoch = app.session_epoch;
    app.auth_state = AuthState::SigningOut;
    let _ = app.update(Message::CountsLoaded(
        OPENING_COUNTS,
        Err(MailboxError::SessionExpired),
    ));
    let _ = app.open_mailbox(MailBackend::demo(), None, None);
    app.auth_state = AuthState::SigningOut;

    let _ = app.update(Message::LogoutFinished(previous_epoch, Ok(())));

    assert_eq!(app.auth_state, AuthState::SigningOut);
    assert!(app.mailbox().is_some());
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

    deliver_send(&mut app, Ok(()));
    assert!(
        app.compose().is_none(),
        "the window stays open after sending"
    );
}

#[test]
fn send_result_from_a_previous_session_keeps_the_new_draft() {
    let mut app = loaded_demo_app();
    let previous_epoch = app.session_epoch;
    write(&mut app, "old@example.com");
    let _ = app.update(Message::Send);
    // Signing out waits for the send; an expired session cannot.
    app.close_mailbox(Some(AuthError::SessionExpired));

    let _ = app.open_mailbox(MailBackend::demo(), None, None);
    write(&mut app, "new@example.com");
    let _ = app.update(Message::Sent(previous_epoch, Ok(())));

    let compose = app.compose().expect("the new session keeps its draft");
    assert_eq!(compose.field(ComposeField::To), "new@example.com");
}

#[test]
fn signing_out_never_takes_a_written_or_sending_message_with_it() {
    let mut app = loaded_demo_app();
    write(&mut app, "alex@example.com");

    // The desktop asks first and discards the message itself.
    assert_eq!(app.update(Message::Logout).units(), 0);
    assert!(matches!(app.auth_state, AuthState::Authenticated { .. }));
    assert!(app.compose().is_some());

    // A send's outcome must reach this session.
    let _ = app.update(Message::Send);
    assert_eq!(app.update(Message::Logout).units(), 0);
    assert!(matches!(app.auth_state, AuthState::Authenticated { .. }));
    assert!(app.compose().unwrap().in_flight());

    deliver_send(&mut app, Ok(()));
    let _ = app.update(Message::Logout);
    assert_eq!(app.auth_state, AuthState::SignedOut);
}

#[test]
fn sending_revalidates_a_cached_sent_folder() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::SelectFolder(sys(MailFolder::Sent)));
    deliver_latest_demo_page(&mut app, 0);
    assert_eq!(
        app.update(Message::SelectFolder(sys(MailFolder::Inbox)))
            .units(),
        0
    );

    write(&mut app, "alex@example.com");
    let _ = app.update(Message::Send);
    deliver_send(&mut app, Ok(()));

    assert_eq!(
        app.update(Message::SelectFolder(sys(MailFolder::Sent)))
            .units(),
        1
    );
    assert!(matches!(
        app.mailbox().unwrap().status(),
        ListStatus::Refreshing(_)
    ));
    assert!(!app.mailbox().unwrap().conversations().is_empty());
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

    deliver_send(&mut app, Err(SendError::DemoLimitReached));

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
    let _ = app.update(Message::OpenCompose);
    press(&mut app, Key::Escape);
    assert!(
        app.compose().is_none(),
        "untouched compose is put away immediately"
    );

    write(&mut app, "alex@example.com");
    press(&mut app, Key::Escape);
    assert!(
        app.compose().is_some_and(|c| c.confirming_discard()),
        "written compose asks for confirmation before being put away"
    );
    let _ = app.update(Message::DiscardCompose);
    assert!(
        app.compose().is_none(),
        "confirming discard puts message away"
    );
}

#[test]
fn a_reply_needs_nobody_named_because_proton_knows() {
    // Core addresses a reply from the message being answered, so the window
    // must not refuse to send for want of a recipient.
    let mut app = loaded_demo_app();
    open_conversation(&mut app, "demo-0");

    let _ = app.update(Message::Answer {
        message_id: "demo-0-2".into(),
        kind: AnswerKind::Reply,
    });

    let writing = app.compose().expect("a reply is open");
    assert!(!writing.asks_for_recipients());
    assert!(!writing.asks_for_subject());
    assert_eq!(app.update(Message::Send).units(), 1);
}

#[test]
fn a_forward_still_has_to_be_addressed() {
    let mut app = loaded_demo_app();
    open_conversation(&mut app, "demo-0");

    let _ = app.update(Message::Answer {
        message_id: "demo-0-2".into(),
        kind: AnswerKind::Forward,
    });

    let writing = app.compose().expect("a forward is open");
    assert!(writing.asks_for_recipients());
    // Core writes the subject, so the window does not ask for one.
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
    open_conversation(&mut app, "demo-0");

    let _ = app.update(Message::Answer {
        message_id: "demo-0-2".into(),
        kind: AnswerKind::ReplyAll,
    });

    let answering = app
        .compose()
        .and_then(Compose::answering)
        .expect("it says what it answers");
    assert_eq!(answering.kind, AnswerKind::ReplyAll);
    assert!(!answering.subject.is_empty());
    assert!(!answering.sender.is_empty());
}

#[test]
fn answering_a_message_that_is_not_there_opens_nothing() {
    let mut app = loaded_demo_app();

    let _ = app.update(Message::Answer {
        message_id: "demo-0-2".into(),
        kind: AnswerKind::Reply,
    });

    assert!(app.compose().is_none());
}

fn inspection_page(app: &App, count: usize) -> ConversationPage {
    let template = app
        .mailbox()
        .unwrap()
        .visible_conversations()
        .next()
        .unwrap();
    let conversations: Vec<_> = (0..count)
        .map(|i| ConversationSummary {
            id: format!("candidate-{i}"),
            ..template.clone()
        })
        .collect();
    ConversationPage {
        inspect_candidates: conversations.iter().map(|row| row.id.clone()).collect(),
        conversations,
        total: count as u32,
    }
}

#[test]
fn inspection_candidates_do_not_spawn_an_unbounded_number_of_tasks() {
    let mut app = loaded_demo_app();
    // Twice as many candidates as the queue takes, each offered twice.
    let mut page = inspection_page(&app, 2 * inspection::CAPACITY);
    page.inspect_candidates
        .extend(page.inspect_candidates.clone());
    let _ = app.update(Message::RefreshMailbox);
    let request = app.last_request - 1;
    let effects = app.update(Message::ConversationsLoaded(request, Ok(page)));
    assert_eq!(effects.units(), INSPECTION_CONCURRENCY);
}

#[test]
fn stale_page_response_does_not_start_inspections() {
    let mut app = loaded_demo_app();
    let page = inspection_page(&app, 2);
    let effects = app.update(Message::ConversationsLoaded(NEVER_ISSUED, Ok(page)));
    assert_eq!(effects.units(), 0);
}

#[test]
fn inspection_queue_drains_with_bounded_deduplicated_work() {
    let mut app = loaded_demo_app();
    // Twice as many candidates as the queue takes, each offered twice.
    let mut page = inspection_page(&app, 2 * inspection::CAPACITY);
    page.inspect_candidates
        .extend(page.inspect_candidates.clone());
    let _ = app.update(Message::RefreshMailbox);
    let request = app.last_request - 1;
    let effects = app.update(Message::ConversationsLoaded(request, Ok(page)));
    let mut running: std::collections::VecDeque<_> = effects.into_iter().collect();
    let mut completed = std::collections::HashSet::new();
    while let Some(effect) = running.pop_front() {
        let Effect::Future(future) = effect else {
            panic!("expected an inspection")
        };
        let message = futures::executor::block_on(future);
        let Message::ThreadInspected {
            conversation_id, ..
        } = &message
        else {
            panic!("expected an inspection result");
        };
        assert!(
            completed.insert(conversation_id.clone()),
            "duplicate candidate"
        );
        let next = app.update(message.clone());
        assert!(next.units() <= 1);
        // A duplicate completion cannot release another slot or launch work.
        assert_eq!(app.update(message).units(), 0);
        running.extend(next.into_iter());
        assert!(running.len() <= INSPECTION_CONCURRENCY);
    }
    assert_eq!(
        completed.len(),
        inspection::CAPACITY,
        "overflow must keep Proton grouping"
    );
}

#[test]
fn inspections_are_cancelled_when_the_context_changes() {
    for (case, transition) in [
        (
            "another folder",
            Message::SelectFolder(sys(MailFolder::Sent)),
        ),
        ("a search", Message::SearchSubmitted),
        ("a refresh", Message::RefreshMailbox),
        ("signing out", Message::Logout),
    ] {
        let mut app = loaded_demo_app();
        let effects = app.inspect_candidates(vec!["demo-0".into()]);
        let request = app.last_request;
        let Effect::Future(future) = effects.into_iter().next().unwrap() else {
            panic!("expected an inspection");
        };
        if matches!(transition, Message::SearchSubmitted) {
            let _ = app.update(Message::SearchChanged("mail".into()));
        }
        let _ = app.update(transition);
        let mut message = futures::executor::block_on(future);
        // Even a success or session error already queued before cancellation
        // must be discarded, without starting the next candidate.
        let Message::ThreadInspected { result, .. } = &mut message else {
            panic!("expected an inspection result");
        };
        *result = Err(MailboxError::SessionExpired);
        let expected_auth = app.auth_state.clone();
        assert_eq!(app.update(message).units(), 0, "{case}");
        assert_eq!(app.auth_state, expected_auth, "{case}");
        assert!(!app.inspections.finish(request, "demo-0"), "{case}");
    }
}

#[test]
fn returning_to_a_folder_does_not_accept_its_cancelled_inspection() {
    let mut app = loaded_demo_app();
    let _ = app.inspect_candidates(vec!["demo-0".into()]);
    let old_request = app.last_request;
    let _ = app.update(Message::SelectFolder(sys(MailFolder::Sent)));
    let _ = app.update(Message::SelectFolder(Folder::INBOX));
    let _ = app.inspect_candidates(vec!["demo-0".into()]);
    let current_request = app.last_request;
    let effects = app.update(Message::ThreadInspected {
        revision: app.mailbox().map_or(0, Mailbox::inspection_revision),
        epoch: app.session_epoch,
        request: old_request,
        folder: Folder::INBOX,
        conversation_id: "demo-0".into(),
        result: Err(MailboxError::SessionExpired),
    });
    assert_eq!(effects.units(), 0);
    assert!(matches!(app.auth_state, AuthState::Authenticated { .. }));
    assert!(app.inspections.finish(current_request, "demo-0"));
}

#[test]
fn conversations_loaded_with_candidates_starts_inspection_effects() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::RefreshMailbox);
    let request = app.last_request - 1;

    let page = ConversationPage {
        conversations: vec![ConversationSummary {
            id: "demo-cand-1".into(),
            kind: crate::mail::SummaryKind::Conversation,
            subject: Some("Candidate 1".into()),
            correspondents: None,
            participants: Vec::new(),
            preview: None,
            time: Some(100),
            unread: false,
            starred: false,
            message_count: 2,
            has_attachments: false,
        }],
        total: 1,
        inspect_candidates: vec!["demo-cand-1".into(), "demo-cand-2".into()],
    };

    let effects = app.update(Message::ConversationsLoaded(request, Ok(page)));
    // Only a candidate present in the accepted listing may be inspected.
    assert_eq!(effects.units(), 1);
}

#[test]
fn verified_split_conversations_can_be_reinspected_without_regrouping_rows() {
    let mut app = loaded_demo_app();
    let rows = vec![ConversationSummary {
        id: "split-member".into(),
        kind: crate::mail::SummaryKind::Message,
        ..app.mailbox().unwrap().visible_row(0).unwrap().clone()
    }];
    app.mailbox
        .as_mut()
        .unwrap()
        .split_conversation(&Folder::INBOX, "demo-0", rows);
    assert!(app.mailbox().unwrap().has_row("split-member"));
    assert!(!app.mailbox().unwrap().has_row("demo-0"));
    assert_eq!(app.inspect_candidates(vec!["demo-0".into()]).units(), 1);
    assert!(app.mailbox().unwrap().has_row("split-member"));
}

#[test]
fn inspection_before_a_confirmed_mutation_cannot_replace_rows_and_releases_its_slot() {
    let mut app = loaded_demo_app();
    let effects = app.inspect_candidates((0..5).map(|index| format!("demo-{index}")).collect());
    let Effect::Future(future) = effects.into_iter().next().unwrap() else {
        panic!("expected an inspection");
    };
    let mut message = futures::executor::block_on(future);
    let stale = ConversationSummary {
        id: "stale-member".into(),
        kind: crate::mail::SummaryKind::Message,
        ..app.mailbox().unwrap().visible_row(0).unwrap().clone()
    };
    let Message::ThreadInspected {
        result, revision, ..
    } = &mut message
    else {
        panic!("expected an inspection result");
    };
    *result = Ok(Some(vec![stale]));
    let previous = *revision;
    let mailbox = app.mailbox.as_mut().unwrap();
    mailbox.record_action("demo-0", &MailAction::SetUnread(false));
    mailbox.invalidate_listings();
    let current = mailbox.inspection_revision();
    assert!(current > previous);

    let next = app.update(message);
    assert!(app.mailbox().unwrap().has_row("demo-0"));
    assert!(!app.mailbox().unwrap().has_row("stale-member"));
    assert_eq!(
        next.units(),
        1,
        "the next candidate can use the released slot"
    );
    let Effect::Future(future) = next.into_iter().next().unwrap() else {
        panic!("expected the next inspection");
    };
    assert!(
        matches!(futures::executor::block_on(future), Message::ThreadInspected { revision, .. } if revision == current)
    );
}

#[test]
fn thread_inspected_splits_conversation_when_successful() {
    let mut app = loaded_demo_app();
    let epoch = app.session_epoch;
    let _ = app.inspect_candidates(vec!["demo-0".into()]);
    let request = app.last_request;

    let split_rows = vec![
        ConversationSummary {
            id: "split-1".into(),
            kind: crate::mail::SummaryKind::Message,
            subject: Some("Split 1".into()),
            correspondents: None,
            participants: Vec::new(),
            preview: None,
            time: Some(150),
            unread: false,
            starred: false,
            message_count: 1,
            has_attachments: false,
        },
        ConversationSummary {
            id: "split-2".into(),
            kind: crate::mail::SummaryKind::Message,
            subject: Some("Split 2".into()),
            correspondents: None,
            participants: Vec::new(),
            preview: None,
            time: Some(140),
            unread: false,
            starred: false,
            message_count: 1,
            has_attachments: false,
        },
    ];

    let _ = app.update(Message::ThreadInspected {
        revision: app.mailbox().map_or(0, Mailbox::inspection_revision),
        epoch,
        request,
        folder: Folder::INBOX,
        conversation_id: "demo-0".into(),
        result: Ok(Some(split_rows)),
    });

    let visible: Vec<_> = app
        .mailbox()
        .unwrap()
        .visible_conversations()
        .map(|c| c.id.as_str())
        .collect();
    assert!(visible.contains(&"split-1"));
    assert!(visible.contains(&"split-2"));
    assert!(!visible.contains(&"demo-0"));
}

#[test]
fn thread_inspected_stale_epoch_or_folder_is_ignored() {
    let mut app = loaded_demo_app();
    let stale_epoch = app.session_epoch + 99;
    let _ = app.inspect_candidates(vec!["demo-0".into()]);
    let request = app.last_request;

    let split_rows = vec![ConversationSummary {
        id: "split-ignored".into(),
        kind: crate::mail::SummaryKind::Message,
        subject: Some("Split Ignored".into()),
        correspondents: None,
        participants: Vec::new(),
        preview: None,
        time: Some(150),
        unread: false,
        starred: false,
        message_count: 1,
        has_attachments: false,
    }];

    // Wrong epoch
    let _ = app.update(Message::ThreadInspected {
        revision: app.mailbox().map_or(0, Mailbox::inspection_revision),
        epoch: stale_epoch,
        request,
        folder: Folder::INBOX,
        conversation_id: "demo-0".into(),
        result: Ok(Some(split_rows.clone())),
    });

    let visible: Vec<_> = app
        .mailbox()
        .unwrap()
        .visible_conversations()
        .map(|c| c.id.as_str())
        .collect();
    assert!(!visible.contains(&"split-ignored"));
    assert!(visible.contains(&"demo-0"));

    // Wrong folder
    let _ = app.update(Message::ThreadInspected {
        revision: app.mailbox().map_or(0, Mailbox::inspection_revision),
        epoch: app.session_epoch,
        request,
        folder: Folder::System(crate::mail::MailFolder::Trash),
        conversation_id: "demo-0".into(),
        result: Ok(Some(split_rows)),
    });

    let visible: Vec<_> = app
        .mailbox()
        .unwrap()
        .visible_conversations()
        .map(|c| c.id.as_str())
        .collect();
    assert!(!visible.contains(&"split-ignored"));
    assert!(visible.contains(&"demo-0"));
}

#[tokio::test]
async fn sending_with_missing_attachment_fails_gracefully_in_background() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::OpenCompose);
    let _ = app.update(Message::ComposeChanged(
        ComposeField::To,
        "alex@example.com".into(),
    ));
    let _ = app.update(Message::ComposeChanged(ComposeField::Body, "Hello".into()));
    let non_existent = PathBuf::from("/tmp/missing_attachment_test_ruston.pdf");
    let compose_id = app.compose().unwrap().id();
    let _ = app.update(Message::AddComposeAttachments(
        compose_id,
        vec![non_existent],
    ));

    // In-memory UI readiness must be immediate and non-blocking
    assert!(app.compose().unwrap().not_ready().is_none());

    // User triggers Send
    let effects = app.update(Message::Send);
    assert_eq!(effects.units(), 1);
    assert!(app.compose().unwrap().in_flight());

    // Execute background effect
    let effect = effects.into_iter().next().unwrap();
    let Effect::Future(future) = effect else {
        panic!("expected future effect");
    };
    let message = future.await;
    let _ = app.update(message);

    // Compose should transition to Failed with MissingAttachment, preserving draft content
    let sending = app.compose().unwrap().sending();
    assert_eq!(
        sending,
        Sending::Failed(SendError::MissingAttachment(
            "missing_attachment_test_ruston.pdf".to_owned()
        ))
    );
    assert_eq!(app.compose().unwrap().field(ComposeField::Body), "Hello");

    // Removing the missing attachment clears the refusal state back to Writing
    let _ = app.update(Message::RemoveComposeAttachment(0));
    assert_eq!(app.compose().unwrap().sending(), Sending::Writing);
    assert!(app.compose().unwrap().attachments().is_empty());
}
