//! Keyboard shortcuts, and which pane a key reaches.

use super::*;

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

    let effects = app.update(Message::KeyPressed(KeyPress::plain(Key::Character('j'))));

    assert_eq!(
        app.mailbox().unwrap().selected_conversation(),
        Some("demo-1")
    );
    assert!(effects.into_iter().any(|effect| matches!(
        effect,
        Effect::Ui(UiEffect::RevealConversation(ref id)) if id == "demo-1"
    )));
}
