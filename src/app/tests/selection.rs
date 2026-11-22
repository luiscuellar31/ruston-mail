//! Choosing several conversations and acting on all of them, in the demo.

use super::*;

/// The rows the Inbox lists, in its order.
fn listed(app: &App) -> Vec<String> {
    app.mailbox()
        .unwrap()
        .visible_conversations()
        .map(|row| row.id.clone())
        .collect()
}

fn selected(app: &App) -> Vec<String> {
    app.mailbox()
        .unwrap()
        .selected_rows()
        .iter()
        .map(|row| row.id.clone())
        .collect()
}

fn press_shift(app: &mut App, key: Key) {
    let _ = app.update(Message::KeyPressed(KeyPress::with_shift(key)));
}

#[test]
fn command_and_shift_clicks_choose_several_rows_and_close_the_reader() {
    let mut app = loaded_demo_app();
    let rows = listed(&app);
    open_conversation(&mut app, &rows[0]);

    let _ = app.update(Message::ToggleSelected(rows[2].clone()));
    assert_eq!(selected(&app), [rows[0].clone(), rows[2].clone()]);
    assert_eq!(app.mailbox().unwrap().selected_conversation(), None);

    // The range starts at the row last clicked with Command.
    let _ = app.update(Message::SelectThrough(rows[4].clone()));
    assert_eq!(selected(&app), rows[2..=4]);

    // Down to one row, that one opens.
    let _ = app.update(Message::SelectThrough(rows[2].clone()));
    deliver_selected_demo_detail(&mut app);
    assert!(selected(&app).is_empty());
    assert_eq!(
        app.mailbox().unwrap().selected_conversation(),
        Some(rows[2].as_str())
    );
}

#[test]
fn an_action_acts_on_every_row_chosen_and_undo_puts_them_all_back() {
    let mut app = loaded_demo_app();
    let rows = listed(&app);
    open_conversation(&mut app, &rows[1]);
    let _ = app.update(Message::SelectThrough(rows[3].clone()));

    let _ = app.update(Message::ApplyAction(MailAction::MoveTo(sys(
        MailFolder::Archive,
    ))));

    for row in &rows[1..=3] {
        assert!(!is_listed(&app, row), "{row} left the Inbox");
    }
    assert!(is_listed(&app, &rows[0]) && is_listed(&app, &rows[4]));
    let undo = app
        .mailbox()
        .unwrap()
        .undo()
        .expect("one offer for the three");
    assert_eq!(undo.rows.len(), 3);
    assert!(selected(&app).is_empty());

    let _ = app.update(Message::UndoMove);
    for row in &rows[1..=3] {
        assert!(is_listed(&app, row), "{row} is back");
    }
    assert!(app.mailbox().unwrap().undo().is_none());
}

#[test]
fn the_delete_key_moves_every_row_chosen_to_trash() {
    let mut app = loaded_demo_app();
    let rows = listed(&app);
    open_conversation(&mut app, &rows[0]);
    let _ = app.update(Message::ToggleSelected(rows[2].clone()));

    press(&mut app, Key::Delete);

    assert!(!is_listed(&app, &rows[0]) && !is_listed(&app, &rows[2]));
    assert!(is_listed(&app, &rows[1]));
}

#[test]
fn a_flag_on_the_rows_chosen_keeps_them_chosen() {
    let mut app = loaded_demo_app();
    let rows = listed(&app);
    open_conversation(&mut app, &rows[0]);
    let _ = app.update(Message::SelectThrough(rows[1].clone()));

    let _ = app.update(Message::ApplyAction(MailAction::SetStarred(true)));

    assert_eq!(selected(&app), rows[0..=1]);
    assert!(
        app.mailbox()
            .unwrap()
            .selected_rows()
            .iter()
            .all(|row| row.starred)
    );
}

#[test]
fn shift_and_the_arrows_choose_rows_from_the_open_one() {
    let mut app = loaded_demo_app();
    let rows = listed(&app);
    open_conversation(&mut app, &rows[0]);

    press_shift(&mut app, Key::ArrowDown);
    press_shift(&mut app, Key::ArrowDown);
    assert_eq!(selected(&app), rows[0..=2]);
    press_shift(&mut app, Key::ArrowUp);
    assert_eq!(selected(&app), rows[0..=1]);

    // A plain arrow goes on from the far end, and lets go of the others.
    press(&mut app, Key::ArrowDown);
    deliver_selected_demo_detail(&mut app);
    assert!(selected(&app).is_empty());
    assert_eq!(
        app.mailbox().unwrap().selected_conversation(),
        Some(rows[2].as_str())
    );
}

#[test]
fn escape_lets_go_of_the_rows_chosen() {
    let mut app = loaded_demo_app();
    let rows = listed(&app);
    open_conversation(&mut app, &rows[0]);
    let _ = app.update(Message::SelectThrough(rows[2].clone()));

    press(&mut app, Key::Escape);

    assert!(selected(&app).is_empty());
    assert_eq!(app.mailbox().unwrap().selected_conversation(), None);
}

#[test]
fn a_click_on_one_row_lets_go_of_the_others() {
    let mut app = loaded_demo_app();
    let rows = listed(&app);
    open_conversation(&mut app, &rows[0]);
    let _ = app.update(Message::SelectThrough(rows[2].clone()));

    open_conversation(&mut app, &rows[4]);

    assert!(selected(&app).is_empty());
    assert!(app.mailbox().unwrap().is_chosen(&rows[4]));
    assert!(!app.mailbox().unwrap().is_chosen(&rows[0]));
}

#[test]
fn command_a_chooses_every_row_listed() {
    let mut app = loaded_demo_app();
    let rows = listed(&app);
    open_conversation(&mut app, &rows[0]);

    press_command(&mut app, Key::Character('a'));

    assert_eq!(selected(&app), rows);
    assert_eq!(app.mailbox().unwrap().selected_conversation(), None);
}

/// Starts `action` on `rows` as the Proton path does, and returns the
/// request that goes now, with all it carries.
fn start_as_proton(app: &mut App, rows: &[String], action: MailAction) -> Vec<ActionRequest> {
    let requests: Vec<RequestId> = (90..).take(rows.len()).collect();
    let mailbox = app.mailbox.as_mut().unwrap();
    let starts = mailbox.start_actions_on(rows, action, &requests);
    let head = starts[0].send.clone().expect("the request goes at once");
    mailbox.request_group(&head)
}

#[test]
fn one_answer_settles_a_request_for_several_rows_and_reads_the_counts_once() {
    let mut app = loaded_demo_app();
    let rows = listed(&app);
    let group = start_as_proton(&mut app, &rows[0..3], MailAction::SetStarred(true));
    assert_eq!(group.len(), 3);
    assert_eq!(app.unsent_changes(), 3);

    let effects = app.update(Message::ActionFinished(group, Ok(())));

    assert_eq!(effects.units(), 1, "the counts, read again once");
    assert_eq!(app.unsent_changes(), 0);
}

#[test]
fn a_request_for_several_rows_proton_refuses_brings_them_back_with_the_list() {
    let mut app = loaded_demo_app();
    let rows = listed(&app);
    let group = start_as_proton(
        &mut app,
        &rows[0..3],
        MailAction::MoveTo(sys(MailFolder::Archive)),
    );
    assert!(rows[0..3].iter().all(|row| !is_listed(&app, row)));

    let _ = app.update(Message::ActionFinished(group, Err(MailboxError::Service)));

    let mailbox = app.mailbox().unwrap();
    assert_eq!(mailbox.action_error(), Some(MailboxError::Service));
    assert!(mailbox.undo().is_none(), "nothing moved to take back");
    let ListStatus::Refreshing(page) = mailbox.status() else {
        panic!("the folder is read again");
    };
    deliver_demo_page(&mut app, page, 0);
    assert!(
        rows[0..3].iter().all(|row| is_listed(&app, row)),
        "Proton never moved them"
    );
}
