//! Settings: changes and their writes, the pane, About, and the layout.

use super::*;

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
fn signing_out_closes_the_settings_pane() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::ShowSettings(true));

    let _ = app.update(Message::Logout);

    // Signing back in must not restore the previous account's Settings view.
    assert!(!app.showing_settings());
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
fn third_party_notices_open_in_the_background_only_when_shipped() {
    let (mut app, _) = App::boot(true, Settings::default());
    let _ = app.update(Message::ShowSettings(true));
    app.third_party_notices = None;
    assert_eq!(app.update(Message::OpenThirdPartyNotices).units(), 0);

    app.third_party_notices = Some(PathBuf::from("THIRD_PARTY_LICENSES.txt"));
    let effects = app.update(Message::OpenThirdPartyNotices);

    assert_eq!(effects.units(), 1);
    assert!(matches!(
        effects.into_iter().next(),
        Some(Effect::Background(_))
    ));
    assert!(app.showing_settings());
}
