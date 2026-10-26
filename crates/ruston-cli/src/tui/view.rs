//! How the interface looks: the three panes side by side, or the focused one
//! alone in a narrow terminal, a status bar, and the keys over them on `?`.
//! It draws from the state alone, so it can be tested on a `TestBackend`.
//! Every piece of text that comes with the mail goes through
//! `terminal::screen_line` first.

use super::app::{App, Pane};
use crate::render;
use crate::terminal::screen_line;
use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ruston_core::Conversation;
use ruston_core::model::enums::label_ids;

/// Below this width the panes take turns on the screen, as the desktop's
/// compact view does.
const THREE_PANES_MIN_WIDTH: u16 = 100;

/// The folder list's width when the panes stand side by side.
const FOLDERS_WIDTH: u16 = 24;

/// How many columns a conversation's sender takes in its row.
const SENDER_WIDTH: usize = 18;

/// Each key and what it does, as the list of keys shows them.
const KEYS: [(&str, &str); 10] = [
    ("Tab, Shift+Tab", "next or previous pane"),
    ("j, k, arrows", "move, or scroll the reader"),
    ("PageUp, PageDown", "scroll the reader a page"),
    ("g, G, Home, End", "go to the top or the bottom"),
    ("Enter", "open the folder or conversation"),
    ("z", "show or hide quoted text"),
    ("Ctrl+R", "refresh the folders and the list"),
    ("?", "show or hide these keys"),
    ("Esc", "close these keys"),
    ("q, Ctrl+C", "quit"),
];

/// Ctrl+Z, where the shell has job control to suspend the interface with.
const SUSPEND_KEYS: &[(&str, &str)] = if cfg!(unix) {
    &[("Ctrl+Z", "suspend to the shell")]
} else {
    &[]
};

/// The terminal's own colors with one accent, or none at all when
/// `NO_COLOR` is set, as <https://no-color.org> asks.
pub(crate) struct Theme {
    accent: Style,
}

impl Theme {
    pub(crate) fn from_env() -> Self {
        Self::new(std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty()))
    }

    fn new(no_color: bool) -> Self {
        let accent = if no_color {
            Style::new().add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(Color::Cyan)
        };
        Self { accent }
    }
}

pub(crate) fn draw(frame: &mut Frame, app: &App, theme: &Theme) {
    let [main, status] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(frame.area());
    if main.width >= THREE_PANES_MIN_WIDTH {
        let [folders, conversations, reader] = Layout::horizontal([
            Constraint::Length(FOLDERS_WIDTH),
            Constraint::Percentage(40),
            Constraint::Fill(1),
        ])
        .areas(main);
        for (pane, area) in Pane::ALL.into_iter().zip([folders, conversations, reader]) {
            draw_pane(frame, app, theme, pane, area);
        }
    } else {
        draw_pane(frame, app, theme, app.focus, main);
    }
    draw_status(frame, app, theme, status);
    if app.help {
        draw_keys(frame, theme);
    }
}

fn draw_pane(frame: &mut Frame, app: &App, theme: &Theme, pane: Pane, area: Rect) {
    let focused = pane == app.focus;
    let mut block = Block::bordered().title(format!(" {} ", pane.title()));
    if focused {
        block = block.border_style(theme.accent);
    }
    match pane {
        Pane::Folders => {
            let items = app.places.iter().map(|place| {
                let mut spans = vec![Span::raw(format!(
                    "{}{}",
                    "  ".repeat(place.depth),
                    screen_line(&place.name)
                ))];
                if place.unread > 0 {
                    spans.push(Span::styled(format!(" {}", place.unread), theme.accent));
                }
                ListItem::new(Line::from(spans))
            });
            draw_list(frame, items, app.selected_place, focused, block, area);
        }
        Pane::Conversations => {
            let items = app.conversations.iter().map(conversation_row);
            draw_list(
                frame,
                items,
                app.selected_conversation,
                focused,
                block,
                area,
            );
        }
        Pane::Reader => draw_reader(frame, app, block, area),
    }
}

/// A list with its selection on `selected`, shown inverted in the focused
/// pane, which reads without colors too, and in bold elsewhere.
fn draw_list<'a>(
    frame: &mut Frame,
    items: impl Iterator<Item = ListItem<'a>>,
    selected: usize,
    focused: bool,
    block: Block,
    area: Rect,
) {
    let highlight = if focused {
        Style::new().add_modifier(Modifier::REVERSED)
    } else {
        Style::new().add_modifier(Modifier::BOLD)
    };
    let list = List::new(items).block(block).highlight_style(highlight);
    let mut state = ListState::default().with_selected(Some(selected));
    frame.render_stateful_widget(list, area, &mut state);
}

/// The open conversation, its quoted passages folded into a line each
/// unless the state shows them, wrapped to the pane and scrolled as far as
/// the state asks, within what there is. Since only the drawing knows how many
/// rows the text wraps to, it tells the state how far it can scroll.
fn draw_reader(frame: &mut Frame, app: &App, block: Block, area: Rect) {
    let inner = block.inner(area);
    let Some(reading) = &app.reading else {
        app.reader_bottom.set(0);
        let hint = Line::styled(
            "Enter on a conversation opens it here.",
            Style::new().add_modifier(Modifier::DIM),
        );
        frame.render_widget(Paragraph::new(hint).block(block), area);
        return;
    };
    let mut lines = vec![
        Line::styled(
            reading.subject.as_str(),
            Style::new().add_modifier(Modifier::BOLD),
        ),
        Line::default(),
    ];
    if app.show_quotes {
        lines.extend(raw(&reading.lines));
    } else {
        let mut rest = 0;
        for quote in &reading.quotes {
            lines.extend(raw(&reading.lines[rest..quote.start]));
            lines.push(Line::styled(
                format!("… {} quoted lines (z shows them)", quote.len()),
                Style::new().add_modifier(Modifier::DIM),
            ));
            rest = quote.end;
        }
        lines.extend(raw(&reading.lines[rest..]));
    }
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    let rows = paragraph.line_count(inner.width);
    let bottom = u16::try_from(rows.saturating_sub(usize::from(inner.height))).unwrap_or(u16::MAX);
    app.reader_bottom.set(bottom);
    app.reader_rows.set(inner.height.max(1));
    frame.render_widget(
        paragraph.scroll((app.scroll.min(bottom), 0)).block(block),
        area,
    );
}

/// Each of `lines` as it is, without a style.
fn raw(lines: &[String]) -> impl Iterator<Item = Line<'_>> {
    lines.iter().map(|line| Line::raw(line.as_str()))
}

/// One row of the list: whether it is unread (•), starred (★) or has
/// attachments (+), its day, who wrote it and its subject. Unread
/// conversations are bold.
fn conversation_row(conversation: &Conversation) -> ListItem<'static> {
    let unread = conversation.num_unread > 0;
    let starred = conversation
        .labels
        .iter()
        .any(|label| label.id == label_ids::STARRED);
    let flags = format!(
        "{}{}{}",
        if unread { "•" } else { " " },
        if starred { "★" } else { " " },
        if conversation.num_attachments > 0 {
            "+"
        } else {
            " "
        },
    );
    let time = render::fmt_time(conversation.time);
    let day = time.get(..10).unwrap_or(&time);
    let sender = conversation.senders.first().map_or("-", |sender| {
        if sender.name.is_empty() {
            &sender.address
        } else {
            &sender.name
        }
    });
    let style = if unread {
        Style::new().add_modifier(Modifier::BOLD)
    } else {
        Style::new()
    };
    ListItem::new(Line::from(vec![
        Span::raw(format!("{flags} {day}  ")),
        Span::styled(
            format!("{:<SENDER_WIDTH$.SENDER_WIDTH$}  ", screen_line(sender)),
            style,
        ),
        Span::styled(
            screen_line(render::subject_or_empty(&conversation.subject)),
            style,
        ),
    ]))
}

fn draw_status(frame: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let hints = Line::from(vec![
        Span::styled("Tab", theme.accent),
        Span::raw(" panes  "),
        Span::styled("?", theme.accent),
        Span::raw(" keys  "),
        Span::styled("q", theme.accent),
        Span::raw(" quit "),
    ]);
    let [state, hints_area] = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(u16::try_from(hints.width()).unwrap_or(u16::MAX)),
    ])
    .areas(area);
    let detail = match &app.notice {
        Some(notice) => notice.clone(),
        None if app.conversations.len() < usize::try_from(app.total).unwrap_or(usize::MAX) => {
            format!(
                "{} of {} conversation(s)",
                app.conversations.len(),
                app.total
            )
        }
        None => format!("{} conversation(s)", app.total),
    };
    let text = format!(" {} · {} · {detail}", app.account, app.listed);
    frame.render_widget(Paragraph::new(screen_line(&text)), state);
    frame.render_widget(Paragraph::new(hints), hints_area);
}

fn draw_keys(frame: &mut Frame, theme: &Theme) {
    let lines: Vec<Line> = KEYS
        .iter()
        .chain(SUSPEND_KEYS)
        .map(|(keys, action)| {
            Line::from(vec![
                Span::styled(format!(" {keys:<18}"), theme.accent),
                Span::raw(*action),
            ])
        })
        .collect();
    let width = lines.iter().map(Line::width).max().unwrap_or(0) + 3;
    let height = lines.len() + 2;
    let area = centered(
        frame.area(),
        u16::try_from(width).unwrap_or(u16::MAX),
        u16::try_from(height).unwrap_or(u16::MAX),
    );
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(" Keys ").border_style(theme.accent)),
        area,
    );
}

/// A `width` by `height` area in the middle of `area`, no larger than it.
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let [area] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(area);
    let [area] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(area);
    area
}

#[cfg(test)]
mod tests {
    use super::super::reading::Reading;
    use super::*;
    use crate::mailbox::{Place, PlaceKind};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ruston_core::Recipient;

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
                depth: 0,
                kind: PlaceKind::System,
                unread: 3,
            },
            Place {
                id: "f-2024".into(),
                name: "2024".into(),
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
        // The pane is 48 columns wide: the subject is cut where it ends.
        assert!(
            screen.contains("•   2024-04-24  alex@example.test   Quarterly │"),
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
        assert!(
            screen.contains(" ★+ 2024-04-24  alex@example.test"),
            "{screen}"
        );
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
        });
        let screen = text(&drawn(120, 30, &app, &Theme::new(false)));
        assert!(screen.contains("Plans for May"), "{screen}");
        assert!(screen.contains("line 0"), "{screen}");
        // The subject, a blank row and 100 lines, 27 rows of them in view.
        assert_eq!(app.reader_bottom.get(), 102 - 27);
        assert_eq!(app.reader_rows.get(), 27);

        app.scroll = 50;
        let screen = text(&drawn(120, 30, &app, &Theme::new(false)));
        assert!(!screen.contains("line 0 "), "{screen}");
        assert!(screen.contains("line 48"), "{screen}");
        // Scrolled past the end, as after the pane grew, it shows the end.
        app.scroll = 500;
        let screen = text(&drawn(120, 30, &app, &Theme::new(false)));
        assert!(screen.contains("line 99"), "{screen}");
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
    fn long_reader_lines_wrap_to_the_pane() {
        let mut app = with_mail("alex@example.test", "Quarterly report");
        app.reading = Some(Reading {
            id: "long".into(),
            subject: "Long".into(),
            lines: vec!["word ".repeat(60)],
            quotes: Vec::new(),
        });
        let _ = drawn(120, 30, &app, &Theme::new(false));
        // 300 columns wrap into several rows of the 46-column pane.
        assert!(app.reader_bottom.get() == 0);
        let rows = drawn(120, 10, &app, &Theme::new(false));
        assert!(app.reader_bottom.get() > 0, "{}", text(&rows));
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
        assert!(buffer.content().iter().any(|cell| cell.fg == Color::Cyan));
    }
}
