use super::super::reading::ReadingMessage;
use super::*;
use crate::mailbox::OutgoingKind;
use ruston_core::model::ConversationLabel;

fn key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn press(app: &mut App, code: KeyCode) -> Vec<Effect> {
    app.message_for(&key(code))
        .map_or_else(Vec::new, |message| app.update(message))
}

/// Presses `code` with Ctrl held.
fn ctrl(app: &mut App, code: KeyCode) -> Vec<Effect> {
    let event = Event::Key(KeyEvent::new(code, KeyModifiers::CONTROL));
    app.message_for(&event)
        .map_or_else(Vec::new, |message| app.update(message))
}

/// Opens the command line, types `line` and runs it.
fn command(app: &mut App, line: &str) -> Vec<Effect> {
    press(app, KeyCode::Char(':'));
    for c in line.chars() {
        press(app, KeyCode::Char(c));
    }
    press(app, KeyCode::Enter)
}

fn place(id: &str, name: &str) -> Place {
    Place {
        id: id.into(),
        name: name.into(),
        path: name.into(),
        depth: 0,
        kind: PlaceKind::System,
        unread: 0,
    }
}

fn conversation(id: &str) -> Conversation {
    Conversation {
        id: id.into(),
        ..Conversation::default()
    }
}

/// An app whose folders loaded, with the Inbox listed.
fn loaded() -> App {
    let mut app = App::new("demo".into());
    let start = app.start();
    let Effect::Conversations { request, .. } = start[1].clone() else {
        panic!("{start:?}");
    };
    app.update(Message::PlacesLoaded(Ok(vec![
        place(label_ids::INBOX, "Inbox"),
        place("folder-1", "Work"),
    ])));
    app.update(Message::ConversationsLoaded {
        page: 0,
        request,
        result: Ok((2, vec![conversation("a"), conversation("b")])),
    });
    app
}

#[test]
fn it_loads_the_folders_and_the_inbox_at_once() {
    let mut app = App::new("demo".into());
    assert_eq!(
        app.start(),
        [
            Effect::Places,
            Effect::Conversations {
                request: 1,
                place: label_ids::INBOX.into(),
                page: 0
            }
        ]
    );
    let app = loaded();
    assert_eq!(app.places.len(), 2);
    assert_eq!(app.total, 2);
    assert_eq!(app.notice, None);
}

#[test]
fn enter_on_a_folder_lists_it_and_moves_to_its_conversations() {
    let mut app = loaded();
    press(&mut app, KeyCode::Char('j'));
    assert_eq!(app.selected_place, 1);
    // The selection stops at the last folder.
    press(&mut app, KeyCode::Down);
    assert_eq!(app.selected_place, 1);

    let effects = press(&mut app, KeyCode::Enter);
    assert_eq!(
        effects,
        [Effect::Conversations {
            request: 2,
            place: "folder-1".into(),
            page: 0
        }]
    );
    assert_eq!(app.focus, Pane::Conversations);
    assert_eq!(app.listed, "Work");
    assert_eq!(app.notice.as_deref(), Some("Loading Work…"));
}

#[test]
fn an_answer_for_a_list_asked_for_before_comes_too_late() {
    let mut app = loaded();
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Enter);
    // The Inbox's answer, asked for first, arrives after Work was asked for.
    app.update(Message::ConversationsLoaded {
        page: 0,
        request: 1,
        result: Ok((1, vec![conversation("old")])),
    });
    assert_eq!(app.total, 2);
    app.update(Message::ConversationsLoaded {
        page: 0,
        request: 2,
        result: Ok((1, vec![conversation("work")])),
    });
    assert_eq!(app.conversations[0].id, "work");
    assert_eq!(app.selected_conversation, 0);
}

#[test]
fn a_failed_load_says_so_and_keeps_what_was_shown() {
    let mut app = loaded();
    press(&mut app, KeyCode::Enter);
    app.update(Message::ConversationsLoaded {
        page: 0,
        request: 2,
        result: Err("offline".into()),
    });
    assert_eq!(app.notice.as_deref(), Some("Cannot load Inbox: offline"));
    assert_eq!(app.conversations.len(), 2);
    app.update(Message::PlacesLoaded(Err("refused".into())));
    assert_eq!(
        app.notice.as_deref(),
        Some("Cannot load the folders: refused")
    );
}

fn reading(lines: usize) -> Reading {
    Reading {
        id: "b".into(),
        subject: "Plans".into(),
        lines: (0..lines).map(|n| format!("line {n}")).collect(),
        quotes: Vec::new(),
        links: Vec::new(),
        attachments: Vec::new(),
        messages: Vec::new(),
        answer: None,
    }
}

/// A reading of a message for each of `unread`, a line each, whether
/// it is not read yet.
fn with_messages(unread: &[bool]) -> Reading {
    let mut reading = reading(4 * unread.len() - 3);
    reading.messages = unread
        .iter()
        .enumerate()
        .map(|(index, &unread)| ReadingMessage {
            lines: 4 * index..4 * index + 1,
            line: format!("▸ message {index}"),
            unread,
        })
        .collect();
    reading
}

#[test]
fn enter_on_a_conversation_opens_it_in_the_reader() {
    let mut app = loaded();
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Down);
    let effects = press(&mut app, KeyCode::Enter);
    assert_eq!(
        effects,
        [Effect::Conversation {
            request: 1,
            id: "b".into()
        }]
    );
    assert_eq!(app.focus, Pane::Reader);
    assert_eq!(app.notice.as_deref(), Some("Opening the conversation…"));
    // An answer for a conversation asked for before comes too late.
    app.update(Message::ConversationLoaded {
        request: 0,
        result: Ok(reading(1)),
    });
    assert!(app.reading.is_none());
    app.update(Message::ConversationLoaded {
        request: 1,
        result: Ok(reading(3)),
    });
    assert_eq!(app.reading.as_ref().unwrap().lines.len(), 3);
    assert_eq!(app.notice, None);
    app.update(Message::ConversationLoaded {
        request: 1,
        result: Err("gone".into()),
    });
    assert_eq!(
        app.notice.as_deref(),
        Some("Cannot open the conversation: gone")
    );
}

#[test]
fn the_reader_scrolls_by_rows_and_pages_as_far_as_its_bottom() {
    let mut app = loaded();
    app.focus = Pane::Reader;
    app.reading = Some(reading(100));
    // As the view leaves them after drawing 100 rows 30 at a time.
    app.reader_bottom.set(70);
    app.reader_rows.set(30);
    press(&mut app, KeyCode::Char('j'));
    assert_eq!(app.scroll.get(), 1);
    press(&mut app, KeyCode::PageDown);
    assert_eq!(app.scroll.get(), 31);
    ctrl(&mut app, KeyCode::Down);
    assert_eq!(app.scroll.get(), 70);
    press(&mut app, KeyCode::Down);
    assert_eq!(app.scroll.get(), 70);
    press(&mut app, KeyCode::PageUp);
    assert_eq!(app.scroll.get(), 40);
    ctrl(&mut app, KeyCode::Up);
    assert_eq!(app.scroll.get(), 0);
    press(&mut app, KeyCode::Up);
    assert_eq!(app.scroll.get(), 0);
}

#[test]
fn z_shows_quoted_text_and_hides_it_again_or_says_there_is_none() {
    let mut app = loaded();
    press(&mut app, KeyCode::Char('z'));
    assert!(!app.show_quotes);
    assert_eq!(
        app.notice.as_deref(),
        Some("Open a conversation to show its quoted text.")
    );
    app.reading = Some(reading(3));
    press(&mut app, KeyCode::Char('z'));
    assert!(!app.show_quotes);
    assert_eq!(
        app.notice.as_deref(),
        Some("This conversation has no quoted text to fold.")
    );
    app.reading.as_mut().unwrap().quotes = vec![0..1, 2..3];
    press(&mut app, KeyCode::Char('z'));
    assert!(app.show_quotes);
    press(&mut app, KeyCode::Char('z'));
    assert!(!app.show_quotes);
}

#[test]
fn e_shows_the_messages_read_and_folds_them_again_or_says_there_are_none() {
    let mut app = loaded();
    press(&mut app, KeyCode::Char('e'));
    assert!(!app.show_read);
    assert_eq!(
        app.notice.as_deref(),
        Some("Open a conversation to show its messages.")
    );
    app.reading = Some(reading(3));
    press(&mut app, KeyCode::Char('e'));
    assert!(!app.show_read);
    assert_eq!(
        app.notice.as_deref(),
        Some("This conversation has no messages read to fold.")
    );
    // Neither a single message, nor the newest, nor one not read yet folds.
    for unread in [&[false][..], &[true, false]] {
        app.reading = Some(with_messages(unread));
        press(&mut app, KeyCode::Char('e'));
        assert!(!app.show_read);
    }
    app.reading = Some(with_messages(&[false, true, false]));
    // Those turned with Enter go as e says, the message at the reader's
    // top kept there.
    app.toggled = vec![1];
    app.in_view.set(Some(1));
    press(&mut app, KeyCode::Char('e'));
    assert!(app.show_read);
    assert!(app.toggled.is_empty());
    assert_eq!(app.landing.get(), Some(1));
    press(&mut app, KeyCode::Char('e'));
    assert!(!app.show_read);
}

#[test]
fn enter_in_the_reader_folds_or_shows_the_message_at_its_top_and_keeps_it_there() {
    let mut app = loaded();
    app.focus = Pane::Reader;
    app.in_view.set(Some(0));
    press(&mut app, KeyCode::Enter);
    assert!(app.toggled.is_empty());
    app.reading = Some(with_messages(&[false, true, false]));
    assert_eq!(
        [app.folds(0), app.folds(1), app.folds(2)],
        [true, false, false]
    );
    // As the view leaves it with the newest at the reader's top.
    app.in_view.set(Some(2));
    press(&mut app, KeyCode::Enter);
    assert!(app.folds(2));
    assert_eq!(app.landing.get(), Some(2));
    press(&mut app, KeyCode::Enter);
    assert!(!app.folds(2));
    app.in_view.set(Some(0));
    press(&mut app, KeyCode::Enter);
    assert!(!app.folds(0));
    // With every message read shown, Enter folds one of them.
    app.show_read = true;
    app.toggled.clear();
    press(&mut app, KeyCode::Enter);
    assert!(app.folds(0));
    // Right opens nothing there.
    app.landing.set(None);
    press(&mut app, KeyCode::Right);
    assert_eq!(
        (app.toggled.as_slice(), app.landing.get()),
        (&[0][..], None)
    );

    // A conversation opens at its top, nothing turned, and lands on its
    // first message not read yet.
    app.scroll.set(9);
    app.focus = Pane::Conversations;
    let effects = press(&mut app, KeyCode::Enter);
    let [Effect::Conversation { request, .. }] = effects[..] else {
        panic!("{effects:?}");
    };
    app.update(Message::ConversationLoaded {
        request,
        result: Ok(with_messages(&[false, false, true, true])),
    });
    assert_eq!(app.scroll.get(), 0);
    assert!(app.toggled.is_empty());
    assert_eq!(app.landing.get(), Some(2));
}

/// An unread conversation, in the Inbox and in `label`.
fn unread(id: &str, label: &str) -> Conversation {
    let unread_in = |label: &str| ConversationLabel {
        id: label.into(),
        context_num_unread: Some(1),
        ..ConversationLabel::default()
    };
    Conversation {
        id: id.into(),
        num_unread: 1,
        labels: vec![unread_in(label_ids::INBOX), unread_in(label)],
        ..Conversation::default()
    }
}

#[test]
fn an_unread_conversation_is_marked_read_once_the_reader_shows_it() {
    let mut app = loaded();
    app.conversations[1] = unread("b", "folder-1");
    app.places[0].unread = 3;
    app.places[1].unread = 1;
    app.focus = Pane::Conversations;
    press(&mut app, KeyCode::Down);
    // Nothing is marked before it shows.
    assert_eq!(
        press(&mut app, KeyCode::Enter),
        [Effect::Conversation {
            request: 1,
            id: "b".into()
        }]
    );
    let effects = app.update(Message::ConversationLoaded {
        request: 1,
        result: Ok(reading(3)),
    });
    assert_eq!(
        effects,
        [Effect::MarkRead {
            id: "b".into(),
            place: label_ids::INBOX.into()
        }]
    );
    assert_eq!(app.conversations[1].num_unread, 1);

    app.update(Message::MarkedRead {
        id: "b".into(),
        result: Ok(()),
    });
    assert_eq!(app.conversations[1].num_unread, 0);
    let unread: Vec<i64> = app.places.iter().map(|place| place.unread).collect();
    assert_eq!(unread, [2, 0]);
    // Read now, it is not marked again.
    press(&mut app, KeyCode::BackTab);
    press(&mut app, KeyCode::Enter);
    let effects = app.update(Message::ConversationLoaded {
        request: 2,
        result: Ok(reading(3)),
    });
    assert!(effects.is_empty());
}

#[test]
fn keep_unread_leaves_an_opened_conversation_unread() {
    let mut app = loaded();
    app.marks_read = false;
    app.conversations[1] = unread("b", "folder-1");
    app.focus = Pane::Conversations;
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Enter);
    let effects = app.update(Message::ConversationLoaded {
        request: 1,
        result: Ok(reading(3)),
    });
    assert!(effects.is_empty());
}

#[test]
fn a_failed_mark_says_so_and_leaves_the_conversation_unread() {
    let mut app = loaded();
    app.conversations[1] = unread("b", "folder-1");
    app.update(Message::MarkedRead {
        id: "b".into(),
        result: Err("offline".into()),
    });
    assert_eq!(
        app.notice.as_deref(),
        Some("Cannot mark the conversation read: offline")
    );
    assert_eq!(app.conversations[1].num_unread, 1);
}

#[test]
fn home_and_end_go_to_the_ends_of_a_list() {
    let mut app = loaded();
    app.focus = Pane::Conversations;
    press(&mut app, KeyCode::End);
    assert_eq!(app.selected_conversation, 1);
    press(&mut app, KeyCode::Home);
    assert_eq!(app.selected_conversation, 0);
    ctrl(&mut app, KeyCode::Down);
    assert_eq!(app.selected_conversation, 1);
    ctrl(&mut app, KeyCode::Up);
    assert_eq!(app.selected_conversation, 0);
}

#[test]
fn g_then_a_letter_goes_to_a_folder_as_in_proton_mail() {
    let mut app = loaded();
    app.places.push(place(label_ids::TRASH, "Trash"));
    assert!(press(&mut app, KeyCode::Char('g')).is_empty());
    assert!(app.going);
    let effects = press(&mut app, KeyCode::Char('t'));
    assert!(
        matches!(&effects[..], [Effect::Conversations { place, page: 0, .. }] if place == label_ids::TRASH),
        "{effects:?}"
    );
    assert!(!app.going);
    assert_eq!(
        (app.focus, app.listed.as_str()),
        (Pane::Conversations, "Trash")
    );
    assert_eq!(app.selected_place, 2);
    // All mail is not among the folders, and is listed all the same.
    press(&mut app, KeyCode::Char('g'));
    let effects = press(&mut app, KeyCode::Char('m'));
    assert!(
        matches!(&effects[..], [Effect::Conversations { place, .. }] if place == label_ids::ALL_MAIL),
        "{effects:?}"
    );
    assert_eq!((app.listed.as_str(), app.selected_place), ("All mail", 2));
    // A key that names no folder goes nowhere, and does nothing else.
    press(&mut app, KeyCode::Char('g'));
    assert!(press(&mut app, KeyCode::Char('q')).is_empty());
    assert!(!app.quit && !app.going);
    assert_eq!(app.listed, "All mail");
}

#[test]
fn proton_mail_keys_move_between_panes_mark_and_open_commands() {
    let mut app = loaded();
    // Right opens, as Enter does; Left goes back, but not past the
    // folders.
    press(&mut app, KeyCode::Right);
    assert_eq!(app.focus, Pane::Conversations);
    press(&mut app, KeyCode::Left);
    press(&mut app, KeyCode::Left);
    assert_eq!(app.focus, Pane::Folders);
    press(&mut app, KeyCode::Tab);
    app.update(Message::ConversationsLoaded {
        request: 2,
        page: 0,
        result: Ok((2, vec![conversation("a"), conversation("b")])),
    });
    // x marks as Space does; Ctrl+A marks all, then none.
    press(&mut app, KeyCode::Char('x'));
    assert_eq!(app.marked, ["a"]);
    ctrl(&mut app, KeyCode::Char('a'));
    assert_eq!(app.marked, ["a", "b"]);
    ctrl(&mut app, KeyCode::Char('a'));
    assert!(app.marked.is_empty());
    // i moves to the Inbox; m, Ctrl+K and Ctrl+F open the command line.
    let effects = press(&mut app, KeyCode::Char('i'));
    assert!(
        matches!(&effects[..], [Effect::Change { change: Change::Move(to), .. }] if to == label_ids::INBOX),
        "{effects:?}"
    );
    press(&mut app, KeyCode::Char('m'));
    assert_eq!(app.command_line.as_ref().unwrap().text, "move ");
    press(&mut app, KeyCode::Esc);
    ctrl(&mut app, KeyCode::Char('k'));
    assert_eq!(app.command_line.as_ref().unwrap().text, "");
    press(&mut app, KeyCode::Esc);
    ctrl(&mut app, KeyCode::Char('f'));
    assert_eq!(app.command_line.as_ref().unwrap().text, "search ");
}

#[test]
fn another_place_lists_from_its_top_whatever_was_selected_before() {
    let mut app = loaded();
    app.selected_place = 1;
    let effects = press(&mut app, KeyCode::Enter);
    let [Effect::Conversations { request, .. }] = effects[..] else {
        panic!("{effects:?}");
    };
    // Work holds the conversation the Inbox's selection was on, second.
    app.update(Message::ConversationsLoaded {
        request,
        page: 0,
        result: Ok((2, vec![conversation("w"), conversation("a")])),
    });
    assert_eq!(app.selected_conversation, 0);
}

#[test]
fn ctrl_r_reloads_the_folders_and_the_list_keeping_the_selection() {
    let mut app = loaded();
    app.selected_place = 1;
    app.focus = Pane::Conversations;
    press(&mut app, KeyCode::Down);
    let refresh = Event::Key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
    let effects = app.update(Message::from_event(&refresh).unwrap());
    assert_eq!(
        effects,
        [
            Effect::Places,
            Effect::Conversations {
                request: 2,
                place: label_ids::INBOX.into(),
                page: 0
            }
        ]
    );
    assert_eq!(app.notice.as_deref(), Some("Loading Inbox…"));
    // New mail came in above the selected conversation, b.
    app.update(Message::ConversationsLoaded {
        page: 0,
        request: 2,
        result: Ok((
            3,
            vec![conversation("new"), conversation("a"), conversation("b")],
        )),
    });
    assert_eq!(app.selected_conversation, 2);
    app.update(Message::PlacesLoaded(Ok(vec![
        place("folder-0", "Added"),
        place(label_ids::INBOX, "Inbox"),
        place("folder-1", "Work"),
    ])));
    assert_eq!(app.places[app.selected_place].name, "Work");
}

#[test]
fn down_at_the_end_of_a_longer_list_loads_its_next_page_below() {
    let mut app = loaded();
    app.total = 5;
    app.focus = Pane::Conversations;
    press(&mut app, KeyCode::Down);
    assert_eq!(app.selected_conversation, 1);
    let effects = press(&mut app, KeyCode::Down);
    assert_eq!(
        effects,
        [Effect::Conversations {
            request: 1,
            place: label_ids::INBOX.into(),
            page: 1
        }]
    );
    assert_eq!(app.notice.as_deref(), Some("Loading more of Inbox…"));
    // Only once at a time.
    assert!(press(&mut app, KeyCode::Down).is_empty());
    // "b" came back on this page as well, pushed down by new mail.
    app.update(Message::ConversationsLoaded {
        request: 1,
        page: 1,
        result: Ok((5, vec![conversation("b"), conversation("c")])),
    });
    let ids: Vec<&str> = app.conversations.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, ["a", "b", "c"]);
    assert_eq!(app.notice, None);
    assert_eq!(app.selected_conversation, 1);
    press(&mut app, KeyCode::Down);
    assert_eq!(
        press(&mut app, KeyCode::Down),
        [Effect::Conversations {
            request: 1,
            place: label_ids::INBOX.into(),
            page: 2
        }]
    );
}

#[test]
fn the_wheel_past_the_last_row_loads_the_next_page_and_moves_no_keys() {
    let mut app = loaded();
    app.total = 5;
    let mut drawn = Drawn::default();
    drawn.note(Pane::Conversations, Rect::new(25, 1, 46, 27), 0);
    app.drawn.set(drawn);
    let wheel = |row| {
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 30,
            row,
            modifiers: KeyModifiers::NONE,
        })
    };
    let over = |app: &mut App, row| {
        app.message_for(&wheel(row))
            .map_or_else(Vec::new, |message| app.update(message))
    };
    // Both rows show, so the list has nowhere to scroll: its next page
    // loads, and the selection stays where it is.
    assert_eq!(
        over(&mut app, 3),
        [Effect::Conversations {
            request: 1,
            place: label_ids::INBOX.into(),
            page: 1
        }]
    );
    assert_eq!(app.selected_conversation, 0);
    assert_eq!(app.focus, Pane::Folders);
    // Over no pane, it does nothing.
    assert!(app.message_for(&wheel(40)).is_some());
    assert!(over(&mut app, 40).is_empty());
    assert_eq!(app.selected_conversation, 0);

    // A message being written, or a question, takes no clicks.
    let click = Event::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 30,
        row: 1,
        modifiers: KeyModifiers::NONE,
    });
    assert!(matches!(
        app.message_for(&click),
        Some(Message::Click {
            pane: Pane::Conversations,
            row: 0
        })
    ));
    app.compose = Some(Compose::new(String::new()));
    assert!(app.message_for(&click).is_none());
    app.compose = None;
    app.question = Some(Question::Discard);
    assert!(app.message_for(&click).is_none());
}

#[test]
fn a_list_with_nothing_more_to_it_just_stays_at_its_end() {
    let mut app = loaded();
    app.focus = Pane::Conversations;
    press(&mut app, KeyCode::Down);
    assert!(press(&mut app, KeyCode::Down).is_empty());
    assert_eq!(app.selected_conversation, 1);
}

#[test]
fn a_next_page_for_a_list_since_replaced_comes_too_late() {
    let mut app = loaded();
    app.total = 5;
    app.focus = Pane::Conversations;
    press(&mut app, KeyCode::End);
    assert_eq!(press(&mut app, KeyCode::Down).len(), 1);
    let refresh = Event::Key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
    app.update(Message::from_event(&refresh).unwrap());
    app.update(Message::ConversationsLoaded {
        request: 1,
        page: 1,
        result: Ok((5, vec![conversation("late")])),
    });
    assert!(app.conversations.iter().all(|c| c.id != "late"));
    // A failed next page can be asked for again.
    app.update(Message::ConversationsLoaded {
        request: 2,
        page: 0,
        result: Ok((5, vec![conversation("a"), conversation("b")])),
    });
    press(&mut app, KeyCode::End);
    press(&mut app, KeyCode::Down);
    app.update(Message::ConversationsLoaded {
        request: 2,
        page: 1,
        result: Err("offline".into()),
    });
    assert_eq!(
        app.notice.as_deref(),
        Some("Cannot load more of Inbox: offline")
    );
    assert_eq!(press(&mut app, KeyCode::Down).len(), 1);
}

#[test]
fn a_list_whose_first_page_is_on_its_way_asks_for_no_more_yet() {
    let mut app = loaded();
    app.total = 5;
    // Work is asked for; the Inbox's rows stay until its answer comes.
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::End);
    assert!(press(&mut app, KeyCode::Down).is_empty());
    app.update(Message::ConversationsLoaded {
        request: 2,
        page: 0,
        result: Ok((5, vec![conversation("w1"), conversation("w2")])),
    });
    press(&mut app, KeyCode::End);
    assert_eq!(
        press(&mut app, KeyCode::Down),
        [Effect::Conversations {
            request: 2,
            place: "folder-1".into(),
            page: 1
        }]
    );
}

#[test]
fn up_and_down_move_the_focused_list_only() {
    let mut app = loaded();
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Down);
    assert_eq!((app.selected_place, app.selected_conversation), (0, 1));
    press(&mut app, KeyCode::Up);
    press(&mut app, KeyCode::Char('k'));
    assert_eq!(app.selected_conversation, 0);
}

#[test]
fn tab_goes_through_the_panes_and_shift_tab_comes_back() {
    let mut app = App::new("demo".into());
    let mut seen = Vec::new();
    for _ in 0..3 {
        seen.push(app.focus);
        press(&mut app, KeyCode::Tab);
    }
    assert_eq!(seen, Pane::ALL);
    assert_eq!(app.focus, Pane::Folders);
    press(&mut app, KeyCode::BackTab);
    assert_eq!(app.focus, Pane::Reader);
}

#[test]
fn a_released_key_does_nothing() {
    let mut release = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
    release.kind = KeyEventKind::Release;
    assert!(Message::from_event(&Event::Key(release)).is_none());
    // A press, and a held key's repeat, still count.
    release.kind = KeyEventKind::Repeat;
    assert!(matches!(
        Message::from_event(&Event::Key(release)),
        Some(Message::Quit)
    ));
}

#[test]
fn q_and_ctrl_c_quit() {
    let mut app = App::new("demo".into());
    press(&mut app, KeyCode::Char('q'));
    assert!(app.quit);

    let ctrl_c = Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(matches!(Message::from_event(&ctrl_c), Some(Message::Quit)));
    // Other control keys ask for nothing yet, Ctrl+Q included.
    let ctrl_q = Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL));
    assert!(Message::from_event(&ctrl_q).is_none());
}

#[test]
fn the_keys_open_with_a_question_mark_and_hold_everything_still_while_open() {
    let mut app = loaded();
    press(&mut app, KeyCode::Char('?'));
    assert!(app.help);
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Down);
    assert!(press(&mut app, KeyCode::Enter).is_empty());
    assert_eq!((app.focus, app.selected_place), (Pane::Folders, 0));
    // Answers still land while it is open.
    app.update(Message::PlacesLoaded(Ok(vec![place("x", "X")])));
    assert_eq!(app.places.len(), 1);
    press(&mut app, KeyCode::Esc);
    assert!(!app.help);
    press(&mut app, KeyCode::Char('?'));
    press(&mut app, KeyCode::Char('?'));
    assert!(!app.help);
}

#[test]
fn the_open_keys_scroll_where_they_do_not_fit_and_open_again_at_their_top() {
    let mut app = loaded();
    press(&mut app, KeyCode::Char('?'));
    // As the view leaves them after drawing 30 rows 10 at a time.
    app.help_bottom.set(20);
    app.help_rows.set(10);
    press(&mut app, KeyCode::Char('j'));
    assert_eq!(app.help_scroll, 1);
    press(&mut app, KeyCode::PageDown);
    assert_eq!(app.help_scroll, 11);
    press(&mut app, KeyCode::End);
    assert_eq!(app.help_scroll, 20);
    press(&mut app, KeyCode::Down);
    assert_eq!(app.help_scroll, 20);
    press(&mut app, KeyCode::PageUp);
    press(&mut app, KeyCode::Up);
    assert_eq!(app.help_scroll, 9);
    // The panes keep still meanwhile.
    assert_eq!((app.focus, app.selected_place), (Pane::Folders, 0));
    press(&mut app, KeyCode::Char('?'));
    command(&mut app, "help");
    assert_eq!((app.help, app.help_scroll), (true, 0));
    press(&mut app, KeyCode::End);
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Char('?'));
    assert_eq!(app.help_scroll, 0);
}

#[test]
fn o_lists_the_links_and_only_a_yes_opens_the_one_selected() {
    let mut app = loaded();
    press(&mut app, KeyCode::Char('b'));
    assert_eq!(
        app.notice.as_deref(),
        Some("Open a conversation to list its links.")
    );
    app.reading = Some(reading(1));
    press(&mut app, KeyCode::Char('b'));
    assert_eq!(
        app.notice.as_deref(),
        Some("This conversation has no links.")
    );

    let mut linked = reading(1);
    linked.links = vec![
        "https://example.test/a".into(),
        "https://example.test/b".into(),
    ];
    app.reading = Some(linked);
    press(&mut app, KeyCode::Char('b'));
    // The keys move through the links now, not the folders.
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Down);
    assert_eq!(app.selected_place, 0);
    let asked = LinkPick {
        selected: 1,
        confirming: true,
    };
    assert!(press(&mut app, KeyCode::Enter).is_empty());
    assert_eq!(app.picking_link, Some(asked));
    // Any other key goes back to the links, one that does nothing
    // elsewhere and q too.
    for other in ['j', 'n', 'q'] {
        assert!(press(&mut app, KeyCode::Char(other)).is_empty());
        assert_eq!(app.picking_link.map(|pick| pick.confirming), Some(false));
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.picking_link, Some(asked));
    }
    assert!(!app.quit);
    assert_eq!(
        press(&mut app, KeyCode::Char('y')),
        [Effect::OpenLink {
            url: "https://example.test/b".into()
        }]
    );
    assert_eq!(app.picking_link, None);

    app.update(Message::LinkOpened(Err("no browser".into())));
    assert_eq!(
        app.notice.as_deref(),
        Some("Cannot open the link: no browser")
    );
    // Esc puts the links away, and y alone asks nothing.
    press(&mut app, KeyCode::Char('b'));
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.picking_link, None);
    assert!(press(&mut app, KeyCode::Char('y')).is_empty());
}

#[test]
fn v_lists_the_attachments_and_enter_saves_the_one_selected() {
    let mut app = loaded();
    press(&mut app, KeyCode::Char('v'));
    assert_eq!(
        app.notice.as_deref(),
        Some("Open a conversation to list its attachments.")
    );
    app.reading = Some(reading(1));
    command(&mut app, "attachments");
    assert_eq!(
        app.notice.as_deref(),
        Some("This conversation has no attachments.")
    );

    let mut attached = reading(1);
    attached.attachments = ["plan.pdf", "budget.xlsx"]
        .iter()
        .enumerate()
        .map(|(index, name)| super::super::reading::ReadingAttachment {
            message_id: "m1".into(),
            id: format!("a{index}"),
            name: (*name).into(),
            size: 1_000,
            inline: false,
        })
        .collect();
    app.reading = Some(attached);
    press(&mut app, KeyCode::Char('v'));
    assert_eq!(app.picking_attachment, Some(0));
    // The keys move through the attachments, not the folders.
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Down);
    assert_eq!((app.picking_attachment, app.selected_place), (Some(1), 0));
    assert_eq!(
        press(&mut app, KeyCode::Enter),
        [Effect::SaveAttachment {
            message_id: "m1".into(),
            attachment_id: "a1".into(),
            name: "budget.xlsx".into()
        }]
    );
    assert_eq!(app.picking_attachment, None);
    assert_eq!(app.notice.as_deref(), Some("Saving budget.xlsx…"));
    app.update(Message::AttachmentSaved {
        name: "budget.xlsx".into(),
        result: Ok("/home/me/Downloads/budget (1).xlsx".into()),
    });
    assert_eq!(
        app.notice.as_deref(),
        Some("Saved budget.xlsx as /home/me/Downloads/budget (1).xlsx")
    );
    app.update(Message::AttachmentSaved {
        name: "budget.xlsx".into(),
        result: Err("attachment signature could not be verified".into()),
    });
    assert_eq!(
        app.notice.as_deref(),
        Some("Cannot save budget.xlsx: attachment signature could not be verified")
    );
    // Esc puts them away.
    press(&mut app, KeyCode::Char('v'));
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.picking_attachment, None);
}

#[test]
fn a_conversation_that_loads_puts_away_the_links_of_the_one_before() {
    let mut app = loaded();
    let mut linked = reading(1);
    linked.links = vec!["https://example.test/a".into()];
    app.reading = Some(linked);
    press(&mut app, KeyCode::Char('b'));
    app.update(Message::ConversationLoaded {
        request: 0,
        result: Ok(reading(2)),
    });
    assert_eq!(app.picking_link, None);
}

#[test]
fn the_quick_actions_change_the_conversation_acted_on_then_reload() {
    let mut app = loaded();
    app.conversations[1] = unread("b", "folder-1");
    // From the folders, nothing is acted on.
    assert!(press(&mut app, KeyCode::Char('*')).is_empty());
    assert_eq!(
        app.notice.as_deref(),
        Some("Choose a conversation in the list first.")
    );
    app.focus = Pane::Conversations;
    press(&mut app, KeyCode::Down);
    let change = |change: Change| {
        vec![Effect::Change {
            ids: vec!["b".into()],
            place: label_ids::INBOX.into(),
            change,
        }]
    };
    assert_eq!(
        press(&mut app, KeyCode::Char('*')),
        change(Change::Star(true))
    );
    assert_eq!(app.notice.as_deref(), Some("Starring the conversation…"));
    assert_eq!(
        press(&mut app, KeyCode::Char('r')),
        change(Change::Read(true))
    );
    assert_eq!(
        press(&mut app, KeyCode::Char('u')),
        change(Change::Read(false))
    );
    let archive = Change::Move(label_ids::ARCHIVE.into());
    assert_eq!(press(&mut app, KeyCode::Char('a')), change(archive));
    let trash = Change::Move(label_ids::TRASH.into());
    assert_eq!(press(&mut app, KeyCode::Char('t')), change(trash.clone()));
    assert_eq!(press(&mut app, KeyCode::Delete), change(trash.clone()));

    // Done, the folders and the list load again.
    let effects = app.update(Message::Changed {
        change: trash.clone(),
        ids: vec!["b".into()],
        place: label_ids::INBOX.into(),
        result: Ok(()),
    });
    assert_eq!(
        effects,
        [
            Effect::Places,
            Effect::Conversations {
                request: 2,
                place: label_ids::INBOX.into(),
                page: 0
            }
        ]
    );
    // Meanwhile the rows are the old ones: nothing is acted on.
    assert!(press(&mut app, KeyCode::Char('*')).is_empty());
    assert_eq!(app.notice.as_deref(), Some("Wait for the list to load."));
    app.update(Message::ConversationsLoaded {
        request: 2,
        page: 0,
        result: Ok((2, vec![conversation("a"), unread("b", "folder-1")])),
    });
    app.update(Message::Changed {
        change: trash,
        ids: vec!["b".into()],
        place: label_ids::INBOX.into(),
        result: Err("offline".into()),
    });
    assert_eq!(
        app.notice.as_deref(),
        Some("Cannot move the conversation to Trash: offline")
    );

    // In the reader, the conversation it shows is acted on.
    app.focus = Pane::Reader;
    app.selected_conversation = 0;
    app.conversations[1].labels.push(ConversationLabel {
        id: label_ids::STARRED.into(),
        ..ConversationLabel::default()
    });
    app.reading = Some(reading(1));
    assert_eq!(
        press(&mut app, KeyCode::Char('*')),
        change(Change::Star(false))
    );
    // In the reader, r is Proton's reply, not marking read.
    assert!(press(&mut app, KeyCode::Char('r')).is_empty());
}

#[test]
fn space_marks_conversations_and_an_action_acts_on_all_marked() {
    let mut app = loaded();
    app.conversations.push(unread("c", "folder-1"));
    app.total = 3;
    app.focus = Pane::Conversations;
    // Space marks and moves down, so several mark in a row.
    press(&mut app, KeyCode::Char(' '));
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Char(' '));
    assert_eq!(app.marked, ["a", "c"]);
    assert_eq!(app.selected_conversation, 2);
    // Space on a marked one takes its mark away.
    press(&mut app, KeyCode::Char('k'));
    press(&mut app, KeyCode::Char('k'));
    press(&mut app, KeyCode::Char(' '));
    press(&mut app, KeyCode::Char(' '));
    assert_eq!(app.marked, ["c", "b"]);

    // Marked: all are marked read, in the list's order.
    assert_eq!(
        press(&mut app, KeyCode::Char('r')),
        [Effect::Change {
            ids: vec!["b".into(), "c".into()],
            place: label_ids::INBOX.into(),
            change: Change::Read(true)
        }]
    );
    assert_eq!(app.notice.as_deref(), Some("Marking 2 conversations read…"));
    // None starred: all are starred.
    let effects = press(&mut app, KeyCode::Char('*'));
    assert!(
        matches!(&effects[..], [Effect::Change { change: Change::Star(true), ids, .. }] if ids.len() == 2),
        "{effects:?}"
    );
    app.update(Message::Changed {
        change: Change::Star(true),
        ids: vec!["b".into(), "c".into()],
        place: label_ids::INBOX.into(),
        result: Err("offline".into()),
    });
    assert_eq!(
        app.notice.as_deref(),
        Some("Cannot star 2 conversations: offline")
    );
    // From the reader, the open conversation alone is acted on.
    app.focus = Pane::Reader;
    app.reading = Some(reading(1));
    let effects = press(&mut app, KeyCode::Char('a'));
    assert!(
        matches!(&effects[..], [Effect::Change { ids, .. }] if ids == &["b".to_owned()]),
        "{effects:?}"
    );
    // Esc in the list takes the marks away before going back.
    app.focus = Pane::Conversations;
    press(&mut app, KeyCode::Esc);
    assert!(app.marked.is_empty());
    assert_eq!(app.focus, Pane::Conversations);
    // A change made, the list loads again without them.
    press(&mut app, KeyCode::Char(' '));
    app.update(Message::Changed {
        change: Change::Star(true),
        ids: vec!["b".into()],
        place: label_ids::INBOX.into(),
        result: Ok(()),
    });
    assert!(app.marked.is_empty());
}

#[test]
fn labels_go_on_and_off_by_name_and_spam_goes_both_ways() {
    let mut app = loaded();
    app.places = vec![
        place(label_ids::INBOX, "Inbox"),
        place(label_ids::SPAM, "Spam"),
        Place {
            kind: PlaceKind::Folder,
            ..place("folder-1", "Work")
        },
        Place {
            kind: PlaceKind::Label,
            ..place("label-1", "Family")
        },
    ];
    app.focus = Pane::Conversations;
    let change = |change: Change| {
        vec![Effect::Change {
            ids: vec!["a".into()],
            place: label_ids::INBOX.into(),
            change,
        }]
    };
    assert_eq!(
        command(&mut app, "label family"),
        change(Change::Label("label-1".into()))
    );
    assert_eq!(app.notice.as_deref(), Some("Labelling the conversation…"));
    // Only a label it bears comes off, as Proton reports once it has.
    assert!(command(&mut app, "unlabel Family").is_empty());
    assert_eq!(
        app.notice.as_deref(),
        Some("No conversation chosen has the label Family.")
    );
    app.conversations[0].labels = vec![ConversationLabel {
        id: "label-1".into(),
        ..ConversationLabel::default()
    }];
    assert_eq!(
        command(&mut app, "unlabel Family"),
        change(Change::Unlabel("label-1".into()))
    );
    assert!(command(&mut app, "label Work").is_empty());
    assert_eq!(
        app.notice.as_deref(),
        Some("Work is a folder: :move moves mail to it")
    );
    assert!(command(&mut app, "label Inbox").is_empty());
    assert_eq!(app.notice.as_deref(), Some("Inbox is not a label"));
    // l starts the line at :label, and Tab offers only labels.
    press(&mut app, KeyCode::Char('l'));
    assert_eq!(app.command_line.as_ref().unwrap().text, "label ");
    press(&mut app, KeyCode::Tab);
    assert_eq!(app.command_line.as_ref().unwrap().text, "label Family");
    press(&mut app, KeyCode::Esc);

    // s moves to Spam, and i to the Inbox.
    assert_eq!(
        press(&mut app, KeyCode::Char('s')),
        change(Change::Move(label_ids::SPAM.into()))
    );
    assert_eq!(
        app.notice.as_deref(),
        Some("Moving the conversation to Spam…")
    );
    assert_eq!(
        command(&mut app, "notspam"),
        change(Change::Move(label_ids::INBOX.into()))
    );
    app.focus = Pane::Folders;
    app.selected_place = 1;
    press(&mut app, KeyCode::Enter);
    app.update(Message::ConversationsLoaded {
        request: 2,
        page: 0,
        result: Ok((1, vec![conversation("junk")])),
    });
    assert_eq!(
        press(&mut app, KeyCode::Char('i')),
        [Effect::Change {
            ids: vec!["junk".into()],
            place: label_ids::SPAM.into(),
            change: Change::Move(label_ids::INBOX.into())
        }]
    );
    assert_eq!(
        app.notice.as_deref(),
        Some("Moving the conversation to the Inbox…")
    );
}

#[test]
fn a_row_names_the_labels_of_its_conversation_but_the_one_listed() {
    let mut app = loaded();
    let label = |id: &str, name: &str| Place {
        kind: PlaceKind::Label,
        ..place(id, name)
    };
    app.places = vec![
        place(label_ids::INBOX, "Inbox"),
        label("label-1", "Family"),
        label("label-2", "Travel"),
    ];
    app.conversations[0].labels = [label_ids::INBOX, "label-2", "label-1"]
        .map(|id| ConversationLabel {
            id: id.into(),
            ..ConversationLabel::default()
        })
        .to_vec();
    assert_eq!(app.row_labels(&app.conversations[0]), ["Family", "Travel"]);
    // Every row under Family bears it.
    command(&mut app, "folder Family");
    assert_eq!(app.row_labels(&app.conversations[0]), ["Travel"]);
}

#[test]
fn shift_l_takes_off_one_of_the_labels_the_conversation_bears() {
    let mut app = loaded();
    let label = |id: &str, name: &str| Place {
        kind: PlaceKind::Label,
        ..place(id, name)
    };
    app.places = vec![
        place(label_ids::INBOX, "Inbox"),
        label("label-1", "Family"),
        label("label-2", "Travel"),
        label("label-3", "Work"),
    ];
    press(&mut app, KeyCode::Char('L'));
    assert!(app.command_line.is_none());
    assert_eq!(
        app.notice.as_deref(),
        Some("Choose a conversation in the list first.")
    );
    app.focus = Pane::Conversations;
    press(&mut app, KeyCode::Char('L'));
    assert!(app.command_line.is_none());
    assert_eq!(
        app.notice.as_deref(),
        Some("There is no label to take off.")
    );
    // Tab offers only the labels it bears, in the folder list's order.
    app.conversations[0].labels = ["label-3", label_ids::INBOX, "label-2"]
        .map(|id| ConversationLabel {
            id: id.into(),
            ..ConversationLabel::default()
        })
        .to_vec();
    press(&mut app, KeyCode::Char('L'));
    assert_eq!(app.command_line.as_ref().unwrap().text, "unlabel ");
    let mut offered = Vec::new();
    for _ in 0..3 {
        press(&mut app, KeyCode::Tab);
        offered.push(app.command_line.as_ref().unwrap().text.clone());
    }
    assert_eq!(
        offered,
        ["unlabel Travel", "unlabel Work", "unlabel Travel"]
    );
    assert_eq!(
        press(&mut app, KeyCode::Enter),
        [Effect::Change {
            ids: vec!["a".into()],
            place: label_ids::INBOX.into(),
            change: Change::Unlabel("label-2".into())
        }]
    );
}

#[test]
fn shift_z_takes_the_last_move_out_of_a_folder_back_until_something_else_happens() {
    let mut app = loaded();
    app.places = vec![
        place(label_ids::INBOX, "Inbox"),
        place(label_ids::ARCHIVE, "Archive"),
        Place {
            kind: PlaceKind::Folder,
            ..place("folder-1", "Work")
        },
        Place {
            kind: PlaceKind::Label,
            ..place("label-1", "Family")
        },
    ];
    app.focus = Pane::Conversations;
    let done = |app: &mut App, change: Change, place: &str| {
        app.update(Message::Changed {
            change,
            ids: vec!["a".into(), "b".into()],
            place: place.into(),
            result: Ok(()),
        });
    };
    let archived = |app: &mut App| {
        let listed = app.listed_id.clone();
        done(app, Change::Move(label_ids::ARCHIVE.into()), &listed);
    };
    assert!(press(&mut app, KeyCode::Char('Z')).is_empty());
    assert_eq!(
        app.notice.as_deref(),
        Some("There is no move to take back.")
    );
    // Moved out of the Inbox, back they go, once, the list loaded again
    // meanwhile.
    archived(&mut app);
    let back = [Effect::Change {
        ids: vec!["a".into(), "b".into()],
        place: label_ids::INBOX.into(),
        change: Change::Move(label_ids::INBOX.into()),
    }];
    assert_eq!(press(&mut app, KeyCode::Char('Z')), back);
    assert_eq!(
        app.notice.as_deref(),
        Some("Moving 2 conversations to the Inbox…")
    );
    assert!(press(&mut app, KeyCode::Char('Z')).is_empty());
    archived(&mut app);
    assert_eq!(command(&mut app, "undo"), back);
    // Another change since, or another place listed, leaves none.
    archived(&mut app);
    done(&mut app, Change::Star(true), label_ids::INBOX);
    assert_eq!(app.undo, None);
    archived(&mut app);
    ctrl(&mut app, KeyCode::Char('r'));
    assert!(app.undo.is_some());
    command(&mut app, "folder Work");
    assert_eq!(app.undo, None);
    // A move from another place than the one listed is not taken back.
    done(
        &mut app,
        Change::Move(label_ids::ARCHIVE.into()),
        label_ids::INBOX,
    );
    assert_eq!(app.undo, None);
    // From a folder of the account's own it is.
    archived(&mut app);
    assert!(app.undo.is_some());
    // Under a label, a conversation's folder is not known.
    command(&mut app, "folder Family");
    archived(&mut app);
    assert_eq!(app.undo, None);
}

#[test]
fn deleting_forever_asks_first_and_only_a_yes_deletes() {
    let mut app = loaded();
    app.places.push(place(label_ids::TRASH, "Trash"));
    app.focus = Pane::Conversations;
    // Nothing outside Trash and Spam is deleted forever.
    assert!(ctrl(&mut app, KeyCode::Backspace).is_empty());
    assert_eq!(app.question, None);
    assert_eq!(
        app.notice.as_deref(),
        Some("Only mail in Trash or Spam can be deleted forever.")
    );
    assert!(command(&mut app, "empty").is_empty());
    assert_eq!(app.question, None);
    assert_eq!(
        app.notice.as_deref(),
        Some("Only Trash and Spam can be emptied.")
    );

    let effects = command(&mut app, "folder trash");
    let [Effect::Conversations { request, .. }] = effects[..] else {
        panic!("{effects:?}");
    };
    let old = |id: &str| Conversation {
        subject: format!("About {id}"),
        ..conversation(id)
    };
    app.update(Message::ConversationsLoaded {
        request,
        page: 0,
        result: Ok((2, vec![old("x"), old("y")])),
    });
    // Ctrl+Backspace asks about the selected conversation; any key but
    // y keeps it.
    assert!(ctrl(&mut app, KeyCode::Backspace).is_empty());
    assert_eq!(
        app.question,
        Some(Question::Delete {
            ids: vec!["x".into()],
            subjects: vec!["About x".into()],
            place: label_ids::TRASH.into(),
            name: "Trash".into(),
        })
    );
    assert!(press(&mut app, KeyCode::Char('n')).is_empty());
    assert_eq!(app.question, None);
    assert_eq!(app.notice.as_deref(), Some("Nothing was deleted."));
    // q too, which quits only once nothing is asked; Ctrl+H asks as
    // Ctrl+Backspace does.
    ctrl(&mut app, KeyCode::Char('h'));
    assert!(app.question.is_some());
    let ctrl_y = Event::Key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL));
    assert!(matches!(app.message_for(&ctrl_y), Some(Message::Back)));
    assert!(press(&mut app, KeyCode::Char('q')).is_empty());
    assert!(!app.quit);
    assert_eq!(app.question, None);
    // With some marked, about all of them; Esc keeps them, marks and
    // all.
    press(&mut app, KeyCode::Char(' '));
    press(&mut app, KeyCode::Char(' '));
    ctrl(&mut app, KeyCode::Backspace);
    assert!(
        matches!(&app.question, Some(Question::Delete { ids, .. }) if ids == &["x", "y"]),
        "{:?}",
        app.question
    );
    assert!(press(&mut app, KeyCode::Esc).is_empty());
    assert_eq!(app.question, None);
    assert_eq!(app.marked, ["x", "y"]);
    // A yes deletes them, and the list loads again.
    assert!(command(&mut app, "delete").is_empty());
    assert_eq!(
        press(&mut app, KeyCode::Char('y')),
        [Effect::Change {
            ids: vec!["x".into(), "y".into()],
            place: label_ids::TRASH.into(),
            change: Change::Delete,
        }]
    );
    assert_eq!(app.question, None);
    assert_eq!(
        app.notice.as_deref(),
        Some("Deleting 2 conversations forever…")
    );
    app.update(Message::Changed {
        change: Change::Delete,
        ids: vec!["x".into(), "y".into()],
        place: label_ids::TRASH.into(),
        result: Err("offline".into()),
    });
    assert_eq!(
        app.notice.as_deref(),
        Some("Cannot delete 2 conversations forever: offline")
    );
    let effects = app.update(Message::Changed {
        change: Change::Delete,
        ids: vec!["x".into(), "y".into()],
        place: label_ids::TRASH.into(),
        result: Ok(()),
    });
    assert_eq!(effects[0], Effect::Places);

    // :empty asks about all that is in the place listed.
    assert!(command(&mut app, "empty").is_empty());
    assert_eq!(
        app.question,
        Some(Question::Empty {
            place: label_ids::TRASH.into(),
            name: "Trash".into(),
        })
    );
    assert_eq!(
        press(&mut app, KeyCode::Char('y')),
        [Effect::Empty {
            place: label_ids::TRASH.into(),
            name: "Trash".into(),
        }]
    );
    assert_eq!(app.notice.as_deref(), Some("Emptying Trash…"));
    app.update(Message::Emptied {
        name: "Trash".into(),
        result: Err("offline".into()),
    });
    assert_eq!(app.notice.as_deref(), Some("Cannot empty Trash: offline"));
    let effects = app.update(Message::Emptied {
        name: "Trash".into(),
        result: Ok(()),
    });
    assert!(
        matches!(&effects[..], [Effect::Places, Effect::Conversations { place, .. }] if place == label_ids::TRASH),
        "{effects:?}"
    );

    // A search's results hold mail from anywhere.
    command(&mut app, "search news");
    ctrl(&mut app, KeyCode::Backspace);
    assert_eq!(app.question, None);
    assert_eq!(
        app.notice.as_deref(),
        Some("Only mail in Trash or Spam can be deleted forever.")
    );
}

/// Types `text` where the keys go.
fn write(app: &mut App, text: &str) {
    for c in text.chars() {
        press(app, KeyCode::Char(c));
    }
}

#[test]
fn n_opens_a_new_message_that_takes_every_key_until_it_closes() {
    let mut app = loaded();
    app.sender = "me@example.test".into();
    assert!(press(&mut app, KeyCode::Char('n')).is_empty());
    assert_eq!(app.compose.as_ref().unwrap().from, "me@example.test");
    // q types instead of quitting.
    press(&mut app, KeyCode::Char('q'));
    assert!(!app.quit);
    assert_eq!(app.compose.as_ref().unwrap().to.text(), "q");
    // Written, Esc asks first, and any other key goes back to it.
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.question, Some(Question::Discard));
    press(&mut app, KeyCode::Char('n'));
    assert_eq!(app.question, None);
    assert_eq!(app.compose.as_ref().unwrap().to.text(), "q");
    // Ctrl+C asks as Esc does, and y discards it.
    ctrl(&mut app, KeyCode::Char('c'));
    assert_eq!(app.question, Some(Question::Discard));
    press(&mut app, KeyCode::Char('y'));
    assert!(app.compose.is_none());
    assert!(!app.quit);
    // Untouched, it closes at once; Ctrl+N and :compose open one too.
    ctrl(&mut app, KeyCode::Char('n'));
    press(&mut app, KeyCode::Esc);
    assert!(app.compose.is_none() && app.question.is_none());
    command(&mut app, "compose");
    assert!(app.compose.is_some());
}

#[test]
fn only_the_first_hundred_recipients_are_looked_up_before_sending() {
    let mut app = loaded();
    press(&mut app, KeyCode::Char('n'));
    let addresses: Vec<String> = (0..101).map(|n| format!("p{n}@proton.me")).collect();
    write(&mut app, &addresses.join(", "));
    let effects = ctrl(&mut app, KeyCode::Enter);
    let [
        Effect::Protection {
            addresses: asked, ..
        },
    ] = effects.as_slice()
    else {
        panic!("{effects:?}");
    };
    assert_eq!(asked.as_slice(), &addresses[..PROTECTION_LOOKUP_LIMIT]);
    let Some(Question::Send {
        outgoing, checks, ..
    }) = &app.question
    else {
        panic!("{:?}", app.question);
    };
    assert_eq!(checks.len(), PROTECTION_LOOKUP_LIMIT);
    // The message still goes to every one of them.
    assert_eq!(outgoing.recipients().len(), 101);
}

#[test]
fn a_message_goes_only_after_a_yes_to_who_gets_it_and_how() {
    let mut app = loaded();
    press(&mut app, KeyCode::Char('n'));
    // With nobody to send it to, it says so and asks nothing.
    assert!(ctrl(&mut app, KeyCode::Char('s')).is_empty());
    assert_eq!(app.question, None);
    let said = |app: &App| app.compose.as_ref().unwrap().said().map(str::to_owned);
    assert_eq!(said(&app).as_deref(), Some("Add someone to send this to."));
    write(&mut app, "sam@proton.me, alex@example.test");
    let addresses = vec!["sam@proton.me".to_owned(), "alex@example.test".to_owned()];
    assert_eq!(
        ctrl(&mut app, KeyCode::Enter),
        [Effect::Protection {
            request: 1,
            addresses: addresses.clone(),
        }]
    );
    let Some(Question::Send {
        outgoing, checks, ..
    }) = &app.question
    else {
        panic!("{:?}", app.question);
    };
    assert_eq!(outgoing.to, addresses);
    assert!(
        checks
            .iter()
            .all(|(_, checked)| *checked == Checked::Checking)
    );
    // An answer to a question asked before comes too late.
    app.update(Message::Protected {
        request: 0,
        checks: vec![("sam@proton.me".into(), Ok(RecipientProtection::Unencrypted))],
    });
    app.update(Message::Protected {
        request: 1,
        checks: vec![
            ("sam@proton.me".into(), Ok(RecipientProtection::EndToEnd)),
            ("alex@example.test".into(), Err("offline".into())),
        ],
    });
    let Some(Question::Send { checks, .. }) = &app.question else {
        panic!("{:?}", app.question);
    };
    assert_eq!(
        checks,
        &[
            (
                "sam@proton.me".to_owned(),
                Checked::Known(RecipientProtection::EndToEnd)
            ),
            ("alex@example.test".to_owned(), Checked::Unknown),
        ]
    );
    // Any key but y goes back to the message; Ctrl+J asks again.
    press(&mut app, KeyCode::Char('n'));
    assert!(app.question.is_none() && app.compose.is_some());
    let effects = ctrl(&mut app, KeyCode::Char('j'));
    assert!(
        matches!(&effects[..], [Effect::Protection { request: 2, .. }]),
        "{effects:?}"
    );
    let effects = press(&mut app, KeyCode::Char('y'));
    assert!(
        matches!(&effects[..], [Effect::Send(outgoing)] if outgoing.to == addresses),
        "{effects:?}"
    );
    assert_eq!(said(&app).as_deref(), Some("Sending…"));
    // On its way, nothing changes it or closes it.
    press(&mut app, KeyCode::Char('x'));
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.question, None);
    assert_eq!(
        app.compose.as_ref().unwrap().to.text(),
        "sam@proton.me, alex@example.test"
    );

    // Not sent: it stays, saying why.
    app.update(Message::Sent(Err(SendFailure::Failed("offline".into()))));
    assert_eq!(said(&app).as_deref(), Some("Not sent: offline"));
    // Perhaps sent: it says to check Sent, until the message goes.
    ctrl(&mut app, KeyCode::Char('s'));
    press(&mut app, KeyCode::Char('y'));
    app.update(Message::Sent(Err(SendFailure::Unconfirmed)));
    let unconfirmed = Some(super::super::compose::UNCONFIRMED);
    assert_eq!(said(&app).as_deref(), unconfirmed);
    write(&mut app, ",");
    ctrl(&mut app, KeyCode::Char('s'));
    press(&mut app, KeyCode::Char('y'));
    assert_eq!(said(&app).as_deref(), Some("Sending…"));
    // Sent: the editor closes, the folders and the list load again,
    // and the status bar says so before and after.
    let effects = app.update(Message::Sent(Ok(())));
    let [Effect::Places, Effect::Conversations { request, .. }] = effects[..] else {
        panic!("{effects:?}");
    };
    assert!(app.compose.is_none());
    assert_eq!(app.notice.as_deref(), Some("Sent."));
    app.update(Message::ConversationsLoaded {
        request,
        page: 0,
        result: Ok((2, vec![conversation("a"), conversation("b")])),
    });
    assert_eq!(app.notice.as_deref(), Some("Sent."));
}

#[test]
fn a_file_is_checked_before_it_is_attached_and_a_late_answer_is_dropped() {
    let mut app = loaded();
    press(&mut app, KeyCode::Char('n'));
    ctrl(&mut app, KeyCode::Char('a'));
    write(&mut app, "notes");
    assert_eq!(
        press(&mut app, KeyCode::Tab),
        [Effect::Complete {
            request: 1,
            typed: "notes".into(),
        }]
    );
    write(&mut app, ".txt");
    assert_eq!(
        press(&mut app, KeyCode::Enter),
        [Effect::Attach {
            request: 2,
            typed: "notes.txt".into(),
            attached: Vec::new(),
        }]
    );
    // The completion asked for first comes too late.
    app.update(Message::Completions {
        request: 1,
        typed: "notes".into(),
        matches: vec!["notes/".into()],
    });
    let compose = app.compose.as_ref().unwrap();
    assert_eq!(compose.files.as_ref().unwrap().path.text(), "notes.txt");
    app.update(Message::Attached {
        request: 2,
        result: Ok((PathBuf::from("notes.txt"), 12)),
    });
    let compose = app.compose.as_ref().unwrap();
    assert_eq!(compose.attached, [(PathBuf::from("notes.txt"), 12)]);
    // The next check names the files attached already.
    write(&mut app, "more.txt");
    assert_eq!(
        press(&mut app, KeyCode::Enter),
        [Effect::Attach {
            request: 3,
            typed: "more.txt".into(),
            attached: vec![PathBuf::from("notes.txt")],
        }]
    );
}

#[test]
fn a_paste_goes_where_keys_type_and_never_answers_a_question() {
    let mut app = loaded();
    let paste = |text: &str| Event::Paste(text.into());
    // In the list, it does nothing.
    assert!(app.message_for(&paste("q")).is_none());
    press(&mut app, KeyCode::Char(':'));
    let message = app.message_for(&paste("folder\nInbox\u{7}")).unwrap();
    app.update(message);
    assert_eq!(app.command_line.as_ref().unwrap().text, "folder Inbox");
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Char('n'));
    let message = app.message_for(&paste("sam@proton.me")).unwrap();
    app.update(message);
    assert_eq!(app.compose.as_ref().unwrap().to.text(), "sam@proton.me");
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.question, Some(Question::Discard));
    assert!(app.message_for(&paste("y")).is_none());
    assert_eq!(app.question, Some(Question::Discard));
}

#[test]
fn r_shift_r_and_shift_f_answer_the_conversation_the_reader_shows() {
    let mut app = loaded();
    app.sender = "me@example.test".into();
    press(&mut app, KeyCode::Char('R'));
    assert_eq!(
        app.notice.as_deref(),
        Some("Open a conversation to reply to it.")
    );
    press(&mut app, KeyCode::Char('F'));
    assert_eq!(
        app.notice.as_deref(),
        Some("Open a conversation to forward it.")
    );
    let mut open = reading(1);
    open.answer = Some(Box::new(super::super::reading::Answerable {
        message_id: "m1".into(),
        from: None,
        subject: "Plans".into(),
        sender: "Alex Reed".into(),
        to: vec!["alex@example.test".into()],
        cc: vec!["sam@example.test".into()],
        redirected_from: None,
        opened: true,
        files: Vec::new(),
    }));
    app.reading = Some(open);
    // In the list, r marks read; in the reader, it replies.
    app.focus = Pane::Conversations;
    let effects = press(&mut app, KeyCode::Char('r'));
    assert!(
        matches!(
            &effects[..],
            [Effect::Change {
                change: Change::Read(true),
                ..
            }]
        ),
        "{effects:?}"
    );
    assert!(app.compose.is_none());
    app.focus = Pane::Reader;
    press(&mut app, KeyCode::Char('r'));
    let kind = |app: &App| app.compose.as_ref().map(|compose| compose.kind.clone());
    assert!(
        matches!(
            kind(&app),
            Some(OutgoingKind::Reply {
                everyone: false,
                ..
            })
        ),
        "{:?}",
        kind(&app)
    );
    assert_eq!(app.compose.as_ref().unwrap().from, "me@example.test");
    // Untouched, an answer closes at once.
    press(&mut app, KeyCode::Esc);
    assert!(app.compose.is_none() && app.question.is_none());
    press(&mut app, KeyCode::Char('R'));
    assert!(matches!(
        kind(&app),
        Some(OutgoingKind::Reply { everyone: true, .. })
    ));
    press(&mut app, KeyCode::Esc);
    command(&mut app, "forward");
    assert!(matches!(kind(&app), Some(OutgoingKind::Forward { .. })));
    write(&mut app, "sam@proton.me");
    ctrl(&mut app, KeyCode::Char('s'));
    let effects = press(&mut app, KeyCode::Char('y'));
    assert!(
        matches!(&effects[..], [Effect::Send(outgoing)] if outgoing.kind == OutgoingKind::Forward { message_id: "m1".into(), quote: ruston_core::Quote::Original, leave_out: Vec::new() }),
        "{effects:?}"
    );
}

#[test]
fn sent_lists_a_message_once_it_goes() {
    let mut app = loaded();
    press(&mut app, KeyCode::Char('g'));
    let effects = press(&mut app, KeyCode::Char('e'));
    let [Effect::Conversations { request, .. }] = effects[..] else {
        panic!("{effects:?}");
    };
    app.update(Message::ConversationsLoaded {
        request,
        page: 0,
        result: Ok((0, Vec::new())),
    });
    press(&mut app, KeyCode::Char('n'));
    write(&mut app, "sam@proton.me");
    ctrl(&mut app, KeyCode::Char('s'));
    press(&mut app, KeyCode::Char('y'));
    let effects = app.update(Message::Sent(Ok(())));
    assert!(
        matches!(&effects[..], [Effect::Places, Effect::Conversations { place, .. }] if place == label_ids::SENT),
        "{effects:?}"
    );
    assert_eq!(app.notice.as_deref(), Some("Sent."));
}

#[test]
fn the_reader_puts_away_a_conversation_a_change_took_out_of_the_list() {
    let changed = |app: &mut App, change: Change, id: &str| {
        let place = app.listed_id.clone();
        app.update(Message::Changed {
            change,
            ids: vec![id.into()],
            place,
            result: Ok(()),
        });
    };
    let mut app = loaded();
    app.reading = Some(reading(3));
    app.focus = Pane::Reader;
    // Starred, labelled or moved to the Inbox it shows, it stays.
    changed(&mut app, Change::Star(true), "b");
    changed(&mut app, Change::Label("label-1".into()), "b");
    changed(&mut app, Change::Move(label_ids::INBOX.into()), "b");
    // So does it when another conversation leaves.
    changed(&mut app, Change::Move(label_ids::ARCHIVE.into()), "a");
    assert!(app.reading.is_some());
    assert_eq!(app.focus, Pane::Reader);
    // Archived, it goes, and the keys go back to the list.
    changed(&mut app, Change::Move(label_ids::ARCHIVE.into()), "b");
    assert!(app.reading.is_none());
    assert_eq!(app.focus, Pane::Conversations);
    // In All mail, only Trash and Spam take it out.
    let mut app = loaded();
    command(&mut app, "search plans");
    app.reading = Some(reading(3));
    changed(&mut app, Change::Move(label_ids::ARCHIVE.into()), "b");
    assert!(app.reading.is_some());
    changed(&mut app, Change::Move(label_ids::TRASH.into()), "b");
    assert!(app.reading.is_none());
    // Unstarred in Starred, it goes too.
    let mut app = loaded();
    press(&mut app, KeyCode::Char('g'));
    press(&mut app, KeyCode::Char('*'));
    app.reading = Some(reading(3));
    changed(&mut app, Change::Star(false), "b");
    assert!(app.reading.is_none());
}

#[test]
fn a_conversation_that_left_the_list_leaves_the_selection_in_its_place() {
    let mut app = loaded();
    app.focus = Pane::Conversations;
    press(&mut app, KeyCode::Down);
    let refresh = Event::Key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
    app.update(Message::from_event(&refresh).unwrap());
    app.update(Message::ConversationsLoaded {
        request: 2,
        page: 0,
        result: Ok((2, vec![conversation("a"), conversation("c")])),
    });
    assert_eq!(app.conversations[app.selected_conversation].id, "c");
    // Another folder starts at the top of its list.
    app.focus = Pane::Folders;
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Enter);
    app.update(Message::ConversationsLoaded {
        request: 3,
        page: 0,
        result: Ok((2, vec![conversation("w1"), conversation("w2")])),
    });
    assert_eq!(app.selected_conversation, 0);
}

#[test]
fn a_command_line_opens_folders_by_name_and_remembers_what_ran() {
    let mut app = loaded();
    assert_eq!(
        command(&mut app, "folder WORK"),
        [Effect::Conversations {
            request: 2,
            place: "folder-1".into(),
            page: 0
        }]
    );
    assert_eq!((app.focus, app.selected_place), (Pane::Conversations, 1));
    assert!(app.command_line.is_none());
    command(&mut app, "folder Nowhere");
    assert_eq!(
        app.notice.as_deref(),
        Some("No folder or label is named Nowhere")
    );
    command(&mut app, "fly");
    assert_eq!(app.notice.as_deref(), Some("Not a command: :fly"));

    // Up recalls the lines run before, and Tab completes.
    press(&mut app, KeyCode::Char(':'));
    press(&mut app, KeyCode::Up);
    assert_eq!(app.command_line.as_ref().unwrap().text, "fly");
    press(&mut app, KeyCode::Up);
    press(&mut app, KeyCode::Up);
    assert_eq!(app.command_line.as_ref().unwrap().text, "folder WORK");
    press(&mut app, KeyCode::Char('u'));
    let ctrl_u = Event::Key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    app.update(app.message_for(&ctrl_u).unwrap());
    for c in "folder in".chars() {
        press(&mut app, KeyCode::Char(c));
    }
    press(&mut app, KeyCode::Tab);
    assert_eq!(app.command_line.as_ref().unwrap().text, "folder Inbox");
    // While it is open, keys type: q does not quit, j does not move.
    press(&mut app, KeyCode::Char('q'));
    assert!(!app.quit);
    assert_eq!(app.command_line.as_ref().unwrap().text, "folder Inboxq");
    press(&mut app, KeyCode::Esc);
    assert!(app.command_line.is_none());
    // Backspace on an empty line closes it, as Ctrl+C does.
    press(&mut app, KeyCode::Char(':'));
    press(&mut app, KeyCode::Backspace);
    assert!(app.command_line.is_none());
    press(&mut app, KeyCode::Char(':'));
    let ctrl_c = Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    app.update(app.message_for(&ctrl_c).unwrap());
    assert!(app.command_line.is_none());
    assert!(!app.quit);
    command(&mut app, "q");
    assert!(app.quit);
}

#[test]
fn altgr_types_on_the_command_line() {
    let mut app = loaded();
    press(&mut app, KeyCode::Char(':'));
    let at = KeyEvent::new(
        KeyCode::Char('@'),
        KeyModifiers::CONTROL | KeyModifiers::ALT,
    );
    app.update(app.message_for(&Event::Key(at)).unwrap());
    assert_eq!(app.command_line.as_ref().unwrap().text, "@");
}

#[test]
fn commands_make_the_quick_actions_and_move_to_any_folder() {
    let mut app = loaded();
    app.places = vec![
        place(label_ids::INBOX, "Inbox"),
        place(label_ids::SENT, "Sent"),
        Place {
            kind: PlaceKind::Folder,
            ..place("folder-1", "Work")
        },
        Place {
            kind: PlaceKind::Label,
            ..place("label-1", "Family")
        },
    ];
    app.focus = Pane::Conversations;
    let change = |change: Change| {
        vec![Effect::Change {
            ids: vec!["a".into()],
            place: label_ids::INBOX.into(),
            change,
        }]
    };
    assert_eq!(command(&mut app, "star"), change(Change::Star(true)));
    assert_eq!(command(&mut app, "unstar"), change(Change::Star(false)));
    assert_eq!(command(&mut app, "read"), change(Change::Read(true)));
    assert_eq!(command(&mut app, "unread"), change(Change::Read(false)));
    let archive = Change::Move(label_ids::ARCHIVE.into());
    assert_eq!(command(&mut app, "archive"), change(archive));
    let trash = Change::Move(label_ids::TRASH.into());
    assert_eq!(command(&mut app, "trash"), change(trash));
    let work = Change::Move("folder-1".into());
    assert_eq!(command(&mut app, "move work"), change(work));
    assert!(command(&mut app, "move Family").is_empty());
    assert_eq!(
        app.notice.as_deref(),
        Some("Conversations cannot be moved to Family")
    );
    assert!(command(&mut app, "move Sent").is_empty());
    // Tab offers only the folders a conversation can move to.
    press(&mut app, KeyCode::Char(':'));
    for c in "move ".chars() {
        press(&mut app, KeyCode::Char(c));
    }
    let mut offered = Vec::new();
    for _ in 0..3 {
        press(&mut app, KeyCode::Tab);
        offered.push(app.command_line.as_ref().unwrap().text.clone());
    }
    assert_eq!(offered, ["move Inbox", "move Work", "move Inbox"]);
    press(&mut app, KeyCode::Esc);
    command(&mut app, "help");
    assert!(app.help);
}

#[test]
fn a_task_that_stopped_on_a_bug_says_so() {
    let mut app = loaded();
    press(&mut app, KeyCode::Char('?'));
    // An answer, even with the keys open.
    app.update(Message::Failed("the renderer broke".into()));
    assert_eq!(
        app.notice.as_deref(),
        Some("Something went wrong: the renderer broke")
    );
}

#[test]
fn a_search_lists_what_it_finds_and_esc_goes_back_to_the_place() {
    let mut app = loaded();
    press(&mut app, KeyCode::Char('/'));
    assert_eq!(app.command_line.as_ref().unwrap().text, "search ");
    for c in "plans".chars() {
        press(&mut app, KeyCode::Char(c));
    }
    let effects = press(&mut app, KeyCode::Enter);
    assert_eq!(
        effects,
        [Effect::Search {
            request: 2,
            opts: looking_for("plans"),
            page: 0
        }]
    );
    assert_eq!(app.focus, Pane::Conversations);
    assert_eq!(app.listed, "Search: plans");
    assert_eq!(app.notice.as_deref(), Some("Searching for plans…"));
    app.update(Message::ConversationsLoaded {
        request: 2,
        page: 0,
        result: Ok((3, vec![conversation("p1"), conversation("p2")])),
    });
    // More of it loads as a list's does, and Ctrl+R searches again.
    press(&mut app, KeyCode::End);
    assert_eq!(
        press(&mut app, KeyCode::Down),
        [Effect::Search {
            request: 2,
            opts: looking_for("plans"),
            page: 1
        }]
    );
    let refresh = Event::Key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
    let effects = app.update(Message::from_event(&refresh).unwrap());
    assert_eq!(
        effects,
        [
            Effect::Places,
            Effect::Search {
                request: 3,
                opts: looking_for("plans"),
                page: 0
            }
        ]
    );
    // A second search goes back to where the first came from.
    command(&mut app, "search notes");
    assert_eq!(app.listed, "Search: notes");
    assert_eq!(
        press(&mut app, KeyCode::Esc),
        [Effect::Conversations {
            request: 5,
            place: label_ids::INBOX.into(),
            page: 0
        }]
    );
    assert_eq!((app.search.clone(), app.focus), (None, Pane::Conversations));
    assert_eq!(app.listed, "Inbox");
}

/// What Proton's search is asked for `words` alone.
fn looking_for(words: &str) -> SearchOpts {
    SearchOpts {
        keyword: Some(words.into()),
        ..SearchOpts::default()
    }
}

#[test]
fn a_search_narrows_by_the_filters_beside_its_words_its_folder_named_by_id() {
    let mut app = loaded();
    let words = "plans from:alex in:WORK after:2026-01-01 before:2026-01-31";
    assert_eq!(
        command(&mut app, &format!("search {words}")),
        [Effect::Search {
            request: 2,
            opts: SearchOpts {
                from: Some("alex".into()),
                after: Some("2026-01-01".into()),
                before: Some("2026-01-31".into()),
                folder: Some("folder-1".into()),
                ..looking_for("plans")
            },
            page: 0
        }]
    );
    assert_eq!(app.listed, format!("Search: {words}"));
}

#[test]
fn in_names_a_folder_by_its_path_or_id_or_all_mail_as_messages_search_does() {
    let mut app = loaded();
    let folder = |id: &str, path: &str| Place {
        kind: PlaceKind::Folder,
        path: path.into(),
        ..place(id, path.rsplit('/').next().unwrap())
    };
    app.places.extend([
        folder("taxes", "Taxes"),
        folder("taxes-2024", "Taxes/2024"),
        folder("travel-2024", "Travel/2024"),
    ]);
    let searched_in = |app: &mut App, folder: &str| {
        let effects = command(app, &format!("search plans in:{folder}"));
        match &effects[..] {
            [Effect::Search { opts, .. }] => opts.folder.clone().unwrap(),
            effects => panic!("in:{folder} {effects:?}"),
        }
    };
    let id = "qBIcv1_Wv5X4hLpEo0Tz9A==";
    for (folder, searched) in [
        ("taxes/2024", "taxes-2024"),
        ("TRAVEL/2024", "travel-2024"),
        ("Taxes", "taxes"),
        ("work", "folder-1"),
        ("inbox", label_ids::INBOX),
        ("all", label_ids::ALL_MAIL),
        (id, id),
    ] {
        assert_eq!(searched_in(&mut app, folder), searched);
    }
    // A name two folders share is refused, naming their paths.
    press(&mut app, KeyCode::Esc);
    assert!(command(&mut app, "search plans in:2024").is_empty());
    assert_eq!(
        app.notice.as_deref(),
        Some("Several folders or labels are named 2024: Taxes/2024, Travel/2024")
    );
    assert_eq!((app.search.clone(), app.listed.as_str()), (None, "Inbox"));
    // Tab offers the places' names after it.
    press(&mut app, KeyCode::Char('/'));
    for c in "plans in:ta".chars() {
        press(&mut app, KeyCode::Char(c));
    }
    press(&mut app, KeyCode::Tab);
    let line = app.command_line.as_ref().unwrap();
    assert_eq!(line.text, "search plans in:Taxes");
}

#[test]
fn a_search_whose_filters_cannot_be_met_is_refused_before_it_starts() {
    let mut app = loaded();
    for (line, said) in [
        (
            "search plans after:2024-05-01 before:2024-04-30",
            "Cannot search: invalid date range: 2024-05-01 is after 2024-04-30",
        ),
        (
            "search plans before:30/04/2024",
            "Cannot search: invalid date '30/04/2024': expected YYYY-MM-DD",
        ),
        (
            "search plans in:Nowhere",
            "No folder or label is named Nowhere",
        ),
        ("search plans from:", ":search needs something after from:"),
        ("search to:sam to:alex", ":search takes to: once"),
    ] {
        assert!(command(&mut app, line).is_empty(), "{line}");
        assert_eq!(app.notice.as_deref(), Some(said), "{line}");
        // The list stays the place it was.
        assert_eq!((app.search.clone(), app.listed.as_str()), (None, "Inbox"));
    }
}

#[test]
fn a_local_search_lists_all_it_finds_at_once_and_searches_the_index_again() {
    let mut app = loaded();
    assert_eq!(
        command(&mut app, "local budget"),
        [Effect::LocalSearch {
            request: 2,
            opts: looking_for("budget")
        }]
    );
    assert_eq!(app.listed, "Local: budget");
    assert_eq!(
        app.notice.as_deref(),
        Some("Searching the local index for budget…")
    );
    app.update(Message::ConversationsLoaded {
        request: 2,
        page: 0,
        result: Ok((2, vec![conversation("b1"), conversation("b2")])),
    });
    // All of it came: Down at the end asks for nothing more.
    press(&mut app, KeyCode::End);
    assert!(press(&mut app, KeyCode::Down).is_empty());
    let refresh = Event::Key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
    assert_eq!(
        app.update(Message::from_event(&refresh).unwrap()),
        [
            Effect::Places,
            Effect::LocalSearch {
                request: 3,
                opts: looking_for("budget")
            }
        ]
    );
    press(&mut app, KeyCode::Esc);
    assert_eq!((app.search.clone(), app.listed.as_str()), (None, "Inbox"));
}

#[test]
fn a_local_search_takes_the_filters_of_a_search_beside_words() {
    let mut app = loaded();
    for (line, said) in [
        (
            "local from:alex in:work",
            "The local index cannot search by filters alone: type words too",
        ),
        (
            "local budget in:Nowhere",
            "No folder or label is named Nowhere",
        ),
        ("local budget to:", ":local needs something after to:"),
    ] {
        assert!(command(&mut app, line).is_empty(), "{line}");
        assert_eq!(app.notice.as_deref(), Some(said), "{line}");
        assert_eq!((app.search.clone(), app.listed.as_str()), (None, "Inbox"));
    }
    let words = "budget from:alex to:sam in:WORK after:2026-01-01 before:2026-01-31";
    assert_eq!(
        command(&mut app, &format!("local {words}")),
        [Effect::LocalSearch {
            request: 2,
            opts: SearchOpts {
                from: Some("alex".into()),
                to: Some("sam".into()),
                after: Some("2026-01-01".into()),
                before: Some("2026-01-31".into()),
                folder: Some("folder-1".into()),
                ..looking_for("budget")
            }
        }]
    );
    assert_eq!(app.listed, format!("Local: {words}"));
}

#[test]
fn a_change_to_a_search_result_searches_again() {
    let mut app = loaded();
    command(&mut app, "search plans");
    app.update(Message::ConversationsLoaded {
        request: 2,
        page: 0,
        result: Ok((1, vec![conversation("p1")])),
    });
    assert_eq!(
        press(&mut app, KeyCode::Char('*')),
        [Effect::Change {
            ids: vec!["p1".into()],
            place: label_ids::ALL_MAIL.into(),
            change: Change::Star(true)
        }]
    );
    let effects = app.update(Message::Changed {
        change: Change::Star(true),
        ids: vec!["p1".into()],
        place: label_ids::ALL_MAIL.into(),
        result: Ok(()),
    });
    assert_eq!(
        effects,
        [
            Effect::Places,
            Effect::Search {
                request: 3,
                opts: looking_for("plans"),
                page: 0
            }
        ]
    );
    // Opening a folder ends the search.
    app.focus = Pane::Folders;
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.search, None);
}

#[test]
fn esc_closes_the_keys_first_then_goes_back_a_pane() {
    let mut app = loaded();
    app.focus = Pane::Reader;
    press(&mut app, KeyCode::Char('?'));
    press(&mut app, KeyCode::Esc);
    assert_eq!((app.help, app.focus), (false, Pane::Reader));
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.focus, Pane::Conversations);
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.focus, Pane::Folders);
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.focus, Pane::Folders);
}

#[test]
fn an_empty_folder_leaves_a_way_back_to_the_folders() {
    let mut app = loaded();
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Enter);
    // Its first page is on its way: the Inbox's rows still show.
    assert!(!app.listed_nothing());
    app.update(Message::ConversationsLoaded {
        request: 2,
        page: 0,
        result: Ok((0, Vec::new())),
    });
    assert!(app.listed_nothing());
    assert_eq!(app.focus, Pane::Conversations);
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Up);
    assert_eq!((app.focus, app.selected_place), (Pane::Folders, 0));
}

#[test]
fn a_resize_asks_for_nothing() {
    assert!(Message::from_event(&Event::Resize(80, 24)).is_none());
}
