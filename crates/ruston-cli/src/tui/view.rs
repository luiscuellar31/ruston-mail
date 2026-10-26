//! How the interface looks: the three panes side by side, or the focused one
//! alone in a narrow terminal, a status bar, and the keys over them on `?`.
//! It draws from the state alone, so it can be tested on a `TestBackend`.

use super::app::{App, Pane};
use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

/// Below this width the panes take turns on the screen, as the desktop's
/// compact view does.
const THREE_PANES_MIN_WIDTH: u16 = 100;

/// The folder list's width when the panes stand side by side.
const FOLDERS_WIDTH: u16 = 24;

/// Each key and what it does, as the list of keys shows them.
const KEYS: [(&str, &str); 4] = [
    ("Tab, Shift+Tab", "next or previous pane"),
    ("?", "show or hide these keys"),
    ("Esc", "close these keys"),
    ("q, Ctrl+C", "quit"),
];

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
    let mut block = Block::bordered().title(format!(" {} ", pane.title()));
    if pane == app.focus {
        block = block.border_style(theme.accent);
    }
    frame.render_widget(block, area);
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
    let [account, hints_area] = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(u16::try_from(hints.width()).unwrap_or(u16::MAX)),
    ])
    .areas(area);
    frame.render_widget(
        Paragraph::new(format!(" Ruston Mail · {}", app.account)),
        account,
    );
    frame.render_widget(Paragraph::new(hints), hints_area);
}

fn draw_keys(frame: &mut Frame, theme: &Theme) {
    let lines: Vec<Line> = KEYS
        .iter()
        .map(|(keys, action)| {
            Line::from(vec![
                Span::styled(format!(" {keys:<16}"), theme.accent),
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
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

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

    #[test]
    fn a_wide_terminal_shows_the_three_panes_and_the_status_bar() {
        let app = App::new("demo".into());
        let screen = text(&drawn(120, 30, &app, &Theme::new(false)));
        for title in ["Folders", "Conversations", "Reader"] {
            assert!(screen.contains(title), "{title} missing:\n{screen}");
        }
        let status = screen.lines().last().unwrap();
        assert!(status.contains("Ruston Mail · demo"), "{status}");
        assert!(status.contains("q quit"), "{status}");
    }

    #[test]
    fn a_narrow_terminal_shows_the_focused_pane_alone() {
        let mut app = App::new("demo".into());
        app.focus = Pane::Conversations;
        let screen = text(&drawn(60, 20, &app, &Theme::new(false)));
        assert!(screen.contains("Conversations"), "{screen}");
        assert!(!screen.contains("Folders"), "{screen}");
        assert!(!screen.contains("Reader"), "{screen}");
    }

    #[test]
    fn the_keys_are_listed_over_the_panes() {
        let mut app = App::new("demo".into());
        app.help = true;
        let screen = text(&drawn(120, 30, &app, &Theme::new(false)));
        for (keys, action) in KEYS {
            assert!(screen.contains(keys), "{keys} missing:\n{screen}");
            assert!(screen.contains(action), "{action} missing:\n{screen}");
        }
        // Even a terminal too small for the list draws without panicking.
        let _ = drawn(12, 4, &app, &Theme::new(false));
    }

    #[test]
    fn no_color_leaves_every_color_to_the_terminal() {
        let mut app = App::new("demo".into());
        app.help = true;
        let buffer = drawn(120, 30, &app, &Theme::new(true));
        assert!(
            buffer
                .content()
                .iter()
                .all(|cell| cell.fg == Color::Reset && cell.bg == Color::Reset)
        );
        // With colors, the accent marks the focused pane.
        let buffer = drawn(120, 30, &app, &Theme::new(false));
        assert!(buffer.content().iter().any(|cell| cell.fg == Color::Cyan));
    }
}
