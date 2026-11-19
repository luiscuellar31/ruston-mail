use super::super::app::{Effect, Message};
use super::super::compose::AnswerKind;
use super::super::reading::Reading;
use super::*;
use crate::mailbox::{Outgoing, Place, PlaceKind};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ruston_core::model::ConversationLabel;
use ruston_core::{ConversationMessage, FullMessage, MessageMetadata, Recipient, Verdict};

fn drawn(width: u16, height: u16, app: &App, theme: &Theme) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| draw(frame, app, theme)).unwrap();
    terminal.backend().buffer().clone()
}

fn text(buffer: &Buffer) -> String {
    let area = buffer.area;
    (0..area.height)
        .map(|y| {
            (0..area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// An app showing two folders and one conversation from `sender` about
/// `subject`.
fn with_mail(sender: &str, subject: &str) -> App {
    let mut app = App::new("me@example.test".into());
    app.places = vec![
        Place {
            id: "0".into(),
            name: "Inbox".into(),
            path: "Inbox".into(),
            depth: 0,
            kind: PlaceKind::System,
            unread: 3,
        },
        Place {
            id: "f-2024".into(),
            name: "2024".into(),
            path: "Taxes/2024".into(),
            depth: 1,
            kind: PlaceKind::Folder,
            unread: 0,
        },
    ];
    app.conversations = vec![Conversation {
        subject: subject.into(),
        time: 1_714_000_000,
        num_unread: 1,
        senders: vec![Recipient::new(sender)],
        ..Conversation::default()
    }];
    app.total = 1;
    app.notice = None;
    app
}

#[test]
fn a_wide_terminal_shows_the_three_panes_and_the_status_bar() {
    let app = with_mail("alex@example.test", "Quarterly report");
    let screen = text(&drawn(120, 30, &app, &Theme::new(false)));
    for title in ["Folders", "Conversations", "Reader"] {
        assert!(screen.contains(title), "{title} missing:\n{screen}");
    }
    assert!(screen.contains("Inbox 3"), "{screen}");
    assert!(screen.contains("  2024"), "{screen}");
    // The pane is 48 columns wide: the day leaves out its year and the
    // sender is cut short, so the subject fits.
    assert!(
        screen.contains(" •   04-24  alex@examp…  Quarterly report     │"),
        "{screen}"
    );
    let status = screen.lines().last().unwrap();
    assert!(
        status.contains("me@example.test · Inbox · 1 conversation(s)"),
        "{status}"
    );
    assert!(status.contains("q quit"), "{status}");
}

#[test]
fn a_marked_row_says_so_and_the_status_bar_counts_them() {
    let mut app = with_mail("alex@example.test", "Quarterly report");
    app.marked = vec![app.conversations[0].id.clone()];
    let screen = text(&drawn(120, 30, &app, &Theme::new(false)));
    assert!(screen.contains("✓•   04-24  alex@examp…"), "{screen}");
    let status = screen.lines().last().unwrap();
    assert!(status.contains("Inbox · 1 marked · "), "{status}");
}

#[test]
fn a_row_marks_a_starred_conversation_and_one_with_attachments() {
    let mut app = with_mail("alex@example.test", "Quarterly report");
    let conversation = &mut app.conversations[0];
    conversation.num_unread = 0;
    conversation.num_attachments = 2;
    conversation.labels = vec![ruston_core::model::ConversationLabel {
        id: label_ids::STARRED.into(),
        ..Default::default()
    }];
    let screen = text(&drawn(120, 30, &app, &Theme::new(false)));
    assert!(screen.contains(" ★+ 04-24  alex@examp…"), "{screen}");
}

/// One of the account's labels.
fn label(id: &str, name: &str) -> Place {
    Place {
        id: id.into(),
        name: name.into(),
        path: name.into(),
        depth: 0,
        kind: PlaceKind::Label,
        unread: 0,
    }
}

/// Bears the folders and labels `ids`.
fn bearing(ids: &[&str]) -> Vec<ConversationLabel> {
    ids.iter()
        .map(|id| ConversationLabel {
            id: (*id).into(),
            ..ConversationLabel::default()
        })
        .collect()
}

#[test]
fn a_row_names_its_labels_after_its_subject_which_gives_way_first() {
    let mut app = with_mail("alex@example.test", "Plans");
    app.places
        .extend([label("l-1", "Family"), label("l-2", "Travel")]);
    app.conversations[0].labels = bearing(&["0", "l-2", "l-1"]);
    app.focus = Pane::Conversations;
    // The list alone: 78 columns inside its border, 41 of them after
    // the sender. The labels come in the folder list's order, and the
    // Inbox is no label of the account's.
    let screen = text(&drawn(80, 24, &app, &Theme::new(false)));
    assert!(
        screen.contains(" •   2024-04-24  alex@example.test   Plans [Family] [Travel] "),
        "{screen}"
    );
    // A long subject gives way to a third of that room, and a label that
    // does not fit there is counted.
    app.conversations[0].subject = "Quarterly report for the board meeting in May".into();
    let screen = text(&drawn(80, 24, &app, &Theme::new(false)));
    assert!(
        screen.contains(
            " •   2024-04-24  alex@example.test   Quarterly report for the boa… [Family] +1│"
        ),
        "{screen}"
    );
    // Where the subject has only a few columns, they stay its own.
    let screen = text(&drawn(100, 30, &app, &Theme::new(false)));
    assert!(
        screen.contains(" •   04-24  alex@exa…  Quarterly repor│"),
        "{screen}"
    );
}

#[test]
fn the_subject_keeps_a_share_of_a_row_beside_the_other_panes() {
    let mut app = with_mail("alex@example.test", "Quarterly report for the board");
    app.places.push(label("l-1", "Work"));
    app.conversations[0].labels = bearing(&["l-1"]);
    app.focus = Pane::Conversations;
    let whole = " •   2024-04-24  alex@example.test   Quarterly report for the board [Work]    │";
    for (width, row) in [
        // Beside the other panes, the day leaves out its year and the
        // sender takes a quarter of the row, cut short, so the subject
        // keeps the largest share, and from 115 columns a label fits too.
        (100, " •   04-24  alex@exa…  Quarterly repor│"),
        (115, " •   04-24  alex@examp…  Quarterly r… [Work]│"),
        (130, " •   04-24  alex@exampl…  Quarterly report… [Work]│"),
        // The list alone in a narrow terminal, and beside the other
        // panes in a wide one, keeps the whole row.
        (80, whole),
        (200, whole),
    ] {
        let screen = text(&drawn(width, 24, &app, &Theme::new(false)));
        assert!(screen.contains(row), "{width} columns:\n{screen}");
    }
}

#[test]
fn labels_take_the_accent_and_without_colors_read_by_their_brackets() {
    let mut app = with_mail("alex@example.test", "Plans");
    app.places.push(label("l-1", "Family"));
    app.conversations[0].labels = bearing(&["l-1"]);
    app.focus = Pane::Conversations;
    let label_cell = |theme: &Theme| {
        let buffer = drawn(80, 24, &app, theme);
        let screen = text(&buffer);
        let (row, line) = screen
            .lines()
            .enumerate()
            .find(|(_, line)| line.contains("[Family]"))
            .unwrap_or_else(|| panic!("no label:\n{screen}"));
        let column = line[..line.find("[Family]").unwrap()].chars().count();
        buffer[(u16::try_from(column).unwrap(), u16::try_from(row).unwrap())].clone()
    };
    assert_eq!(label_cell(&Theme::new(false)).fg, ACCENT);
    // Bold would say the conversation is unread.
    let plain = label_cell(&Theme::new(true));
    assert_eq!(plain.fg, Color::Reset);
    assert!(!plain.modifier.contains(Modifier::BOLD));
}

#[test]
fn mail_text_cannot_draw_controls_or_spill_into_other_rows() {
    let app = with_mail(
        "evil\x1b[2J\u{202e}@example.test",
        "first\nsecond\x1b]52;c;payload\x07\tend",
    );
    // Wide enough for the whole subject to show.
    let buffer = drawn(200, 30, &app, &Theme::new(false));
    assert!(
        buffer.content().iter().all(|cell| cell
            .symbol()
            .chars()
            .all(|c| !c.is_control() && c != '\u{202e}')),
        "{}",
        text(&buffer)
    );
    let screen = text(&buffer);
    assert!(screen.contains("first second"), "{screen}");
}

#[test]
fn the_reader_shows_the_open_conversation_and_scrolls_within_it() {
    let mut app = with_mail("alex@example.test", "Quarterly report");
    let screen = text(&drawn(120, 30, &app, &Theme::new(false)));
    assert!(
        screen.contains("Enter on a conversation opens it here."),
        "{screen}"
    );

    app.reading = Some(Reading {
        id: "plans".into(),
        subject: "Plans for May".into(),
        lines: (0..100).map(|n| format!("line {n}")).collect(),
        quotes: Vec::new(),
        links: Vec::new(),
        attachments: Vec::new(),
        messages: Vec::new(),
        answer: None,
    });
    let screen = text(&drawn(120, 30, &app, &Theme::new(false)));
    assert!(screen.contains("Plans for May"), "{screen}");
    assert!(screen.contains("line 0"), "{screen}");
    // The subject, a blank row and 100 lines, 27 rows of them in view.
    assert_eq!(app.reader_bottom.get(), 102 - 27);
    assert_eq!(app.reader_rows.get(), 27);

    app.scroll.set(50);
    let screen = text(&drawn(120, 30, &app, &Theme::new(false)));
    assert!(!screen.contains("line 0 "), "{screen}");
    assert!(screen.contains("line 48"), "{screen}");
    // Scrolled past the end, as after the pane grew, it shows the end.
    app.scroll.set(500);
    let screen = text(&drawn(120, 30, &app, &Theme::new(false)));
    assert!(screen.contains("line 99"), "{screen}");
}

#[test]
fn the_reader_names_the_labels_the_conversation_bears() {
    let mut app = with_mail("alex@example.test", "Quarterly report");
    let label = |id: &str, name: &str| Place {
        id: id.into(),
        name: name.into(),
        path: name.into(),
        depth: 0,
        kind: PlaceKind::Label,
        unread: 0,
    };
    app.places
        .extend([label("l-1", "Family"), label("l-2", "Travel")]);
    app.conversations[0].id = "plans".into();
    app.reading = Some(Reading {
        id: "plans".into(),
        subject: "Plans".into(),
        lines: vec!["Hi".into()],
        quotes: Vec::new(),
        links: Vec::new(),
        attachments: Vec::new(),
        messages: Vec::new(),
        answer: None,
    });
    let screen = text(&drawn(120, 30, &app, &Theme::new(false)));
    assert!(!screen.contains("Labels:"), "{screen}");

    app.conversations[0].labels = ["0", "l-2"]
        .map(|id| ConversationLabel {
            id: id.into(),
            ..ConversationLabel::default()
        })
        .to_vec();
    let screen = text(&drawn(120, 30, &app, &Theme::new(false)));
    assert!(screen.contains("Labels: Travel "), "{screen}");
}

#[test]
fn the_reader_folds_quoted_passages_until_they_are_shown() {
    let mut app = with_mail("alex@example.test", "Quarterly report");
    let lines = [
        "Thanks!", "", "> a", "", "> b", "> c", "", "Then:", "> d", "> e", "> f", "", "Bye",
    ];
    app.reading = Some(Reading {
        id: "plans".into(),
        subject: "Plans".into(),
        lines: lines.map(str::to_owned).to_vec(),
        quotes: vec![2..6, 8..11],
        links: Vec::new(),
        attachments: Vec::new(),
        messages: Vec::new(),
        answer: None,
    });
    let screen = text(&drawn(120, 30, &app, &Theme::new(false)));
    for shown in [
        "… 4 quoted lines (z shows them)",
        "Then:",
        "… 3 quoted lines (z shows them)",
        "Bye",
    ] {
        assert!(screen.contains(shown), "{shown:?} missing:\n{screen}");
    }
    assert!(
        !screen.contains("> b") && !screen.contains("> e"),
        "{screen}"
    );

    app.show_quotes = true;
    let screen = text(&drawn(120, 30, &app, &Theme::new(false)));
    assert!(screen.contains("> b") && screen.contains("> e"), "{screen}");
    assert!(!screen.contains("quoted lines"), "{screen}");
}

#[test]
fn z_keeps_the_line_at_the_readers_top_there() {
    let mut app = with_mail("alex@example.test", "Quarterly report");
    let mut lines: Vec<String> = (0..10).map(|n| format!("> quoted {n}")).collect();
    lines.extend((0..40).map(|n| format!("reply line {n}")));
    app.reading = Some(Reading {
        id: "plans".into(),
        subject: "Plans".into(),
        lines,
        quotes: std::iter::once(0..10).collect(),
        links: Vec::new(),
        attachments: Vec::new(),
        messages: Vec::new(),
        answer: None,
    });
    // Under the subject and a blank row, the folded passage takes a row.
    app.scroll.set(8);
    let screen = text(&drawn(120, 30, &app, &Theme::new(false)));
    assert!(top_row(&screen).contains("reply line 5"), "{screen}");

    // Shown, the passage takes nine rows more, and the reader follows.
    press(&mut app, KeyCode::Char('z'));
    let screen = text(&drawn(120, 30, &app, &Theme::new(false)));
    assert!(top_row(&screen).contains("reply line 5"), "{screen}");
    assert_eq!(app.scroll.get(), 17);
    press(&mut app, KeyCode::Char('z'));
    let screen = text(&drawn(120, 30, &app, &Theme::new(false)));
    assert!(top_row(&screen).contains("reply line 5"), "{screen}");
    assert_eq!(app.scroll.get(), 8);

    // From inside the passage, its fold comes to the top as it folds.
    press(&mut app, KeyCode::Char('z'));
    let _ = drawn(120, 30, &app, &Theme::new(false));
    app.scroll.set(5);
    let screen = text(&drawn(120, 30, &app, &Theme::new(false)));
    assert!(top_row(&screen).contains("> quoted 3"), "{screen}");
    press(&mut app, KeyCode::Char('z'));
    let screen = text(&drawn(120, 30, &app, &Theme::new(false)));
    assert!(
        top_row(&screen).contains("… 10 quoted lines (z shows them)"),
        "{screen}"
    );
}

/// A message from `sender`, `hours` after the first, saying `body`; not
/// read yet when `unread` is 1.
fn message(sender: &str, hours: i64, unread: i64, body: &str) -> ConversationMessage {
    ConversationMessage::Opened(FullMessage {
        meta: MessageMetadata {
            sender: Recipient::new(sender),
            time: 1_714_000_000 + hours * 3_600,
            unread,
            ..MessageMetadata::default()
        },
        body: body.into(),
        mime_type: "text/plain".into(),
        verdict: Verdict::Verified,
        attachments: Vec::new(),
        reply_tos: Vec::new(),
    })
}

#[test]
fn the_reader_folds_the_messages_read_but_the_newest_until_e_shows_them() {
    let messages = [
        message("alex@example.test", 0, 0, "First thoughts\n> on the plan"),
        message("sam@example.test", 1, 0, "Second thoughts"),
        message("alex@example.test", 2, 1, "A reply not read yet"),
        message("sam@example.test", 3, 0, "The newest words"),
    ];
    let mut app = with_mail("alex@example.test", "Quarterly report");
    let conversation = Conversation {
        subject: "Plans".into(),
        ..Conversation::default()
    };
    app.reading = Some(Reading::new(&conversation, &messages, &[]));
    let screen = text(&drawn(200, 40, &app, &Theme::new(false)));
    // One line each, with when, from whom and how sure, then its words.
    for folded in [
        "▸ 2024-04-24 23:06 · alex@example.test · verified · First thoughts > on the plan",
        "▸ 2024-04-25 00:06 · sam@example.test · verified · Second thoughts",
        "A reply not read yet",
        "The newest words",
        "e opens the messages read",
    ] {
        assert!(screen.contains(folded), "{folded:?} missing:\n{screen}");
    }
    assert!(!screen.contains("Date:    2024-04-24 23:06"), "{screen}");
    // Folded in a row, they follow one another without a separator.
    let row = |text: &str| screen.lines().position(|line| line.contains(text));
    assert_eq!(
        row("· Second thoughts"),
        row("· First thoughts").map(|r| r + 1)
    );

    press(&mut app, KeyCode::Char('e'));
    let screen = text(&drawn(200, 40, &app, &Theme::new(false)));
    assert!(screen.contains("Date:    2024-04-24 23:06"), "{screen}");
    assert!(screen.contains("Second thoughts"), "{screen}");
    assert!(!screen.contains('▸'), "{screen}");
}

fn press(app: &mut App, code: KeyCode) {
    let key = crossterm::event::Event::Key(KeyEvent::new(code, KeyModifiers::NONE));
    if let Some(message) = app.message_for(&key) {
        app.update(message);
    }
}

/// An app that opened, as the list opens one, the conversation "Plans"
/// of `messages`.
fn opened(messages: &[ConversationMessage]) -> App {
    let mut app = with_mail("alex@example.test", "Quarterly report");
    app.focus = Pane::Conversations;
    app.update(Message::Open);
    let conversation = Conversation {
        subject: "Plans".into(),
        ..Conversation::default()
    };
    app.update(Message::ConversationLoaded {
        request: 1,
        result: Ok(Reading::new(&conversation, messages, &[])),
    });
    app
}

/// Ten messages read, a row each folded, then `then`.
fn after_ten_read(then: Vec<ConversationMessage>) -> Vec<ConversationMessage> {
    let read = (0..10).map(|n| message("alex@example.test", n, 0, &format!("read {n}")));
    read.chain(then).collect()
}

/// Forty lines that say `what`.
fn long(what: &str) -> String {
    (0..40).map(|n| format!("{what} line {n}\n")).collect()
}

/// The first row inside the reader's borders, beside the other panes'.
fn top_row(screen: &str) -> &str {
    screen.lines().nth(1).unwrap_or_default()
}

#[test]
fn a_conversation_opens_at_its_first_unread_message_unless_it_shows_already() {
    let messages = after_ten_read(vec![
        message("sam@example.test", 10, 1, &long("unread")),
        message("alex@example.test", 11, 0, &long("newest")),
    ]);
    let app = opened(&messages);
    let screen = text(&drawn(120, 30, &app, &Theme::new(false)));
    // Past the subject, a blank row, the ten folded and a separator.
    assert_eq!(app.scroll.get(), 15);
    assert!(
        top_row(&screen).contains("From:    sam@example.test"),
        "{screen}"
    );
    assert_eq!(app.in_view.get(), Some(10));

    // Read, it opens at its newest message.
    let messages = after_ten_read(vec![message("sam@example.test", 10, 0, &long("newest"))]);
    let app = opened(&messages);
    let screen = text(&drawn(120, 30, &app, &Theme::new(false)));
    assert!(
        top_row(&screen).contains("From:    sam@example.test"),
        "{screen}"
    );

    // In the upper third already, it leaves the subject in view.
    let messages = [
        message("alex@example.test", 0, 0, "Read"),
        message("sam@example.test", 1, 1, &long("unread")),
    ];
    let app = opened(&messages);
    let screen = text(&drawn(120, 30, &app, &Theme::new(false)));
    assert_eq!(app.scroll.get(), 0);
    assert!(top_row(&screen).contains("Plans"), "{screen}");
}

#[test]
fn enter_folds_or_opens_the_message_at_the_readers_top_and_e_keeps_it_there() {
    let messages = after_ten_read(vec![
        message("sam@example.test", 10, 1, &long("unread")),
        message("alex@example.test", 11, 0, &long("newest")),
    ]);
    let mut app = opened(&messages);
    let _ = drawn(120, 30, &app, &Theme::new(false));
    // Folded, the unread message joins the run above it and stays on top.
    press(&mut app, KeyCode::Enter);
    let screen = text(&drawn(120, 30, &app, &Theme::new(false)));
    assert_eq!(app.scroll.get(), 12);
    let unread_folded = "▸ 2024-04-25 09:06 · sam@example.test";
    assert!(top_row(&screen).contains(unread_folded), "{screen}");
    press(&mut app, KeyCode::Up);
    let screen = text(&drawn(120, 30, &app, &Theme::new(false)));
    let rows: Vec<&str> = screen.lines().skip(1).take(2).collect();
    assert!(
        rows[0].contains("▸ 2024-04-25 08:06 · alex@example.test"),
        "{screen}"
    );
    assert!(rows[1].contains(unread_folded), "{screen}");

    // The read message at the top opens where it is.
    press(&mut app, KeyCode::Enter);
    let screen = text(&drawn(120, 30, &app, &Theme::new(false)));
    assert_eq!(app.scroll.get(), 11);
    assert!(
        screen.contains("Date:    2024-04-25 08:06:40 UTC"),
        "{screen}"
    );
    assert!(screen.contains(unread_folded), "{screen}");

    // e shows every message read, all as it says, the message at the
    // top brought back there.
    press(&mut app, KeyCode::Char('e'));
    let screen = text(&drawn(120, 30, &app, &Theme::new(false)));
    assert!(
        top_row(&screen).contains("From:    alex@example.test"),
        "{screen}"
    );
    let date = screen.lines().nth(3).unwrap_or_default();
    assert!(
        date.contains("Date:    2024-04-25 08:06:40 UTC"),
        "{screen}"
    );
    assert!(screen.contains("unread line 0"), "{screen}");
    assert!(!screen.contains('▸'), "{screen}");
}

/// Does what the mouse's `kind` at `column` and `row` asks, as the loop
/// would.
fn mouse(app: &mut App, kind: MouseEventKind, column: u16, row: u16) -> Vec<Effect> {
    let event = crossterm::event::Event::Mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    });
    app.message_for(&event)
        .map_or_else(Vec::new, |message| app.update(message))
}

const CLICK: MouseEventKind = MouseEventKind::Down(MouseButton::Left);

#[test]
fn a_click_selects_a_row_and_on_the_row_selected_opens_it() {
    let mut app = with_mail("alex@example.test", "Quarterly report");
    app.conversations.push(Conversation {
        id: "plans".into(),
        subject: "Plans".into(),
        ..Conversation::default()
    });
    // At 120 columns, the folders' rows start at (1, 1), the
    // conversations' at (25, 1) and the reader's at (73, 1).
    let _ = drawn(120, 30, &app, &Theme::new(false));
    assert!(mouse(&mut app, CLICK, 5, 2).is_empty());
    assert_eq!((app.focus, app.selected_place), (Pane::Folders, 1));
    let listed = mouse(&mut app, CLICK, 5, 2);
    assert!(
        matches!(&listed[..], [Effect::Conversations { place, .. }] if place == "f-2024"),
        "{listed:?}"
    );
    assert_eq!(app.focus, Pane::Conversations);

    // In a pane without the keys, the row selected already only gets
    // them; a row past the list's last, or the reader, too.
    app.focus = Pane::Reader;
    let _ = drawn(120, 30, &app, &Theme::new(false));
    assert!(mouse(&mut app, CLICK, 30, 1).is_empty());
    assert_eq!(
        (app.focus, app.selected_conversation),
        (Pane::Conversations, 0)
    );
    assert!(mouse(&mut app, CLICK, 30, 9).is_empty());
    assert_eq!(app.selected_conversation, 0);
    assert!(mouse(&mut app, CLICK, 90, 5).is_empty());
    assert_eq!(app.focus, Pane::Reader);
    let _ = drawn(120, 30, &app, &Theme::new(false));
    assert!(mouse(&mut app, CLICK, 30, 2).is_empty());
    assert_eq!(app.selected_conversation, 1);
    let opened = mouse(&mut app, CLICK, 30, 2);
    assert!(
        matches!(&opened[..], [Effect::Conversation { id, .. }] if id == "plans"),
        "{opened:?}"
    );
    assert_eq!(app.focus, Pane::Reader);

    // Nothing but a press of the left button clicks.
    let _ = drawn(120, 30, &app, &Theme::new(false));
    for kind in [
        MouseEventKind::Up(MouseButton::Left),
        MouseEventKind::Down(MouseButton::Right),
        MouseEventKind::Moved,
    ] {
        assert!(mouse(&mut app, kind, 5, 1).is_empty());
        assert_eq!((app.focus, app.selected_place), (Pane::Reader, 1));
    }

    // A list scrolled to its selection: its nine rows show the
    // conversations from the eighth on.
    app.conversations = vec![Conversation::default(); 20];
    app.focus = Pane::Conversations;
    app.selected_conversation = 15;
    let _ = drawn(120, 12, &app, &Theme::new(false));
    mouse(&mut app, CLICK, 30, 1);
    assert_eq!(app.selected_conversation, 7);
}

#[test]
fn a_click_on_a_message_folded_into_a_line_opens_it() {
    let messages = [
        message("alex@example.test", 0, 0, "First thoughts"),
        message("sam@example.test", 1, 0, "Second thoughts"),
        message("alex@example.test", 2, 0, "The newest words"),
    ];
    let mut app = opened(&messages);
    let screen = text(&drawn(200, 40, &app, &Theme::new(false)));
    let row = |screen: &str, text: &str| {
        let at = screen.lines().position(|line| line.contains(text));
        u16::try_from(at.unwrap()).unwrap()
    };
    // A click on a message shown only gives the reader the keys.
    app.focus = Pane::Folders;
    assert!(mouse(&mut app, CLICK, 150, row(&screen, "The newest words")).is_empty());
    assert_eq!(app.focus, Pane::Reader);
    assert!(app.toggled.is_empty());

    assert!(mouse(&mut app, CLICK, 150, row(&screen, "· Second thoughts")).is_empty());
    let screen = text(&drawn(200, 40, &app, &Theme::new(false)));
    assert!(screen.contains("Date:    2024-04-25 00:06"), "{screen}");
    assert!(screen.contains("· First thoughts"), "{screen}");
}

#[test]
fn a_row_without_a_date_keeps_its_columns() {
    let mut app = with_mail("alex@example.test", "Dated");
    app.conversations.push(Conversation {
        subject: "Undated".into(),
        senders: vec![Recipient::new("sam@example.test")],
        ..Conversation::default()
    });
    // Beside the other panes, a row shows its day without the year; alone,
    // with it.
    for width in [120, 60] {
        app.focus = Pane::Conversations;
        let screen = text(&drawn(width, 30, &app, &Theme::new(false)));
        // In characters, which here take a column each.
        let column = |text: &str| {
            let row = screen.lines().find(|line| line.contains(text))?;
            Some(row[..row.find(text)?].chars().count())
        };
        assert!(screen.contains(" -  "), "{screen}");
        assert_eq!(column("alex@"), column("sam@"), "{screen}");
        assert_eq!(column("Dated"), column("Undated"), "{screen}");
    }
}

#[test]
fn the_wheel_scrolls_a_list_without_moving_its_selection() {
    let mut app = with_mail("alex@example.test", "Quarterly report");
    app.conversations = (0..40)
        .map(|n| Conversation {
            id: format!("c{n}"),
            subject: format!("Subject {n}"),
            ..Conversation::default()
        })
        .collect();
    app.focus = Pane::Conversations;
    let _ = drawn(120, 30, &app, &Theme::new(false));
    // Two notches down, the list shows from its seventh row, the selection
    // on its first, out of sight.
    mouse(&mut app, MouseEventKind::ScrollDown, 30, 10);
    mouse(&mut app, MouseEventKind::ScrollDown, 30, 10);
    let screen = text(&drawn(120, 30, &app, &Theme::new(false)));
    assert_eq!(app.selected_conversation, 0);
    assert!(top_row(&screen).contains("Subject 6 "), "{screen}");
    // Drawn again, it stays there; a notch up, it goes back three rows.
    let screen = text(&drawn(120, 30, &app, &Theme::new(false)));
    assert!(top_row(&screen).contains("Subject 6 "), "{screen}");
    mouse(&mut app, MouseEventKind::ScrollUp, 30, 10);
    let screen = text(&drawn(120, 30, &app, &Theme::new(false)));
    assert!(top_row(&screen).contains("Subject 3 "), "{screen}");

    // A key moves the selection from where it was, and the list follows it.
    press(&mut app, KeyCode::Down);
    let screen = text(&drawn(120, 30, &app, &Theme::new(false)));
    assert_eq!(app.selected_conversation, 1);
    assert!(top_row(&screen).contains("Subject 1 "), "{screen}");
    // Up from there, the list only scrolls once the selection reaches its
    // top row.
    for _ in 0..30 {
        press(&mut app, KeyCode::Down);
    }
    let _ = drawn(120, 30, &app, &Theme::new(false));
    press(&mut app, KeyCode::Up);
    let screen = text(&drawn(120, 30, &app, &Theme::new(false)));
    assert_eq!(app.selected_conversation, 30);
    assert_eq!(app.conversations_scroll.get().top, 5);
    assert!(top_row(&screen).contains("Subject 5 "), "{screen}");
}

#[test]
fn the_wheel_scrolls_what_it_is_over_and_the_keys_over_the_panes() {
    let mut app = with_mail("alex@example.test", "Quarterly report");
    app.conversations.push(Conversation::default());
    app.reading = Some(Reading {
        id: "plans".into(),
        subject: "Plans for May".into(),
        lines: (0..100).map(|n| format!("line {n}")).collect(),
        quotes: Vec::new(),
        links: Vec::new(),
        attachments: Vec::new(),
        messages: Vec::new(),
        answer: None,
    });
    let _ = drawn(120, 30, &app, &Theme::new(false));
    // The reader a few rows a notch, the keys staying with the folders and
    // the list's selection where it is.
    mouse(&mut app, MouseEventKind::ScrollDown, 90, 10);
    mouse(&mut app, MouseEventKind::ScrollDown, 90, 10);
    mouse(&mut app, MouseEventKind::ScrollUp, 90, 10);
    assert_eq!(app.scroll.get(), 3);
    mouse(&mut app, MouseEventKind::ScrollDown, 30, 10);
    assert_eq!(app.selected_conversation, 0);
    assert_eq!(app.focus, Pane::Folders);

    // Over the keys, it scrolls them, and a click does nothing.
    app.update(Message::ToggleHelp);
    let _ = drawn(80, 12, &app, &Theme::new(false));
    assert!(app.help_bottom.get() > 3);
    mouse(&mut app, MouseEventKind::ScrollDown, 40, 6);
    assert_eq!(app.help_scroll, 3);
    mouse(&mut app, CLICK, 5, 2);
    assert!(app.help);
    assert_eq!(app.selected_place, 0);

    // The command line, like a question or a message being written,
    // keeps the keys, and the mouse does nothing meanwhile.
    app.update(Message::ToggleHelp);
    app.update(Message::Command);
    let _ = drawn(120, 30, &app, &Theme::new(false));
    mouse(&mut app, CLICK, 30, 2);
    mouse(&mut app, MouseEventKind::ScrollDown, 90, 10);
    assert_eq!((app.focus, app.scroll.get()), (Pane::Folders, 3));
    assert!(app.command_line.is_some());
}

#[test]
fn long_reader_lines_wrap_to_the_pane() {
    let mut app = with_mail("alex@example.test", "Quarterly report");
    app.reading = Some(Reading {
        id: "long".into(),
        subject: "Long".into(),
        lines: vec!["word ".repeat(60)],
        quotes: Vec::new(),
        links: Vec::new(),
        attachments: Vec::new(),
        messages: Vec::new(),
        answer: None,
    });
    let _ = drawn(120, 30, &app, &Theme::new(false));
    // 300 columns wrap into several rows of the 46-column pane.
    assert!(app.reader_bottom.get() == 0);
    let rows = drawn(120, 10, &app, &Theme::new(false));
    assert!(app.reader_bottom.get() > 0, "{}", text(&rows));
}

#[test]
fn a_quoted_line_stays_dim_on_every_row_it_wraps_to() {
    let mut app = with_mail("alex@example.test", "Quarterly report");
    app.reading = Some(Reading {
        id: "long".into(),
        subject: "Long".into(),
        lines: vec![format!("> {}", "quoted ".repeat(20)), "Fresh words".into()],
        quotes: Vec::new(),
        links: Vec::new(),
        attachments: Vec::new(),
        messages: Vec::new(),
        answer: None,
    });
    let buffer = drawn(120, 30, &app, &Theme::new(false));
    let screen = text(&buffer);
    let dim_at = |row: usize, word: &str| {
        let line = screen.lines().nth(row).unwrap();
        let column = line[..line.find(word).unwrap()].chars().count();
        let cell = &buffer[(u16::try_from(column).unwrap(), u16::try_from(row).unwrap())];
        cell.modifier.contains(Modifier::DIM)
    };
    let quoted: Vec<usize> = screen
        .lines()
        .enumerate()
        .filter(|(_, line)| line.contains("quoted"))
        .map(|(row, _)| row)
        .collect();
    // 140 columns wrap into several rows of the 46-column pane.
    assert!(quoted.len() > 1, "{screen}");
    assert!(quoted.iter().all(|&row| dim_at(row, "quoted")), "{screen}");
    let fresh = screen
        .lines()
        .position(|line| line.contains("Fresh"))
        .unwrap();
    assert!(!dim_at(fresh, "Fresh"));
}

#[test]
fn a_narrow_terminal_shows_the_focused_pane_alone() {
    let mut app = with_mail("alex@example.test", "Quarterly report");
    app.focus = Pane::Conversations;
    let screen = text(&drawn(60, 20, &app, &Theme::new(false)));
    assert!(screen.contains("Conversations"), "{screen}");
    assert!(screen.contains("Quarterly report"), "{screen}");
    assert!(!screen.contains("Folders"), "{screen}");
    assert!(!screen.contains("Reader"), "{screen}");
}

#[test]
fn the_status_bar_says_how_much_of_a_longer_list_is_loaded() {
    let mut app = with_mail("alex@example.test", "Quarterly report");
    app.total = 120;
    let screen = text(&drawn(120, 30, &app, &Theme::new(false)));
    let status = screen.lines().last().unwrap();
    assert!(
        status.contains("Inbox · 1 of 120 conversation(s)"),
        "{status}"
    );
}

#[test]
fn the_links_list_over_the_panes_and_ask_before_one_opens() {
    let mut app = with_mail("alex@example.test", "Quarterly report");
    app.reading = Some(Reading {
        id: "plans".into(),
        subject: "Plans".into(),
        lines: Vec::new(),
        quotes: Vec::new(),
        links: vec![
            "https://example.test/a".into(),
            "https://xn--bcher-kva.example/\u{1b}[2J".into(),
        ],
        attachments: Vec::new(),
        messages: Vec::new(),
        answer: None,
    });
    app.picking_link = Some(LinkPick {
        selected: 1,
        confirming: false,
    });
    let screen = text(&drawn(120, 30, &app, &Theme::new(false)));
    assert!(screen.contains(" 1. https://example.test/a"), "{screen}");
    assert!(
        screen.contains(" 2. https://xn--bcher-kva.example/[2J"),
        "{screen}"
    );
    assert!(screen.contains("Enter opens one"), "{screen}");

    app.picking_link = Some(LinkPick {
        selected: 1,
        confirming: true,
    });
    let screen = text(&drawn(120, 30, &app, &Theme::new(false)));
    assert!(
        screen.contains("Open this link in your browser?"),
        "{screen}"
    );
    assert!(
        screen.contains("https://xn--bcher-kva.example/[2J"),
        "{screen}"
    );
    assert!(screen.contains("y opens it"), "{screen}");
}

#[test]
fn a_message_being_written_takes_the_screen_with_its_fields_and_text() {
    let mut app = with_mail("alex@example.test", "Quarterly report");
    app.sender = "me@example.test".into();
    app.update(Message::Compose);
    let compose = app.compose.as_mut().unwrap();
    for c in "alex@example.test".chars() {
        compose.to.insert(c);
    }
    compose.focus = Field::Body;
    for c in "A line long enough to wrap twice in a terminal forty columns wide.".chars() {
        compose.body.insert(c);
    }
    let mut terminal = Terminal::new(TestBackend::new(40, 14)).unwrap();
    terminal
        .draw(|frame| draw(frame, &app, &Theme::new(false)))
        .unwrap();
    let screen = text(terminal.backend().buffer());
    for line in [
        " New message ",
        "From     me@example.test",
        "To       alex@example.test",
        "Subject",
        // The text wraps to the 38 columns inside the border.
        "A line long enough to wrap twice in a",
        "terminal forty columns wide.",
        "Ctrl+Enter or Ctrl+S send",
    ] {
        assert!(screen.contains(line), "{line} missing:\n{screen}");
    }
    assert!(!screen.contains("Folders"), "{screen}");
    // The cursor stands after the last character, in the text's second
    // row, under the five fields and the rule.
    assert_eq!(
        terminal.get_cursor_position().unwrap(),
        ratatui::layout::Position::new(29, 8)
    );
    // What went wrong takes the place of the keys.
    app.compose.as_mut().unwrap().notice = Some("Add someone to send this to.".into());
    let screen = text(&drawn(40, 14, &app, &Theme::new(false)));
    assert!(screen.contains("Add someone to send this to."), "{screen}");
    // Even a terminal too small for it draws without panicking.
    let _ = drawn(8, 3, &app, &Theme::new(false));
}

#[test]
fn an_answer_says_what_it_answers_and_what_goes_with_it() {
    let mut app = with_mail("alex@example.test", "Quarterly report");
    let answering = super::super::reading::Answerable {
        message_id: "m1".into(),
        from: None,
        subject: "Plans".into(),
        sender: "Alex Reed".into(),
        to: vec!["list@example.test".into()],
        cc: Vec::new(),
        redirected_from: Some("alex@example.test".into()),
        opened: true,
        files: vec![super::super::reading::ReadingAttachment {
            message_id: "m1".into(),
            id: "a1".into(),
            name: "plan.pdf".into(),
            size: 24_576,
        }],
    };
    app.compose = Some(Compose::answer(
        "me@example.test".into(),
        AnswerKind::Forward,
        answering.clone(),
    ));
    let screen = text(&drawn(100, 20, &app, &Theme::new(false)));
    for line in [
        " Forward ",
        "Answering “Plans” from Alex Reed.",
        "The original goes under your text; Ctrl+Q leaves it out.",
        "Forwarding: plan.pdf",
    ] {
        assert!(screen.contains(line), "{line} missing:\n{screen}");
    }
    // Core writes an answer's subject.
    assert!(!screen.contains("Subject"), "{screen}");
    assert!(!screen.contains("asks for replies"), "{screen}");
    app.compose = Some(Compose::answer(
        "me@example.test".into(),
        AnswerKind::Reply,
        answering,
    ));
    let screen = text(&drawn(100, 20, &app, &Theme::new(false)));
    for line in [
        " Reply ",
        "The message asks for replies to go to list@example.test, not to its sender, alex@example.test.",
        "To       list@example.test",
    ] {
        assert!(screen.contains(line), "{line} missing:\n{screen}");
    }
    assert!(!screen.contains("Forwarding"), "{screen}");
}

#[test]
fn the_files_a_message_carries_are_listed_over_it() {
    let mut app = with_mail("alex@example.test", "Quarterly report");
    app.update(Message::Compose);
    let compose = app.compose.as_mut().unwrap();
    compose.attached = vec![(
        std::path::PathBuf::from("/home/me/report\x1b[2J.pdf"),
        1_536,
    )];
    let screen = text(&drawn(100, 24, &app, &Theme::new(false)));
    assert!(
        screen.contains("Attached: report[2J.pdf (1.5 KB)"),
        "{screen}"
    );
    assert!(screen.contains("Ctrl+A attach"), "{screen}");
    let compose = app.compose.as_mut().unwrap();
    compose.files = Some(super::super::compose::Files {
        path: super::super::editor::Editor::with("~/no"),
        chosen: Some(0),
        matches: vec!["~/notes/".into(), "~/november.txt".into()],
        notice: Some("Cannot attach ~/x: gone".into()),
    });
    let screen = text(&drawn(100, 24, &app, &Theme::new(false)));
    for line in [
        " Attachments ",
        " report[2J.pdf · 1.5 KB",
        "Attach   ~/no",
        "notes/  november.txt",
        "Cannot attach ~/x: gone",
        "Enter attaches",
    ] {
        assert!(screen.contains(line), "{line} missing:\n{screen}");
    }
    assert!(!screen.contains('\u{1b}'));
}

#[test]
fn sending_asks_naming_who_gets_it_and_how_proton_protects_it() {
    let mut app = with_mail("alex@example.test", "Quarterly report");
    app.update(Message::Compose);
    let ask = |app: &mut App, checks: Vec<(String, Checked)>| {
        app.question = Some(Question::Send {
            request: 1,
            outgoing: Outgoing {
                kind: crate::mailbox::OutgoingKind::New,
                to: vec!["sam@proton.me".into()],
                cc: vec!["alex@example.test".into()],
                bcc: Vec::new(),
                subject: String::new(),
                body: String::new(),
                attachments: Vec::new(),
            },
            files: vec!["plan.pdf (24 KB)".into()],
            checks,
        });
        text(&drawn(100, 30, app, &Theme::new(false)))
    };
    let sam = |checked| ("sam@proton.me".to_owned(), checked);
    let alex = |checked| ("alex@example.test".to_owned(), checked);
    let end_to_end = Checked::Known(RecipientProtection::EndToEnd);
    let screen = ask(&mut app, vec![sam(end_to_end), alex(Checked::Checking)]);
    for line in [
        " Send ",
        "Send this message?",
        "To: sam@proton.me",
        "Cc: alex@example.test",
        "Files: plan.pdf (24 KB)",
        "Checking how this message will be protected…",
        "y sends it; any other key goes back to it.",
    ] {
        assert!(screen.contains(line), "{line} missing:\n{screen}");
    }
    let unencrypted = Checked::Known(RecipientProtection::Unencrypted);
    let screen = ask(&mut app, vec![sam(Checked::Unknown), alex(unencrypted)]);
    for line in [
        "Proton will not end-to-end encrypt this message for: alex@example.test",
        "Could not check how this message will be protected for: sam@proton.me",
    ] {
        assert!(screen.contains(line), "{line} missing:\n{screen}");
    }
    let pgp = Checked::Known(RecipientProtection::Pgp);
    let screen = ask(&mut app, vec![sam(end_to_end), alex(pgp)]);
    assert!(
        screen.contains("Proton will end-to-end encrypt this message for every recipient."),
        "{screen}"
    );

    app.question = Some(Question::Discard);
    let screen = text(&drawn(100, 30, &app, &Theme::new(false)));
    for line in [
        " Discard ",
        "Discard the message you are writing?",
        "This cannot be undone.",
        "y discards it; any other key goes back to it.",
    ] {
        assert!(screen.contains(line), "{line} missing:\n{screen}");
    }
}

#[test]
fn a_question_names_what_it_would_delete_forever_and_what_a_yes_does() {
    let mut app = with_mail("alex@example.test", "Quarterly report");
    let mut subjects = vec!["\u{1b}[2JYou won".to_owned(), String::new()];
    subjects.extend((3..=7).map(|n| format!("Old news {n}")));
    app.question = Some(Question::Delete {
        ids: subjects.clone(),
        subjects,
        place: label_ids::TRASH.into(),
        name: "Trash".into(),
    });
    let screen = text(&drawn(120, 30, &app, &Theme::new(false)));
    for line in [
        " Delete forever ",
        "Delete these 7 conversations in Trash forever?",
        "[2JYou won",
        "(no subject)",
        "Old news 5",
        "…and 2 more",
        "This cannot be undone.",
        "y deletes them; any other key keeps them.",
    ] {
        assert!(screen.contains(line), "{line} missing:\n{screen}");
    }
    assert!(!screen.contains("Old news 6"), "{screen}");

    app.question = Some(Question::Delete {
        ids: vec!["a".into()],
        subjects: vec!["Quarterly report".into()],
        place: label_ids::SPAM.into(),
        name: "Spam".into(),
    });
    let screen = text(&drawn(120, 30, &app, &Theme::new(false)));
    assert!(
        screen.contains("Delete this conversation in Spam forever?"),
        "{screen}"
    );
    assert!(
        screen.contains("y deletes it; any other key keeps it."),
        "{screen}"
    );

    app.question = Some(Question::Empty {
        place: label_ids::TRASH.into(),
        name: "Trash".into(),
    });
    let screen = text(&drawn(80, 24, &app, &Theme::new(false)));
    for line in [
        " Empty Trash ",
        "Delete everything in Trash forever?",
        "y empties it; any other key keeps what it holds.",
    ] {
        assert!(screen.contains(line), "{line} missing:\n{screen}");
    }
    // Even a terminal too small for it draws without panicking.
    let _ = drawn(12, 4, &app, &Theme::new(false));
}

#[test]
fn the_command_line_takes_the_status_bar_with_the_cursor_after_it() {
    let mut app = with_mail("alex@example.test", "Quarterly report");
    app.update(Message::Command);
    for c in "folder Wo".chars() {
        let key = KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        app.update(Message::Typed(key));
    }
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    terminal
        .draw(|frame| draw(frame, &app, &Theme::new(false)))
        .unwrap();
    let screen = text(terminal.backend().buffer());
    let status = screen.lines().last().unwrap();
    assert!(status.starts_with(":folder Wo "), "{status}");
    assert!(!status.contains("conversation(s)"), "{status}");
    let cursor = terminal.backend_mut().get_cursor_position().unwrap();
    assert_eq!((cursor.x, cursor.y), (10, 29));
}

#[test]
fn a_search_that_finds_nothing_says_where_esc_goes() {
    let mut app = App::new("me@example.test".into());
    app.update(Message::Command);
    for c in "search zebra\u{1b}[2J".chars() {
        let key = KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        app.update(Message::Typed(key));
    }
    app.update(Message::Typed(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )));
    app.update(Message::ConversationsLoaded {
        request: 1,
        page: 0,
        result: Ok((0, Vec::new())),
    });
    let screen = text(&drawn(120, 30, &app, &Theme::new(false)));
    assert!(screen.contains("Nothing matches zebra[2J."), "{screen}");
    assert!(screen.contains("Esc goes back to Inbox."), "{screen}");
    let status = screen.lines().last().unwrap();
    assert!(
        status.contains("Search: zebra[2J · 0 conversation(s)"),
        "{status}"
    );
}

#[test]
fn a_local_search_that_finds_nothing_says_how_mail_gets_indexed() {
    let mut app = App::new("me@example.test".into());
    app.update(Message::Command);
    for c in "local zebra".chars() {
        let key = KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        app.update(Message::Typed(key));
    }
    app.update(Message::Typed(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )));
    app.update(Message::ConversationsLoaded {
        request: 1,
        page: 0,
        result: Ok((0, Vec::new())),
    });
    let screen = text(&drawn(120, 30, &app, &Theme::new(false)));
    for said in [
        "Nothing in the local index matches zebra.",
        "`ruston-cli index` puts mail in it.",
        "Esc goes back to Inbox.",
    ] {
        assert!(screen.contains(said), "{said:?} missing:\n{screen}");
    }
    let status = screen.lines().last().unwrap();
    assert!(
        status.contains("Local: zebra · 0 conversation(s)"),
        "{status}"
    );
}

#[test]
fn the_attachments_list_over_the_panes_with_their_sizes() {
    let mut app = with_mail("alex@example.test", "Quarterly report");
    app.reading = Some(Reading {
        id: "plans".into(),
        subject: "Plans".into(),
        lines: Vec::new(),
        quotes: Vec::new(),
        links: Vec::new(),
        attachments: vec![super::super::reading::ReadingAttachment {
            message_id: "m1".into(),
            id: "a1".into(),
            name: "plan.pdf".into(),
            size: 24_576,
        }],
        messages: Vec::new(),
        answer: None,
    });
    app.picking_attachment = Some(0);
    let screen = text(&drawn(120, 30, &app, &Theme::new(false)));
    assert!(screen.contains("Attachments"), "{screen}");
    assert!(screen.contains(" plan.pdf · 24 KB"), "{screen}");
    assert!(screen.contains("Enter saves one to Downloads"), "{screen}");
}

#[test]
fn an_empty_list_says_so_and_how_to_go_back() {
    let mut app = App::new("me@example.test".into());
    // Still loading: nothing to say yet.
    let screen = text(&drawn(120, 30, &app, &Theme::new(false)));
    assert!(!screen.contains("No conversations"), "{screen}");

    app.listed = "Personal".into();
    app.update(Message::ConversationsLoaded {
        request: 0,
        page: 0,
        result: Ok((0, Vec::new())),
    });
    let screen = text(&drawn(120, 30, &app, &Theme::new(false)));
    assert!(screen.contains("No conversations in Personal."), "{screen}");
    assert!(screen.contains("Esc goes back to the folders."), "{screen}");
}

#[test]
fn the_status_bar_offers_z_while_a_move_can_be_taken_back() {
    let mut app = with_mail("alex@example.test", "Quarterly report");
    let status = |app: &App| {
        let screen = text(&drawn(80, 24, app, &Theme::new(false)));
        screen.lines().last().unwrap().to_owned()
    };
    assert!(!status(&app).contains("undo"), "{}", status(&app));
    app.update(Message::Changed {
        change: crate::mailbox::Change::Move(label_ids::ARCHIVE.into()),
        ids: vec!["a".into()],
        place: label_ids::INBOX.into(),
        result: Ok(()),
    });
    assert!(
        status(&app).ends_with("Z undo  Tab panes  ? keys  q quit "),
        "{}",
        status(&app)
    );
}

#[test]
fn the_status_bar_says_what_is_loading_or_what_went_wrong() {
    let mut app = with_mail("alex@example.test", "Quarterly report");
    app.notice = Some("Cannot load Inbox: offline\x1b[2J".into());
    let screen = text(&drawn(120, 30, &app, &Theme::new(false)));
    let status = screen.lines().last().unwrap();
    assert!(status.contains("Cannot load Inbox: offline[2J"), "{status}");
}

#[test]
fn the_keys_are_listed_over_the_panes() {
    let mut app = App::new("demo".into());
    app.help = true;
    let screen = text(&drawn(120, 30, &app, &Theme::new(false)));
    for (keys, action) in KEYS.iter().chain(SUSPEND_KEYS) {
        assert!(screen.contains(keys), "{keys} missing:\n{screen}");
        assert!(screen.contains(action), "{action} missing:\n{screen}");
    }
    // Even a terminal too small for the list draws without panicking.
    let _ = drawn(12, 4, &app, &Theme::new(false));
}

#[test]
fn after_g_the_status_bar_names_the_folders_it_goes_to() {
    let mut app = with_mail("alex@example.test", "Quarterly report");
    app.going = true;
    let screen = text(&drawn(80, 24, &app, &Theme::new(false)));
    let status = screen.lines().last().unwrap();
    for (key, _, name) in GO_TO {
        assert!(status.contains(&format!("{key} {name}")), "{status}");
    }
}

#[test]
fn recipients_past_the_lookup_limit_are_counted_in_one_line() {
    let encrypted = Checked::Known(RecipientProtection::EndToEnd);
    let checks = vec![("sam@proton.me".to_owned(), encrypted)];
    assert_eq!(
        protection(&checks, 0),
        ["Proton will end-to-end encrypt this message for every recipient."]
    );
    assert_eq!(
        protection(&checks, 1),
        ["Not checked: 1 more recipient, past the first 100."]
    );
    assert_eq!(
        protection(&checks, 7),
        ["Not checked: 7 more recipients, past the first 100."]
    );
}

#[test]
fn the_selection_of_a_pane_without_the_keys_never_reads_as_unread() {
    let mut app = with_mail("alex@example.test", "Quarterly report");
    app.conversations.push(Conversation {
        subject: "Read already".into(),
        time: 1_713_000_000,
        senders: vec![Recipient::new("sam@example.test")],
        ..Conversation::default()
    });
    app.selected_conversation = 1;
    app.focus = Pane::Reader;
    let buffer = drawn(120, 30, &app, &Theme::new(false));
    let screen = text(&buffer);
    let style_of = |word: &str| {
        let (row, line) = screen
            .lines()
            .enumerate()
            .find(|(_, line)| line.contains(word))
            .unwrap();
        let column = line[..line.find(word).unwrap()].chars().count();
        buffer[(u16::try_from(column).unwrap(), u16::try_from(row).unwrap())].modifier
    };
    // Bold is for unread mail; the selection elsewhere is underlined.
    let selected = style_of("sam@exa");
    assert!(selected.contains(Modifier::UNDERLINED) && !selected.contains(Modifier::BOLD));
    let unread = style_of("alex@exa");
    assert!(unread.contains(Modifier::BOLD) && !unread.contains(Modifier::UNDERLINED));
    // And under the list, what its marks say.
    assert!(
        screen.contains("✓ marked · • unread · ★ starred · + files"),
        "{screen}"
    );
}

#[test]
fn every_key_can_be_reached_at_80_by_24_and_where_they_do_not_fit() {
    let all = KEYS.len() + SUSPEND_KEYS.len();
    for (width, height) in [(80, 24), (80, 12)] {
        let mut app = App::new("demo".into());
        app.update(Message::ToggleHelp);
        // Each screen down to the last row, and one more.
        let mut seen = String::new();
        for _ in 0..=all {
            seen.push_str(&text(&drawn(width, height, &app, &Theme::new(false))));
            app.update(Message::Down);
        }
        for (keys, action) in KEYS.iter().chain(SUSPEND_KEYS) {
            assert!(seen.contains(keys), "{keys} missing at {height} rows");
            assert!(seen.contains(action), "{action} missing at {height} rows");
        }
        // With what `:search` and `:local` take beside their words.
        assert!(seen.contains(SEARCH_FILTERS.trim()), "{seen}");
    }
    // Too few rows for all of them, the first screen says how to see the rest.
    let mut app = App::new("demo".into());
    app.update(Message::ToggleHelp);
    let screen = text(&drawn(80, 12, &app, &Theme::new(false)));
    let (last, _) = SUSPEND_KEYS.last().unwrap_or(&KEYS[KEYS.len() - 1]);
    assert!(!screen.contains(last), "{screen}");
    assert!(screen.contains("j, k scroll"), "{screen}");
}

#[test]
fn the_keys_take_two_columns_where_the_terminal_is_wide_enough() {
    let mut app = App::new("demo".into());
    app.update(Message::ToggleHelp);
    let half = (KEYS.len() + SUSPEND_KEYS.len()).div_ceil(2);
    let row_of = |screen: &str, keys: &str| {
        screen
            .lines()
            .position(|line| line.contains(keys))
            .unwrap_or_else(|| panic!("{keys} missing:\n{screen}"))
    };
    let screen = text(&drawn(200, 30, &app, &Theme::new(false)));
    assert_eq!(
        row_of(&screen, KEYS[0].0),
        row_of(&screen, KEYS[half].0),
        "{screen}"
    );
    // One column where two would not fit.
    let screen = text(&drawn(120, 30, &app, &Theme::new(false)));
    assert!(row_of(&screen, KEYS[0].0) < row_of(&screen, KEYS[half].0));
}

#[test]
fn no_color_leaves_every_color_to_the_terminal() {
    let mut app = with_mail("alex@example.test", "Quarterly report");
    app.help = true;
    let buffer = drawn(120, 30, &app, &Theme::new(true));
    assert!(
        buffer
            .content()
            .iter()
            .all(|cell| cell.fg == Color::Reset && cell.bg == Color::Reset)
    );
    // With colors, the accent marks the focused pane and unread counts.
    let buffer = drawn(120, 30, &app, &Theme::new(false));
    assert!(buffer.content().iter().any(|cell| cell.fg == ACCENT));
}

#[test]
#[cfg(not(windows))]
fn the_accent_is_ruston_mails_violet_as_near_as_the_terminal_shows_it() {
    assert_eq!(accent_among("truecolor", "xterm-256color"), ACCENT);
    assert_eq!(accent_among("24bit", ""), ACCENT);
    // As Terminal.app says it, without COLORTERM.
    assert_eq!(accent_among("", "xterm-256color"), Color::Indexed(99));
    assert_eq!(accent_among("", "screen-256color"), Color::Indexed(99));
    assert_eq!(accent_among("", "linux"), Color::Magenta);
}
