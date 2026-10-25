use super::*;

#[test]
fn email_link_waits_for_file_settings_and_close_guards_without_losing_the_request() {
    let context = egui::Context::default();
    let mut desktop = DesktopApp::with_context(&context, true, Settings::default());
    desktop.dispatch(
        Message::ReceiveMailto(
            crate::mailto::Request::parse("mailto:team@example.org?body=Hello").unwrap(),
        ),
        &context,
    );
    desktop.file_dialog_open = true;
    desktop.focus_after_activation = true;
    desktop.focus_activated_composer(&context);
    assert!(desktop.focus_after_activation);
    desktop.open_waiting_mailto(&context);
    assert!(desktop.app.compose().is_none());
    desktop.file_dialog_open = false;
    desktop.pending_discard = Some(close::Leaving::Window);
    desktop.open_waiting_mailto(&context);
    assert!(desktop.app.compose().is_none());
    desktop.pending_discard = None;
    desktop.pending_settings_exit = Some(SettingsExit::CloseWindow);
    desktop.open_waiting_mailto(&context);
    assert!(desktop.app.compose().is_none());
    desktop.pending_settings_exit = None;
    desktop.dispatch(Message::ShowSettings(true), &context);
    desktop.open_waiting_mailto(&context);
    assert!(desktop.app.compose().is_none());
    desktop.dispatch(Message::ShowSettings(false), &context);
    desktop.open_waiting_mailto(&context);
    assert_eq!(
        desktop
            .app
            .compose()
            .unwrap()
            .field(crate::app::ComposeField::To),
        "team@example.org"
    );
    assert_eq!(desktop.ui.compact_view, mailbox::CompactView::Reader);
    assert_eq!(desktop.app.pending_mailto_count(), 0);
    desktop.focus_activated_composer(&context);
    assert!(!desktop.focus_after_activation);
}

fn native_close(desktop: &mut DesktopApp, context: &egui::Context) -> Vec<egui::ViewportCommand> {
    let mut raw = egui::RawInput::default();
    let viewport = raw.viewports.get_mut(&egui::ViewportId::ROOT).unwrap();
    viewport.minimized = Some(true);
    viewport.events.push(egui::ViewportEvent::Close);
    let mut output = context.run_logic(&raw, |context| desktop.check_window_close(context));
    output
        .viewport_commands
        .remove(&egui::ViewportId::ROOT)
        .unwrap_or_default()
}

/// A demo window whose mailbox has one change on its way, as Proton's
/// would be, and that change.
fn window_with_a_change_on_its_way(
    context: &egui::Context,
) -> (DesktopApp, crate::app::ActionRequest) {
    let mut desktop = DesktopApp::with_context(context, true, Settings::default());
    let page = crate::mail::demo::DemoMailbox::new().list_conversations(
        &Folder::INBOX,
        0,
        crate::mail::demo::PAGE_SIZE,
        crate::mail::demo::now(),
    );
    desktop.dispatch(Message::ConversationsLoaded(1, page), context);
    let sent = desktop
        .app
        .mailbox_mut()
        .unwrap()
        .start_action_on(
            "demo-0".into(),
            crate::mail::SummaryKind::Conversation,
            crate::mail::MailAction::SetStarred(true),
            900,
        )
        .unwrap()
        .send
        .unwrap();
    (desktop, sent)
}

#[test]
fn closing_waits_for_changes_on_their_way_to_proton() {
    let context = egui::Context::default();
    let (mut desktop, sent) = window_with_a_change_on_its_way(&context);

    let commands = native_close(&mut desktop, &context);
    assert!(commands.contains(&egui::ViewportCommand::CancelClose));
    assert!(!desktop.window_close_approved);

    // Once Proton has it, the close goes ahead by itself.
    let _ = desktop
        .app
        .mailbox_mut()
        .unwrap()
        .finish_action(&sent, Ok(()));
    let mut output = context.run_logic(&egui::RawInput::default(), |context| {
        desktop.check_window_close(context);
    });
    let commands = output
        .viewport_commands
        .remove(&egui::ViewportId::ROOT)
        .unwrap_or_default();
    assert!(commands.contains(&egui::ViewportCommand::Close));
    assert!(desktop.window_close_approved);
}

#[test]
fn closing_again_while_changes_are_on_their_way_closes_at_once() {
    let context = egui::Context::default();
    let (mut desktop, _) = window_with_a_change_on_its_way(&context);

    assert!(native_close(&mut desktop, &context).contains(&egui::ViewportCommand::CancelClose));
    assert!(native_close(&mut desktop, &context).contains(&egui::ViewportCommand::Close));
    assert!(desktop.window_close_approved);
}

#[test]
fn settled_settings_are_written_off_the_interface() {
    let context = egui::Context::default();
    let mut desktop = DesktopApp::with_context(&context, true, Settings::default());
    desktop.dispatch(Message::SetConfirmLinks(false), &context);
    assert!(desktop.app.settings_unsaved());

    // The change settles, and the frame after the quiet time writes it.
    for time in [0.0, f64::from(SETTINGS_QUIET) + 0.1] {
        let input = egui::RawInput {
            time: Some(time),
            ..Default::default()
        };
        context
            .run_ui(input, |ui| desktop.save_settled_settings(ui.ctx()))
            .drop_without_applying_deltas();
    }
    // The write's result comes back through the runtime.
    for _ in 0..200 {
        desktop.drain_runtime(&context);
        if !desktop.app.settings_unsaved() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("the settled change was never written");
}

#[test]
fn closing_while_signing_out_waits_until_the_session_is_forgotten() {
    let context = egui::Context::default();
    let mut desktop = DesktopApp::with_context(&context, true, Settings::default());
    desktop.app.set_auth_state(AuthState::SigningOut);

    let commands = native_close(&mut desktop, &context);
    assert!(commands.contains(&egui::ViewportCommand::CancelClose));
    assert!(!desktop.window_close_approved);

    // The sign-out ends: this device has forgotten the session.
    desktop.app.set_auth_state(AuthState::SignedOut);
    let mut output = context.run_logic(&egui::RawInput::default(), |context| {
        desktop.check_window_close(context);
    });
    let commands = output
        .viewport_commands
        .remove(&egui::ViewportId::ROOT)
        .unwrap_or_default();
    assert!(commands.contains(&egui::ViewportCommand::Close));
    assert!(desktop.window_close_approved);
}

fn signed_in(desktop: &DesktopApp) -> bool {
    desktop.app.auth_state().is_authenticated()
}

#[test]
fn signing_out_waits_for_changes_on_their_way_to_proton() {
    let context = egui::Context::default();
    let (mut desktop, sent) = window_with_a_change_on_its_way(&context);

    // In the demo, signing out is leaving it; it waits the same way.
    desktop.dispatch(Message::Logout, &context);
    assert!(signed_in(&desktop));
    assert_eq!(desktop.deferred_leave.waiting(), Some(close::Leaving::Demo));

    // Once Proton has the change, the sign-out goes ahead by itself.
    let _ = desktop
        .app
        .mailbox_mut()
        .unwrap()
        .finish_action(&sent, Ok(()));
    let _ = context.run_logic(&egui::RawInput::default(), |context| {
        desktop.check_window_close(context);
    });
    assert!(!signed_in(&desktop));
    assert_eq!(desktop.deferred_leave.waiting(), None);
}

#[test]
fn signing_out_again_while_changes_are_on_their_way_signs_out_at_once() {
    let context = egui::Context::default();
    let (mut desktop, _) = window_with_a_change_on_its_way(&context);

    desktop.dispatch(Message::Logout, &context);
    assert!(signed_in(&desktop));
    desktop.dispatch(Message::Logout, &context);
    assert!(!signed_in(&desktop));
}

#[test]
fn main_window_close_requires_explicit_discard_in_both_composer_placements() {
    for placement in [ComposePlacement::ReadingPane, ComposePlacement::Window] {
        let context = egui::Context::default();
        let mut desktop = DesktopApp::with_context(&context, true, Settings::default());
        desktop.dispatch(Message::SetComposePlacement(placement), &context);
        desktop.dispatch(Message::OpenCompose, &context);
        desktop.dispatch(
            Message::ComposeChanged(crate::app::ComposeField::Body, "Keep this draft".into()),
            &context,
        );
        for _ in 0..2 {
            assert!(
                native_close(&mut desktop, &context).contains(&egui::ViewportCommand::CancelClose)
            );
            assert_eq!(desktop.pending_discard, Some(close::Leaving::Window));
            assert!(!desktop.window_close_approved);
            assert_eq!(
                desktop
                    .app
                    .compose()
                    .unwrap()
                    .field(crate::app::ComposeField::Body),
                "Keep this draft"
            );
        }
        desktop.resolve_discard(Message::CancelDiscard, &context);
        assert_eq!(desktop.pending_discard, None);
        assert!(desktop.app.compose().is_some());
        native_close(&mut desktop, &context);
        desktop.resolve_discard(Message::DiscardCompose, &context);
        assert!(desktop.app.compose().is_none());
        assert!(desktop.window_close_approved);
    }
}

#[test]
fn closing_asks_before_dropping_a_draft_kept_for_its_account() {
    let context = egui::Context::default();
    let mut desktop = DesktopApp::with_context(&context, true, Settings::default());
    desktop.app = App::signed_in_as("user-1", "alex@proton.me");
    desktop.dispatch(Message::OpenCompose, &context);
    desktop.dispatch(
        Message::ComposeChanged(crate::app::ComposeField::Body, "Keep this draft".into()),
        &context,
    );
    desktop.dispatch(
        Message::ConversationsLoaded(1, Err(crate::mail::MailboxError::SessionExpired)),
        &context,
    );
    assert!(desktop.app.has_parked_draft());

    assert!(native_close(&mut desktop, &context).contains(&egui::ViewportCommand::CancelClose));
    assert_eq!(desktop.pending_discard, Some(close::Leaving::Window));
    desktop.resolve_discard(Message::CancelDiscard, &context);
    assert!(desktop.app.has_parked_draft());

    native_close(&mut desktop, &context);
    desktop.resolve_discard(Message::DiscardCompose, &context);
    assert!(!desktop.app.has_parked_draft());
    assert!(desktop.window_close_approved);
}

#[test]
fn untouched_main_window_closes_but_native_dialogs_block_it() {
    let context = egui::Context::default();
    let mut desktop = DesktopApp::with_context(&context, true, Settings::default());
    desktop.dispatch(Message::OpenCompose, &context);
    desktop.file_dialog_open = true;
    assert!(native_close(&mut desktop, &context).contains(&egui::ViewportCommand::CancelClose));
    assert!(!desktop.window_close_approved);
    assert_eq!(desktop.pending_discard, None);
    desktop.file_dialog_open = false;
    assert!(native_close(&mut desktop, &context).contains(&egui::ViewportCommand::Close));
    assert!(desktop.window_close_approved);
}

#[test]
fn pending_send_cannot_be_discarded_by_a_window_close_decision() {
    let context = egui::Context::default();
    let mut desktop = DesktopApp::with_context(&context, true, Settings::default());
    desktop.dispatch(Message::OpenCompose, &context);
    desktop.dispatch(
        Message::ComposeChanged(crate::app::ComposeField::To, "alex@example.com".into()),
        &context,
    );
    native_close(&mut desktop, &context);
    desktop.dispatch(Message::Send, &context);
    assert!(desktop.app.compose().unwrap().in_flight());
    desktop.resolve_discard(Message::DiscardCompose, &context);
    assert!(desktop.app.compose().unwrap().in_flight());
    assert!(!desktop.window_close_approved);
    assert!(native_close(&mut desktop, &context).contains(&egui::ViewportCommand::CancelClose));
}

#[test]
fn the_send_key_waits_while_a_file_dialog_is_choosing_attachments() {
    let context = egui::Context::default();
    let mut desktop = DesktopApp::with_context(&context, true, Settings::default());
    theme::install(&context);
    desktop.dispatch(Message::OpenCompose, &context);
    desktop.dispatch(
        Message::ComposeChanged(crate::app::ComposeField::To, "alex@example.com".into()),
        &context,
    );
    let command_enter = || egui::RawInput {
        events: vec![
            egui::Event::ModifiersChanged(egui::Modifiers::COMMAND),
            egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::COMMAND,
            },
        ],
        ..Default::default()
    };

    // The composer's own window may have its picker open while the main
    // window takes the key: the files still to come would be left out.
    desktop.file_dialog_open = true;
    context
        .run_ui(command_enter(), |ui| desktop.keyboard_shortcuts(ui.ctx()))
        .drop_without_applying_deltas();
    assert!(!desktop.app.compose().unwrap().in_flight());

    desktop.file_dialog_open = false;
    context
        .run_ui(command_enter(), |ui| desktop.keyboard_shortcuts(ui.ctx()))
        .drop_without_applying_deltas();
    assert!(desktop.app.compose().unwrap().in_flight());
}

#[test]
fn signing_out_asks_before_discarding_a_written_message() {
    let context = egui::Context::default();
    let mut desktop = DesktopApp::with_context(&context, true, Settings::default());
    desktop.dispatch(Message::OpenCompose, &context);
    desktop.dispatch(
        Message::ComposeChanged(crate::app::ComposeField::Body, "Keep this draft".into()),
        &context,
    );

    desktop.dispatch(Message::Logout, &context);
    assert_eq!(desktop.pending_discard, Some(close::Leaving::Demo));
    desktop.resolve_discard(Message::CancelDiscard, &context);
    assert!(desktop.app.compose().is_some());
    assert!(desktop.app.auth_state().is_authenticated());

    desktop.dispatch(Message::Logout, &context);
    desktop.resolve_discard(Message::DiscardCompose, &context);
    assert!(desktop.app.compose().is_none());
    assert!(matches!(desktop.app.auth_state(), AuthState::SignedOut));
}

#[test]
fn a_message_being_sent_keeps_the_session_open() {
    let context = egui::Context::default();
    let mut desktop = DesktopApp::with_context(&context, true, Settings::default());
    desktop.dispatch(Message::OpenCompose, &context);
    desktop.dispatch(
        Message::ComposeChanged(crate::app::ComposeField::To, "alex@example.com".into()),
        &context,
    );
    desktop.dispatch(Message::Send, &context);

    desktop.dispatch(Message::Logout, &context);
    assert_eq!(desktop.pending_discard, None, "a send cannot be discarded");
    assert!(desktop.app.compose().unwrap().in_flight());
    assert!(desktop.app.auth_state().is_authenticated());
}

#[test]
fn main_close_confirms_settings_then_preserves_or_discards_the_draft() {
    let (mut desktop, context) = demo_with_settings();
    desktop.dispatch(Message::OpenCompose, &context);
    desktop.dispatch(
        Message::ComposeChanged(crate::app::ComposeField::Body, "Draft".into()),
        &context,
    );
    desktop.dispatch(Message::ShowSettings(true), &context);
    desktop.settings_draft = Some(desktop.app.settings().clone());
    desktop.settings_draft.as_mut().unwrap().confirm_links = false;
    assert!(native_close(&mut desktop, &context).contains(&egui::ViewportCommand::CancelClose));
    assert!(matches!(
        desktop.pending_settings_exit,
        Some(SettingsExit::CloseWindow)
    ));
    assert_eq!(desktop.pending_discard, None);
    desktop.resolve_settings_exit(settings::ExitDecision::KeepEditing, &context);
    assert!(!desktop.window_close_approved);
    assert!(desktop.settings_draft.is_some());
    assert!(desktop.app.compose().is_some());
    native_close(&mut desktop, &context);
    desktop.resolve_settings_exit(settings::ExitDecision::Apply, &context);
    assert!(!desktop.app.settings().confirm_links);
    assert_eq!(desktop.pending_discard, Some(close::Leaving::Window));
    assert!(!desktop.window_close_approved);
    desktop.resolve_discard(Message::CancelDiscard, &context);
    assert!(desktop.app.compose().is_some());
    native_close(&mut desktop, &context);
    desktop.resolve_discard(Message::DiscardCompose, &context);
    assert!(desktop.window_close_approved);
}

#[test]
fn main_close_can_discard_unapplied_settings_without_changing_preferences() {
    let (mut desktop, context) = demo_with_settings();
    desktop.settings_draft.as_mut().unwrap().confirm_links = false;
    native_close(&mut desktop, &context);
    desktop.resolve_settings_exit(settings::ExitDecision::Discard, &context);
    assert!(desktop.app.settings().confirm_links);
    assert!(desktop.window_close_approved);
}

#[cfg(target_os = "macos")]
#[test]
fn native_commands_reuse_settings_guards_and_never_replace_an_existing_draft() {
    let (mut desktop, context) = demo_with_settings();
    desktop.settings_draft.as_mut().unwrap().confirm_links = false;
    desktop.native_action(macos::Action::NewMessage, &context);
    assert!(matches!(
        desktop.pending_settings_exit,
        Some(SettingsExit::Navigate(ref message)) if matches!(**message, Message::OpenCompose)
    ));
    assert!(desktop.app.compose().is_none());
    desktop.resolve_settings_exit(settings::ExitDecision::Discard, &context);
    let draft = desktop.app.compose().unwrap().id();
    desktop.native_action(macos::Action::NewMessage, &context);
    assert_eq!(desktop.app.compose().unwrap().id(), draft);
    desktop.native_action(macos::Action::Settings, &context);
    assert!(desktop.app.showing_settings());
    desktop.native_action(macos::Action::Settings, &context);
    assert!(desktop.app.showing_settings());
    desktop.dispatch(Message::Logout, &context);
    desktop.native_action(macos::Action::NewMessage, &context);
    assert!(desktop.app.compose().is_none());
}

#[test]
fn a_finished_draft_leaves_no_editor_memory_behind() {
    let context = egui::Context::default();
    let mut desktop = DesktopApp::with_context(&context, true, Settings::default());
    desktop.dispatch(Message::OpenCompose, &context);
    let draft = desktop.app.compose().unwrap().id();
    desktop.forget_finished_draft(&context);
    compose::remember_draft(&context, draft);

    desktop.forget_finished_draft(&context);
    assert!(
        compose::remembers_draft(&context, draft),
        "an open draft keeps its undo history"
    );

    desktop.dispatch(Message::CloseCompose, &context);
    desktop.forget_finished_draft(&context);
    assert!(!compose::remembers_draft(&context, draft));
}

#[test]
fn changing_folders_releases_a_superseded_poll_even_without_its_response() {
    let context = egui::Context::default();
    let mut desktop = DesktopApp::with_context(&context, true, Settings::default());
    let demo = crate::mail::demo::DemoMailbox::new();
    desktop.dispatch(
        Message::ConversationsLoaded(
            1,
            demo.list_conversations(&Folder::INBOX, 0, 50, crate::mail::demo::now()),
        ),
        &context,
    );
    assert!(
        !desktop
            .auto_refresh
            .should_start(0.0, auto_refresh::FOREGROUND)
    );
    desktop.dispatch(Message::AutoRefreshMailbox, &context);
    let crate::app::ListStatus::Refreshing(request) = desktop.app.mailbox().unwrap().status()
    else {
        panic!("expected refresh");
    };
    desktop.auto_refresh.started(request);

    let sent = Folder::System(crate::mail::MailFolder::Sent);
    desktop.dispatch(Message::SelectFolder(sent), &context);
    // The old result need never arrive for the scheduler to recover.
    assert!(desktop.auto_refresh.repaint_after(0.0, true).is_some());
    assert!(
        desktop
            .auto_refresh
            .should_start(60.0, auto_refresh::FOREGROUND)
    );
}

#[test]
fn stale_pages_do_not_reset_polling_backoff() {
    let context = egui::Context::default();
    let mut desktop = DesktopApp::with_context(&context, true, Settings::default());
    assert!(
        !desktop
            .auto_refresh
            .should_start(0.0, auto_refresh::FOREGROUND)
    );
    assert!(
        desktop
            .auto_refresh
            .should_start(60.0, auto_refresh::FOREGROUND)
    );
    desktop.auto_refresh.started(7);
    desktop.auto_refresh.page_finished(7, false, 61.0);
    let page = crate::mail::demo::DemoMailbox::new().list_conversations(
        &Folder::INBOX,
        0,
        50,
        crate::mail::demo::now(),
    );
    desktop.dispatch(Message::ConversationsLoaded(99, page), &context);
    assert!(
        !desktop
            .auto_refresh
            .should_start(100.0, auto_refresh::FOREGROUND)
    );
    assert!(
        desktop
            .auto_refresh
            .should_start(181.0, auto_refresh::FOREGROUND)
    );
}

#[test]
fn file_dialog_slot_blocks_duplicates_and_releases_after_a_stale_cancellation() {
    let context = egui::Context::default();
    let mut desktop = DesktopApp::with_context(&context, true, Settings::default());
    desktop.dispatch(Message::OpenCompose, &context);
    let draft = desktop.app.compose().unwrap().id();
    // Model an already-open native picker without opening a GUI in CI.
    desktop.file_dialog_open = true;
    desktop.dispatch(Message::PickComposeAttachments, &context);
    desktop.dispatch(
        Message::SaveAttachmentAs("message".into(), "file".into(), "report.pdf".into()),
        &context,
    );
    assert!(desktop.file_dialog_open);
    assert!(desktop.app.saving_attachment().is_none());
    desktop.dispatch(Message::CloseCompose, &context);
    assert!(desktop.app.compose().is_none());
    desktop.dispatch(Message::AddComposeAttachments(draft, Vec::new()), &context);
    assert!(!desktop.file_dialog_open);
    assert!(desktop.app.compose().is_none());
}

#[test]
fn compact_navigation_follows_open_search_and_compose_without_losing_a_draft() {
    let context = egui::Context::default();
    let mut desktop = DesktopApp::with_context(&context, true, Settings::default());
    let page = crate::mail::demo::DemoMailbox::new().list_conversations(
        &Folder::INBOX,
        0,
        crate::mail::demo::PAGE_SIZE,
        crate::mail::demo::now(),
    );
    let id = page.as_ref().unwrap().conversations[0].id.clone();
    desktop.dispatch(Message::ConversationsLoaded(1, page), &context);
    desktop.dispatch(Message::SelectConversation(id.clone()), &context);
    assert_eq!(desktop.ui.compact_view, mailbox::CompactView::Reader);
    desktop.ui.compact_view = mailbox::CompactView::Mail;
    desktop.dispatch(Message::SelectConversation(id), &context);
    assert_eq!(desktop.ui.compact_view, mailbox::CompactView::Reader);
    desktop.dispatch(
        Message::KeyPressed(KeyPress::with_command(Key::Character('f'))),
        &context,
    );
    assert_eq!(desktop.ui.compact_view, mailbox::CompactView::Mail);
    assert!(desktop.ui.focus_search);
    desktop.dispatch(
        Message::KeyPressed(KeyPress::with_command(Key::Character('n'))),
        &context,
    );
    assert_eq!(desktop.ui.compact_view, mailbox::CompactView::Reader);
    desktop.dispatch(
        Message::ComposeChanged(crate::app::ComposeField::Body, "Keep this draft".into()),
        &context,
    );
    desktop.ui.compact_view = mailbox::CompactView::Mail;
    desktop.dispatch(Message::SelectFolder(Folder::INBOX), &context);
    assert_eq!(
        desktop
            .app
            .compose()
            .unwrap()
            .field(crate::app::ComposeField::Body),
        "Keep this draft"
    );
    desktop.dispatch(Message::ShowSettings(true), &context);
    desktop.settings_draft = Some(desktop.app.settings().clone());
    desktop.settings_draft.as_mut().unwrap().confirm_links = false;
    desktop.dispatch(Message::ShowSettings(false), &context);
    assert!(desktop.app.showing_settings());
    assert!(desktop.pending_settings_exit.is_some());
    desktop.resolve_settings_exit(settings::ExitDecision::KeepEditing, &context);
    assert!(desktop.app.showing_settings());
    assert_eq!(
        desktop
            .app
            .compose()
            .unwrap()
            .field(crate::app::ComposeField::Body),
        "Keep this draft"
    );
}

fn demo_with_settings() -> (DesktopApp, egui::Context) {
    let context = egui::Context::default();
    let mut desktop = DesktopApp::with_context(&context, true, Settings::default());
    desktop.dispatch(Message::ShowSettings(true), &context);
    desktop.settings_draft = Some(desktop.app.settings().clone());
    (desktop, context)
}

#[test]
fn command_equals_steps_the_zoom_and_keeps_settings_in_step() {
    let (mut desktop, context) = demo_with_settings();
    theme::install(&context);
    let zoom_in = egui::RawInput {
        events: vec![egui::Event::Key {
            key: egui::Key::Equals,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        }],
        ..Default::default()
    };
    let output = context.run_ui(zoom_in, |ui| desktop.zoom_shortcuts(ui.ctx()));
    output.drop_without_applying_deltas();

    assert_eq!(desktop.app.settings().zoom, 1.15);
    assert_eq!(desktop.settings_draft.as_ref().unwrap().zoom, 1.15);
    // egui no longer zooms on its own, so no frame shows another scale.
    assert_eq!(context.zoom_factor(), 1.0);
}

#[cfg(target_os = "windows")]
#[test]
fn ctrl_alt_types_on_windows_instead_of_taking_ctrl_shortcuts() {
    let context = egui::Context::default();
    let mut desktop = DesktopApp::with_context(&context, true, Settings::default());
    theme::install(&context);
    let mut press = |key, modifiers| {
        let raw = egui::RawInput {
            events: vec![
                egui::Event::ModifiersChanged(modifiers),
                egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                },
            ],
            ..Default::default()
        };
        context
            .run_ui(raw, |ui| {
                desktop.zoom_shortcuts(ui.ctx());
                desktop.keyboard_shortcuts(ui.ctx());
            })
            .drop_without_applying_deltas();
        (desktop.app.compose().is_some(), desktop.app.settings().zoom)
    };
    let ctrl_alt = egui::Modifiers::COMMAND | egui::Modifiers::ALT;

    // "ń" on a Polish layout, and "}" on a German one.
    assert_eq!(press(egui::Key::N, ctrl_alt), (false, 1.0));
    assert_eq!(press(egui::Key::Num0, ctrl_alt), (false, 1.0));
    assert_eq!(press(egui::Key::Equals, ctrl_alt), (false, 1.0));
    // Ctrl alone still takes them.
    assert_eq!(
        press(egui::Key::Equals, egui::Modifiers::COMMAND),
        (false, 1.15)
    );
    assert_eq!(press(egui::Key::N, egui::Modifiers::COMMAND), (true, 1.15));
}

#[test]
fn zoom_keys_in_the_composer_window_reach_the_shell() {
    let context = egui::Context::default();
    let mut desktop = DesktopApp::with_context(&context, true, Settings::default());
    desktop.dispatch(Message::OpenCompose, &context);
    let zoom_out = egui::RawInput {
        events: vec![egui::Event::Key {
            key: egui::Key::Minus,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        }],
        ..Default::default()
    };
    // Without a backend the window is drawn inside this one, with its keys.
    let mut zoom = None;
    let output = context.run_ui(zoom_out, |ui| {
        let compose = desktop.app.compose().unwrap();
        zoom = compose::show_window(
            ui.ctx(),
            compose,
            false,
            Vec::new(),
            &mut theme::TitleBar::default(),
        )
        .zoom;
    });
    output.drop_without_applying_deltas();

    assert_eq!(zoom, Some(settings::ZoomStep::Out));
}

#[derive(Debug)]
struct DroppedFile(std::path::PathBuf);

impl egui::DroppedFile for DroppedFile {
    fn path(&self) -> &std::path::Path {
        &self.0
    }

    fn bytes(&self) -> Result<Vec<u8>, String> {
        Ok(Vec::new())
    }
}

#[test]
fn files_dropped_on_the_composer_window_reach_the_shell() {
    let context = egui::Context::default();
    let mut desktop = DesktopApp::with_context(&context, true, Settings::default());
    desktop.dispatch(Message::OpenCompose, &context);
    let path = std::env::temp_dir().join("agenda, ñ.pdf");
    let drop = egui::RawInput {
        dropped_files: vec![std::sync::Arc::new(DroppedFile(path.clone()))],
        ..Default::default()
    };
    let mut dropped = Vec::new();
    let output = context.run_ui(drop, |ui| {
        let compose = desktop.app.compose().unwrap();
        dropped = compose::show_window(
            ui.ctx(),
            compose,
            false,
            Vec::new(),
            &mut theme::TitleBar::default(),
        )
        .dropped;
    });
    output.drop_without_applying_deltas();

    assert_eq!(dropped, [path]);
}

#[test]
fn dropped_files_join_the_draft_unless_a_file_dialog_is_open() {
    let context = egui::Context::default();
    let mut desktop = DesktopApp::with_context(&context, true, Settings::default());
    desktop.dispatch(Message::OpenCompose, &context);
    let first = std::path::PathBuf::from("/tmp/first.pdf");
    let second = std::path::PathBuf::from("/tmp/second.pdf");

    // The picker is choosing: a drop must not join, nor free its slot.
    desktop.file_dialog_open = true;
    desktop.attach_dropped(vec![first.clone()], &context);
    assert!(desktop.app.compose().unwrap().attachments().is_empty());
    assert!(desktop.file_dialog_open);

    desktop.file_dialog_open = false;
    desktop.attach_dropped(vec![first.clone(), second.clone()], &context);
    assert_eq!(
        desktop.app.compose().unwrap().attachments(),
        [first, second]
    );
}

#[test]
fn files_dragged_over_the_draft_show_where_they_go() {
    let context = egui::Context::default();
    theme::install(&context);
    let (mut app, _) = App::boot(true, Settings::default());
    app.update(Message::OpenCompose);
    let hovering = egui::RawInput {
        hovered_files: vec![egui::HoveredFile::default()],
        ..Default::default()
    };
    let output = context.run_ui(hovering, |ui| {
        compose::show_in_pane(ui, app.compose().unwrap(), &mut Vec::new());
    });
    let shows_hint = output.shapes.iter().any(|clipped| {
        matches!(&clipped.shape, egui::epaint::Shape::Text(text)
            if text.galley.text() == "Drop files to attach")
    });
    output.drop_without_applying_deltas();
    assert!(shows_hint);
}

#[test]
fn sign_out_clears_the_saved_conversation_viewport() {
    let context = egui::Context::default();
    let mut desktop = DesktopApp::with_context(&context, true, Settings::default());
    let page = crate::mail::demo::DemoMailbox::new().list_conversations(
        &Folder::INBOX,
        0,
        crate::mail::demo::PAGE_SIZE,
        crate::mail::demo::now(),
    );
    desktop.dispatch(Message::ConversationsLoaded(1, page), &context);
    context
        .run_ui(egui::RawInput::default(), |root| {
            mailbox::show(root, &desktop.app, None, &mut desktop.ui, &mut Vec::new());
        })
        .drop_without_applying_deltas();
    assert!(desktop.ui.conversation_scroll.is_some());

    desktop.dispatch(Message::Logout, &context);
    assert!(desktop.app.mailbox().is_none());
    assert!(desktop.ui.conversation_scroll.is_none());
}

#[test]
fn j_moves_the_list_past_a_focused_button_but_types_into_a_focused_field() {
    let context = egui::Context::default();
    let mut desktop = DesktopApp::with_context(&context, true, Settings::default());
    let page = crate::mail::demo::DemoMailbox::new().list_conversations(
        &Folder::INBOX,
        0,
        crate::mail::demo::PAGE_SIZE,
        crate::mail::demo::now(),
    );
    desktop.dispatch(Message::ConversationsLoaded(1, page), &context);
    let rows: Vec<String> = desktop
        .app
        .mailbox()
        .unwrap()
        .visible_conversations()
        .map(|row| row.id.clone())
        .collect();
    desktop.dispatch(Message::SelectConversation(rows[0].clone()), &context);
    let mut text = String::new();
    // Focus is taken in one frame, and j pressed in the next.
    let mut press_j = |desktop: &mut DesktopApp, field: bool| {
        for events in [Vec::new(), vec![egui::Event::Text("j".into())]] {
            let input = egui::RawInput {
                events,
                ..Default::default()
            };
            context
                .run_ui(input, |ui| {
                    let widget = if field {
                        ui.add(egui::TextEdit::singleline(&mut text))
                    } else {
                        ui.button("Archive")
                    };
                    if !field || ui.ctx().memory(|memory| memory.focused()).is_none() {
                        widget.request_focus();
                    }
                    desktop.keyboard_shortcuts(ui.ctx());
                })
                .drop_without_applying_deltas();
        }
    };
    let selected = |desktop: &DesktopApp| {
        desktop
            .app
            .mailbox()
            .unwrap()
            .selected_conversation()
            .map(str::to_owned)
    };

    press_j(&mut desktop, false);
    assert_eq!(selected(&desktop), Some(rows[1].clone()));
    // The button lets focus go, so its ring does not stay behind.
    assert_eq!(context.memory(|memory| memory.focused()), None);

    press_j(&mut desktop, true);
    assert_eq!(selected(&desktop), Some(rows[1].clone()));
    assert_eq!(text, "j");
}

#[test]
fn escape_closes_an_open_menu_and_nothing_behind_it() {
    let context = egui::Context::default();
    let mut desktop = DesktopApp::with_context(&context, true, Settings::default());
    let page = crate::mail::demo::DemoMailbox::new().list_conversations(
        &Folder::INBOX,
        0,
        crate::mail::demo::PAGE_SIZE,
        crate::mail::demo::now(),
    );
    desktop.dispatch(Message::ConversationsLoaded(1, page), &context);
    let first = desktop
        .app
        .mailbox()
        .unwrap()
        .visible_conversations()
        .next()
        .unwrap()
        .id
        .clone();
    desktop.dispatch(Message::SelectConversation(first.clone()), &context);
    let escape = || egui::RawInput {
        events: vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
        ..Default::default()
    };
    let selected = |desktop: &DesktopApp| {
        desktop
            .app
            .mailbox()
            .unwrap()
            .selected_conversation()
            .map(str::to_owned)
    };

    egui::Popup::open_id(&context, egui::Id::new("labels-menu"));
    context
        .run_ui(escape(), |ui| desktop.keyboard_shortcuts(ui.ctx()))
        .drop_without_applying_deltas();
    assert_eq!(selected(&desktop), Some(first), "the menu took Escape");

    egui::Popup::close_all(&context);
    context
        .run_ui(escape(), |ui| desktop.keyboard_shortcuts(ui.ctx()))
        .drop_without_applying_deltas();
    assert_eq!(selected(&desktop), None, "with no menu, Escape closes");
}

#[test]
fn unchanged_settings_close_with_escape_or_a_folder() {
    let (mut desktop, context) = demo_with_settings();
    desktop.dispatch(Message::KeyPressed(KeyPress::plain(Key::Escape)), &context);
    assert!(!desktop.app.showing_settings());
    assert!(desktop.pending_settings_exit.is_none());

    let (mut desktop, context) = demo_with_settings();
    desktop.dispatch(Message::SelectFolder(Folder::INBOX), &context);
    assert!(!desktop.app.showing_settings());
    assert_eq!(desktop.app.mailbox().unwrap().folder(), &Folder::INBOX);
    assert!(desktop.pending_settings_exit.is_none());
}

#[test]
fn escape_and_command_comma_confirm_dirty_settings() {
    let (mut desktop, context) = demo_with_settings();
    desktop.settings_draft.as_mut().unwrap().confirm_links = false;

    desktop.dispatch(Message::KeyPressed(KeyPress::plain(Key::Escape)), &context);
    assert!(desktop.app.showing_settings());
    assert!(matches!(
        desktop.pending_settings_exit,
        Some(SettingsExit::Navigate(ref message)) if matches!(**message, Message::ShowSettings(false))
    ));
    desktop.resolve_settings_exit(settings::ExitDecision::KeepEditing, &context);

    desktop.dispatch(
        Message::KeyPressed(KeyPress::with_command(Key::Character(','))),
        &context,
    );
    assert!(matches!(
        desktop.pending_settings_exit,
        Some(SettingsExit::Navigate(ref message)) if matches!(**message, Message::ShowSettings(false))
    ));
    desktop.resolve_settings_exit(settings::ExitDecision::Discard, &context);
    assert!(!desktop.app.showing_settings());
    assert!(desktop.app.settings().confirm_links);
}

#[test]
fn applying_settings_keeps_the_pane_open_and_allows_a_clean_exit() {
    let (mut desktop, context) = demo_with_settings();
    desktop.settings_draft.as_mut().unwrap().confirm_links = false;
    let changes = settings::preference_changes(
        desktop.app.settings(),
        desktop.settings_draft.as_ref().unwrap(),
    );
    for change in changes {
        desktop.dispatch(change, &context);
    }
    assert!(desktop.app.showing_settings());
    assert!(!desktop.app.settings().confirm_links);

    desktop.dispatch(Message::KeyPressed(KeyPress::plain(Key::Escape)), &context);
    assert!(!desktop.app.showing_settings());
    assert!(desktop.pending_settings_exit.is_none());
}

#[test]
fn dirty_settings_keep_the_requested_folder_until_a_decision() {
    let (mut desktop, context) = demo_with_settings();
    desktop.settings_draft.as_mut().unwrap().confirm_links = false;
    let original = desktop.app.mailbox().unwrap().folder().clone();
    let sent = Folder::System(crate::mail::MailFolder::Sent);

    desktop.dispatch(Message::SelectFolder(sent.clone()), &context);
    assert!(desktop.app.showing_settings());
    assert_eq!(desktop.app.mailbox().unwrap().folder(), &original);
    assert!(matches!(
        desktop.pending_settings_exit,
        Some(SettingsExit::Navigate(ref message)) if matches!(**message, Message::SelectFolder(_))
    ));

    desktop.dispatch(Message::KeyPressed(KeyPress::plain(Key::Escape)), &context);
    assert!(matches!(
        desktop.pending_settings_exit,
        Some(SettingsExit::Navigate(ref message)) if matches!(**message, Message::SelectFolder(_)),
    ));
    desktop.resolve_settings_exit(settings::ExitDecision::KeepEditing, &context);
    assert!(desktop.app.showing_settings());
    assert!(desktop.pending_settings_exit.is_none());

    desktop.dispatch(Message::SelectFolder(sent.clone()), &context);
    desktop.resolve_settings_exit(settings::ExitDecision::Apply, &context);
    assert!(!desktop.app.showing_settings());
    assert_eq!(desktop.app.mailbox().unwrap().folder(), &sent);
    assert!(!desktop.app.settings().confirm_links);
    assert!(desktop.settings_draft.is_none());
}

#[test]
fn dirty_settings_can_be_discarded_before_composing_or_signing_out() {
    let (mut desktop, context) = demo_with_settings();
    desktop.settings_draft.as_mut().unwrap().appearance = crate::settings::Appearance::Light;
    desktop.dispatch(Message::OpenCompose, &context);
    assert!(desktop.app.compose().is_none());
    desktop.resolve_settings_exit(settings::ExitDecision::Discard, &context);
    assert!(!desktop.app.showing_settings());
    assert!(desktop.app.compose().is_some());
    assert_eq!(
        desktop.app.settings().appearance,
        crate::settings::Appearance::Dark
    );

    let (mut desktop, context) = demo_with_settings();
    desktop.settings_draft.as_mut().unwrap().confirm_links = false;
    desktop.dispatch(Message::Logout, &context);
    assert!(desktop.app.auth_state().is_authenticated());
    desktop.resolve_settings_exit(settings::ExitDecision::Discard, &context);
    assert!(matches!(desktop.app.auth_state(), AuthState::SignedOut));
    assert!(desktop.app.settings().confirm_links);
}

#[test]
fn the_title_carries_unread_mail_only_when_there_is_some() {
    assert_eq!(title_label(Some(3), false), "Ruston Mail (3)");
    assert_eq!(title_label(Some(0), false), "Ruston Mail");
    assert_eq!(title_label(None, false), "Ruston Mail");
    assert_eq!(title_label(Some(3), true), "Ruston Mail (demo) (3)");
}

#[test]
fn unread_badge_count_returns_none_when_unauthenticated() {
    let (app, _) = App::boot(false, crate::settings::Settings::default());
    assert_eq!(unread_badge_count(&app), None);
}

#[test]
fn set_unread_badge_does_not_panic() {
    assert!(set_unread_badge(None, Some(5)));
    assert!(set_unread_badge(None, None));
}

#[test]
#[cfg(target_os = "macos")]
fn main_viewport_enables_macos_fullsize_content_view() {
    let settings = Settings::default();
    let viewport = main_viewport(&settings);
    assert_eq!(viewport.fullsize_content_view, Some(true));
    assert_eq!(viewport.titlebar_shown, Some(false));
    assert_eq!(viewport.title_shown, Some(false));
}

#[test]
#[cfg(not(target_os = "macos"))]
fn main_viewport_leaves_system_decorations_intact() {
    let settings = Settings::default();
    let viewport = main_viewport(&settings);
    assert_eq!(viewport.fullsize_content_view, None);
    assert_eq!(viewport.titlebar_shown, None);
    assert_eq!(viewport.title_shown, None);
}

#[test]
fn mailbox_and_composer_share_the_desktop_identity_and_icon() {
    let settings = Settings::default();
    let (mut app, _) = App::boot(true, settings.clone());
    app.update(Message::OpenCompose);
    let main = main_viewport(&settings);
    let composer = compose::window_viewport(app.compose().unwrap());
    assert_eq!(main.app_id.as_deref(), Some(identity::APP_ID));
    assert_eq!(composer.app_id, main.app_id);
    assert!(std::sync::Arc::ptr_eq(
        main.icon.as_ref().unwrap(),
        composer.icon.as_ref().unwrap(),
    ));
}

#[test]
fn dispatch_clears_password_visibility_on_submit_cancel_or_other_auth_step() {
    let context = egui::Context::default();
    let mut desktop = DesktopApp::with_context(&context, true, Settings::default());

    desktop.ui.show_password = true;
    desktop.dispatch(
        Message::SetAppearance(crate::settings::Appearance::Light),
        &context,
    );
    assert!(!desktop.ui.show_password);

    desktop.app = App::signed_out();
    desktop.ui.show_password = true;
    desktop.dispatch(
        Message::SetAppearance(crate::settings::Appearance::Dark),
        &context,
    );
    assert!(desktop.ui.show_password);

    desktop.dispatch(Message::Submit, &context);
    assert!(matches!(desktop.app.auth_state(), AuthState::SignedOut));
    assert!(!desktop.ui.show_password);

    desktop.ui.show_password = true;
    desktop.dispatch(Message::CancelChallenge, &context);
    assert!(!desktop.ui.show_password);
}

#[test]
fn a_waiting_screen_repaints_on_a_delay_and_holds_still_without_focus() {
    use std::time::Duration;

    let repaint_delay = |focused: bool| {
        let context = egui::Context::default();
        let mut delay = Duration::ZERO;
        // The first frames settle fonts and layout; the last one shows
        // what the screen asks for while it only waits.
        for _ in 0..3 {
            let output = context.run_ui(
                egui::RawInput {
                    focused,
                    ..Default::default()
                },
                |ui| status_view(ui, "Opening Ruston Mail…"),
            );
            delay = output.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
            output.drop_without_applying_deltas();
        }
        delay
    };

    let focused = repaint_delay(true);
    assert!(
        focused > Duration::ZERO && focused <= Duration::from_millis(100),
        "{focused:?}"
    );
    assert_eq!(repaint_delay(false), Duration::MAX);
}

#[test]
fn execute_only_requests_repaint_for_visual_effects() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let check_effect = |effect: UiEffect, should_repaint: bool| {
        let boot_ctx = egui::Context::default();
        let mut desktop = DesktopApp::with_context(&boot_ctx, true, Settings::default());
        desktop.runtime.attach(&egui::Context::default());

        let ctx = egui::Context::default();
        let repainted = Arc::new(AtomicBool::new(false));
        let r = repainted.clone();
        ctx.set_request_repaint_callback(move |_info| {
            r.store(true, Ordering::SeqCst);
        });

        desktop.execute(Effects::ui(effect), &ctx);
        assert_eq!(repainted.load(Ordering::SeqCst), should_repaint);
    };

    // Non-visual effects must not request repaint.
    check_effect(UiEffect::CopyText("test".into()), false);
    check_effect(
        UiEffect::NotifyNewMail {
            sender: "a".into(),
            subject: "b".into(),
        },
        false,
    );

    // Visual effects must request repaint.
    check_effect(UiEffect::FocusSearch, true);
    check_effect(UiEffect::ScrollReaderTop, true);
    check_effect(UiEffect::RevealConversation("123".into()), true);

    // Empty effects must not request repaint.
    let boot_ctx = egui::Context::default();
    let mut desktop = DesktopApp::with_context(&boot_ctx, true, Settings::default());
    desktop.runtime.attach(&egui::Context::default());

    let ctx = egui::Context::default();
    let repainted = Arc::new(AtomicBool::new(false));
    let r = repainted.clone();
    ctx.set_request_repaint_callback(move |_info| {
        r.store(true, Ordering::SeqCst);
    });
    desktop.execute(Effects::none(), &ctx);
    assert!(!repainted.load(Ordering::SeqCst));
}

#[test]
fn remember_window_size_deduplicates_unchanged_dimensions() {
    let ctx = egui::Context::default();
    let mut raw_input = egui::RawInput::default();
    raw_input
        .viewports
        .entry(egui::ViewportId::ROOT)
        .or_default()
        .inner_rect = Some(egui::Rect::from_min_size(
        egui::Pos2::ZERO,
        egui::vec2(1000.0, 700.0),
    ));

    let mut output = ctx.run_ui(raw_input, |_| {});
    output.textures_delta.clear();

    let mut desktop = DesktopApp::with_context(&ctx, true, Settings::default());
    assert_eq!(desktop.last_window_size, None);

    // First pass: records the window size and updates last_window_size.
    desktop.remember_window_size(&ctx);
    assert_eq!(
        desktop.last_window_size,
        Some(Window {
            width: 1000.0,
            height: 700.0,
            ..Window::default()
        })
    );

    // Simulate subsequent idle frames where window size has not changed.
    // We track settings revision to verify no new WindowResized messages are dispatched.
    let revision_before = desktop.app.settings_revision();
    for _ in 0..10 {
        desktop.remember_window_size(&ctx);
    }
    assert_eq!(desktop.app.settings_revision(), revision_before);

    // When the window is resized, a new message is dispatched and settings updated.
    let mut resized_input = egui::RawInput::default();
    resized_input
        .viewports
        .entry(egui::ViewportId::ROOT)
        .or_default()
        .inner_rect = Some(egui::Rect::from_min_size(
        egui::Pos2::ZERO,
        egui::vec2(1200.0, 800.0),
    ));
    let mut output2 = ctx.run_ui(resized_input, |_| {});
    output2.textures_delta.clear();

    desktop.remember_window_size(&ctx);
    assert_eq!(
        desktop.last_window_size,
        Some(Window {
            width: 1200.0,
            height: 800.0,
            ..Window::default()
        })
    );
    assert_eq!(desktop.app.settings().window.width, 1200.0);
    assert_eq!(desktop.app.settings().window.height, 800.0);
    assert!(desktop.app.settings_revision() > revision_before);
}

#[test]
fn full_screen_and_maximized_windows_keep_the_size_to_come_back_to() {
    let context = egui::Context::default();
    let frame = |size: egui::Vec2, maximized: bool, fullscreen: bool| {
        let mut input = egui::RawInput::default();
        let viewport = input.viewports.entry(egui::ViewportId::ROOT).or_default();
        viewport.inner_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size));
        viewport.maximized = Some(maximized);
        viewport.fullscreen = Some(fullscreen);
        context.run_ui(input, |_| {}).drop_without_applying_deltas();
    };
    let mut settings = Settings::default();
    settings.window.width = 1_000.0;
    settings.window.height = 700.0;
    let mut desktop = DesktopApp::with_context(&context, true, settings);
    let window = |desktop: &DesktopApp| desktop.app.settings().window;

    // Full screen changes nothing that is remembered.
    frame(egui::vec2(1_512.0, 982.0), false, true);
    desktop.remember_window_size(&context);
    assert_eq!(
        (
            window(&desktop).width,
            window(&desktop).height,
            window(&desktop).maximized
        ),
        (1_000.0, 700.0, false)
    );

    frame(egui::vec2(1_512.0, 945.0), true, false);
    desktop.remember_window_size(&context);
    if cfg!(target_os = "macos") {
        // A zoomed Mac window is remembered as the frame it has.
        assert!(!window(&desktop).maximized);
        assert_eq!(window(&desktop).width, 1_512.0);
    } else {
        assert!(window(&desktop).maximized);
        assert_eq!(
            (window(&desktop).width, window(&desktop).height),
            (1_000.0, 700.0)
        );
    }

    frame(egui::vec2(1_100.0, 750.0), false, false);
    desktop.remember_window_size(&context);
    assert!(!window(&desktop).maximized);
    assert_eq!(
        (window(&desktop).width, window(&desktop).height),
        (1_100.0, 750.0)
    );
}

#[test]
fn reset_layout_handles_window_resize_and_rejection() {
    fn input(size: egui::Vec2) -> egui::RawInput {
        let mut input = egui::RawInput::default();
        input
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .inner_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size));
        input
    }

    let context = egui::Context::default();
    let old = Window {
        width: 1_300.0,
        height: 900.0,
        ..Window::default()
    };
    context
        .run_ui(input(egui::vec2(old.width, old.height)), |_| {})
        .drop_without_applying_deltas();
    let mut settings = Settings::default();
    settings.window = old;
    let mut desktop = DesktopApp::with_context(&context, true, settings);
    desktop.remember_window_size(&context);

    let panel_ids = ["mailbox-sidebar", "conversation-list"].map(egui::Id::new);
    context.data_mut(|data| {
        for id in panel_ids {
            data.insert_persisted(
                id,
                egui::containers::panel::PanelState {
                    outer_rect: egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(350.0, 900.0),
                    ),
                },
            );
        }
    });

    desktop.dispatch(Message::ResetLayout, &context);
    desktop.dispatch(
        Message::PanelsResized(crate::settings::Panels {
            sidebar: 0.35,
            conversations: 0.6,
        }),
        &context,
    );
    desktop.remember_window_size(&context);
    assert_eq!(desktop.app.settings().window, Window::default());
    assert_eq!(desktop.app.panels(), crate::settings::Panels::default());
    assert!(desktop.layout_reset_at.is_some());
    for id in panel_ids {
        assert!(egui::containers::panel::PanelState::load(&context, id).is_some());
    }

    let default = Window::default();
    context
        .run_ui(input(egui::vec2(default.width, default.height)), |_| {})
        .drop_without_applying_deltas();
    desktop.remember_window_size(&context);
    assert_eq!(desktop.layout_reset_at, None);
    assert_eq!(desktop.app.settings().window, default);
    for id in panel_ids {
        assert!(egui::containers::panel::PanelState::load(&context, id).is_none());
    }

    // If the window manager refuses a later resize, keep its actual size.
    context
        .run_ui(input(egui::vec2(old.width, old.height)), |_| {})
        .drop_without_applying_deltas();
    desktop.remember_window_size(&context);
    desktop.dispatch(Message::ResetLayout, &context);
    let mut denied = input(egui::vec2(old.width, old.height));
    denied.time = Some(desktop.layout_reset_at.unwrap() + LAYOUT_RESET_WAIT + 1.0);
    context
        .run_ui(denied, |_| {})
        .drop_without_applying_deltas();
    desktop.remember_window_size(&context);
    assert_eq!(desktop.layout_reset_at, None);
    assert_eq!(desktop.app.settings().window, old);
}

#[test]
fn a_layout_reset_in_full_screen_ends_when_its_wait_does() {
    let context = egui::Context::default();
    let mut desktop = DesktopApp::with_context(&context, true, Settings::default());
    desktop.dispatch(Message::ResetLayout, &context);
    let started = desktop
        .layout_reset_at
        .expect("a reset waits for the window");

    // Full screen keeps the window from ever reaching the default size.
    let mut full_screen = egui::RawInput {
        time: Some(started + LAYOUT_RESET_WAIT + 1.0),
        ..Default::default()
    };
    let viewport = full_screen
        .viewports
        .entry(egui::ViewportId::ROOT)
        .or_default();
    viewport.inner_rect = Some(egui::Rect::from_min_size(
        egui::Pos2::ZERO,
        egui::vec2(1920.0, 1080.0),
    ));
    viewport.fullscreen = Some(true);
    context
        .run_ui(full_screen, |_| {})
        .drop_without_applying_deltas();
    desktop.remember_window_size(&context);

    assert_eq!(desktop.layout_reset_at, None);
}

#[test]
fn reset_layout_requests_the_default_window_size_at_current_zoom() {
    let context = egui::Context::default();
    let mut settings = Settings::default();
    settings.zoom = 2.0;
    let mut desktop = DesktopApp::with_context(&context, true, settings);
    context.set_zoom_factor(2.0);
    context
        .run_ui(egui::RawInput::default(), |_| {})
        .drop_without_applying_deltas();

    let output = context.run_ui(egui::RawInput::default(), |_| {
        desktop.dispatch(Message::ResetLayout, &context);
    });
    let expected = egui::vec2(
        Window::default().width / 2.0,
        Window::default().height / 2.0,
    );
    let requested = output.viewport_output[&egui::ViewportId::ROOT]
        .commands
        .iter()
        .any(|command| {
            matches!(command, egui::ViewportCommand::InnerSize(size) if *size == expected)
        });
    output.drop_without_applying_deltas();
    assert!(requested);
}
