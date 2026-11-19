//! How the interface looks: the three panes side by side, or the focused one
//! alone in a narrow terminal, a status bar, and the keys over them on `?`.
//! It draws from the state alone, so it can be tested on a `TestBackend`.
//! Every piece of text that comes with the mail goes through
//! `terminal::screen_line` first.

use super::app::{App, Drawn, GO_TO, LinkPick, Pane, Query, Question};
use super::compose::{Checked, Compose, Field};
use super::reading::{Reading, is_quoted, size_label};
use crate::mailbox::OutgoingKind;
use crate::render;
use crate::terminal::screen_line;
use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ruston_core::mail::send::PROTECTION_LOOKUP_LIMIT;
use ruston_core::model::enums::label_ids;
use ruston_core::{Conversation, Quote, RecipientProtection};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Below this width the panes take turns on the screen, as the desktop's
/// compact view does.
const THREE_PANES_MIN_WIDTH: u16 = 100;

/// The folder list's width when the panes stand side by side.
const FOLDERS_WIDTH: u16 = 24;

/// What the marks at the start of a conversation's row say, under the list.
const ROW_MARKS: &str = " ✓ marked · • unread · ★ starred · + files ";

/// How many columns a conversation's sender takes in its row, at most: a
/// narrower row gives it a quarter of its width.
const SENDER_WIDTH: usize = 18;

/// The narrowest row whose day shows its year; a narrower one, as beside the
/// other panes below 155 columns, leaves it to the reader.
const YEAR_WIDTH: usize = 60;

/// How many conversations a question names before it counts the rest.
const NAMED: usize = 5;

/// The width of a field's name, before what it holds.
const LABEL_WIDTH: u16 = 9;

/// Each key and what it does, as the list of keys shows them: Proton Mail's
/// where it has one, and keys it leaves free for what only a terminal needs.
const KEYS: [(&str, &str); 23] = [
    ("Tab, Shift+Tab", "next or previous pane"),
    ("j, k, Up, Down", "move, or scroll the reader"),
    ("Enter, Right, Left", "open it, or go back a pane"),
    (
        "Enter, in the reader",
        "fold or open the message at its top",
    ),
    (
        "Click, wheel",
        "select it, open it once selected, or scroll",
    ),
    ("PageUp, PageDown", "scroll the reader a page"),
    ("Home, End, Ctrl+Up/Down", "go to the top or the bottom"),
    (
        "g, then i, a, e, *…",
        "go to the Inbox, Archive, Sent, Starred…",
    ),
    ("z, e", "show or hide quoted text, or messages already read"),
    ("b, v", "list its links, or its attachments"),
    ("n, Ctrl+N", "write a new message"),
    ("r, R, F", "in the reader: reply, reply to all, forward"),
    ("*, r, u", "star or unstar it, mark it read or unread"),
    (
        "a, s, i, t, Delete",
        "archive it, or move it to Spam, the Inbox or Trash",
    ),
    ("Z", "take the last move back"),
    ("Ctrl+Backspace", "delete it forever, in Trash or Spam"),
    (
        "l, L, m",
        "label it, remove a label, or move it to a folder",
    ),
    (
        "Space, x, Ctrl+A",
        "mark it, or all; an action acts on all marked",
    ),
    ("/, Ctrl+F, :, Ctrl+K", "search all mail, or type a command"),
    ("Ctrl+R", "refresh the folders and the list"),
    ("?", "show or hide these keys"),
    ("Esc", "close these keys, clear the marks, or go back"),
    ("q, Ctrl+C", "quit"),
];

/// What `:search` takes beside its words, under the list of keys.
const SEARCH_FILTERS: &str = " :search words from:who to:who after:day before:day in:folder ";

/// Ctrl+Z, where the shell has job control to suspend the interface with.
const SUSPEND_KEYS: &[(&str, &str)] = if cfg!(unix) {
    &[("Ctrl+Z", "suspend to the shell")]
} else {
    &[]
};

/// Ruston Mail's violet, the accent the desktop draws.
const ACCENT: Color = Color::Rgb(109, 92, 245);

/// The terminal's own colors with one accent, Ruston Mail's violet, or none
/// at all when `NO_COLOR` is set, as <https://no-color.org> asks.
pub(crate) struct Theme {
    accent: Style,
    /// The labels a row of the list names: in the accent's color, or in the
    /// terminal's own without colors, since bold says a row is unread.
    label: Style,
}

impl Theme {
    pub(crate) fn from_env() -> Self {
        if std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty()) {
            return Self::new(true);
        }
        let accent = Style::new().fg(accent_color());
        Self {
            accent,
            label: accent,
        }
    }

    fn new(no_color: bool) -> Self {
        if no_color {
            return Self {
                accent: Style::new().add_modifier(Modifier::BOLD),
                label: Style::new(),
            };
        }
        let accent = Style::new().fg(ACCENT);
        Self {
            accent,
            label: accent,
        }
    }
}

/// Ruston Mail's violet as this terminal shows it: the Windows console and
/// Windows Terminal show any color.
#[cfg(windows)]
fn accent_color() -> Color {
    ACCENT
}

/// Ruston Mail's violet as this terminal shows it, by what its environment
/// says it can show.
#[cfg(not(windows))]
fn accent_color() -> Color {
    let var = |name| std::env::var(name).unwrap_or_default();
    accent_among(&var("COLORTERM"), &var("TERM"))
}

/// Ruston Mail's violet as a terminal can show it: exactly where `COLORTERM`
/// says it shows any color, the 256-color palette's nearest violet, #875fff,
/// where `TERM` says it has those, and its own magenta otherwise. A color a
/// terminal cannot show could come out as another, green in some.
#[cfg(not(windows))]
fn accent_among(colorterm: &str, term: &str) -> Color {
    if matches!(colorterm, "truecolor" | "24bit") {
        ACCENT
    } else if term.contains("256color") {
        Color::Indexed(99)
    } else {
        Color::Magenta
    }
}

pub(crate) fn draw(frame: &mut Frame, app: &App, theme: &Theme) {
    // Each pane drawn below notes where it is.
    app.drawn.set(Drawn::default());
    // The message being written takes the whole screen.
    if let Some(compose) = &app.compose {
        draw_compose(frame, compose, theme, app.question.is_none());
        if let Some(question) = &app.question {
            draw_question(frame, question, theme);
        }
        return;
    }
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
    if let Some(pick) = app.picking_link {
        draw_links(frame, app, pick, theme);
    }
    if let Some(selected) = app.picking_attachment {
        draw_attachments(frame, app, selected, theme);
    }
    if let Some(question) = &app.question {
        draw_question(frame, question, theme);
    }
    if app.help {
        draw_keys(frame, app, theme);
    }
}

/// The question on screen, over everything else: what it would delete
/// forever, naming the first few conversations by their subjects; whether
/// to throw away the message being written; or whether to send it, to whom
/// and how Proton would protect it. Each says what a yes does.
fn draw_question(frame: &mut Frame, question: &Question, theme: &Theme) {
    let (title, lines) = match question {
        Question::Delete { subjects, name, .. } => {
            let (asks, yes) = match subjects.len() {
                1 => (
                    format!("Delete this conversation in {name} forever?"),
                    "y deletes it; any other key keeps it.",
                ),
                count => (
                    format!("Delete these {count} conversations in {name} forever?"),
                    "y deletes them; any other key keeps them.",
                ),
            };
            let mut lines = vec![Line::raw(screen_line(&asks)), Line::default()];
            lines.extend(subjects.iter().take(NAMED).map(|subject| {
                Line::styled(screen_line(render::subject_or_empty(subject)), theme.accent)
            }));
            if subjects.len() > NAMED {
                lines.push(Line::raw(format!("…and {} more", subjects.len() - NAMED)));
            }
            lines.extend(undone(yes));
            (" Delete forever ".to_owned(), lines)
        }
        Question::Empty { name, .. } => {
            let mut lines = vec![Line::raw(screen_line(&format!(
                "Delete everything in {name} forever?"
            )))];
            lines.extend(undone("y empties it; any other key keeps what it holds."));
            (format!(" Empty {} ", screen_line(name)), lines)
        }
        Question::Discard => {
            let mut lines = vec![Line::raw("Discard the message you are writing?")];
            lines.extend(undone("y discards it; any other key goes back to it."));
            (" Discard ".to_owned(), lines)
        }
        Question::Send {
            outgoing,
            files,
            checks,
            ..
        } => {
            let mut lines = vec![Line::raw("Send this message?"), Line::default()];
            for (field, people) in [
                ("To", &outgoing.to),
                ("Cc", &outgoing.cc),
                ("Bcc", &outgoing.bcc),
                ("Files", files),
            ] {
                if !people.is_empty() {
                    let named = format!("{field}: {}", people.join(", "));
                    lines.push(Line::raw(screen_line(&named)));
                }
            }
            lines.push(Line::default());
            // Recipients past the lookup limit were not asked about.
            let unchecked = outgoing.recipients().len().saturating_sub(checks.len());
            lines.extend(
                protection(checks, unchecked)
                    .iter()
                    .map(|said| Line::raw(screen_line(said))),
            );
            lines.extend([
                Line::default(),
                Line::raw("y sends it; any other key goes back to it."),
            ]);
            (" Send ".to_owned(), lines)
        }
    };
    let area = frame.area();
    let widest = lines.iter().map(Line::width).max().unwrap_or(0) + 2;
    let width = u16::try_from(widest)
        .unwrap_or(u16::MAX)
        .min(area.width.saturating_sub(4));
    let question = Paragraph::new(lines).wrap(Wrap { trim: false });
    let rows = question.line_count(width.saturating_sub(2)) + 2;
    let area = centered(area, width, u16::try_from(rows).unwrap_or(u16::MAX));
    let block = Block::bordered().title(title).border_style(theme.accent);
    frame.render_widget(Clear, area);
    frame.render_widget(question.block(block), area);
}

/// The end of a question about what cannot be undone, with what a yes does.
fn undone(yes: &'static str) -> [Line<'static>; 3] {
    [
        Line::default(),
        Line::raw("This cannot be undone."),
        Line::raw(yes),
    ]
}

/// What the question before sending says about how Proton would protect
/// the message, as the desktop's editor says it: who would not get it
/// end-to-end encrypted, who could not be checked, and how many `unchecked`
/// past the lookup limit were not asked about, as soon as any is known;
/// until every recipient is, that it is checking; then that every one would.
fn protection(checks: &[(String, Checked)], unchecked: usize) -> Vec<String> {
    let with = |wanted: Checked| -> Vec<&str> {
        checks
            .iter()
            .filter(|(_, checked)| *checked == wanted)
            .map(|(address, _)| address.as_str())
            .collect()
    };
    let unencrypted = with(Checked::Known(RecipientProtection::Unencrypted));
    let unknown = with(Checked::Unknown);
    let mut said = Vec::new();
    if !unencrypted.is_empty() {
        said.push(format!(
            "Proton will not end-to-end encrypt this message for: {}",
            unencrypted.join(", ")
        ));
    }
    if !unknown.is_empty() {
        said.push(format!(
            "Could not check how this message will be protected for: {}",
            unknown.join(", ")
        ));
    }
    if unchecked > 0 {
        let recipients = if unchecked == 1 {
            "recipient"
        } else {
            "recipients"
        };
        said.push(format!(
            "Not checked: {unchecked} more {recipients}, past the first {PROTECTION_LOOKUP_LIMIT}."
        ));
    }
    if said.is_empty() {
        let checking = checks
            .iter()
            .any(|(_, checked)| *checked == Checked::Checking);
        said.push(if checking {
            "Checking how this message will be protected…".to_owned()
        } else {
            "Proton will end-to-end encrypt this message for every recipient.".to_owned()
        });
    }
    said
}

/// The message being written, over the whole screen: the address it goes
/// from, its fields and its text, and at the foot what the keys do, or what
/// is under way or went wrong. A field scrolls sideways to keep the cursor
/// in sight, and the text, wrapped as the editor wraps it, scrolls down;
/// the cursor stands where typing goes, unless a question covers it all.
fn draw_compose(frame: &mut Frame, compose: &Compose, theme: &Theme, cursor: bool) {
    let [main, foot] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(frame.area());
    let title = match &compose.kind {
        OutgoingKind::New => " New message ",
        OutgoingKind::Reply {
            everyone: false, ..
        } => " Reply ",
        OutgoingKind::Reply { everyone: true, .. } => " Reply to everyone ",
        OutgoingKind::Forward { .. } => " Forward ",
    };
    let block = Block::bordered().title(title).border_style(theme.accent);
    let inner = block.inner(main);
    frame.render_widget(block, main);
    let dim = Style::new().add_modifier(Modifier::DIM);
    let notes = notes(compose, theme, dim);
    let headers = compose.headers();
    let header_rows = 1 + headers.len() + notes.len();
    let [header, rule, body] = Layout::vertical([
        Constraint::Length(u16::try_from(header_rows).unwrap_or(u16::MAX)),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(inner);
    let rows = Layout::vertical(vec![Constraint::Length(1); header_rows]).split(header);
    draw_field(
        frame,
        rows[0],
        Span::raw("From"),
        Span::styled(screen_line(&compose.from), dim),
        0,
    );
    for (note, row) in notes.into_iter().zip(rows.iter().skip(1 + headers.len())) {
        frame.render_widget(Paragraph::new(note), *row);
    }
    let mut at = None;
    for (field, row) in headers.iter().copied().zip(rows.iter().skip(1)) {
        let editor = compose.field(field);
        let focused = compose.focus == field;
        let name = if focused {
            Span::styled(field.label(), theme.accent.add_modifier(Modifier::BOLD))
        } else {
            Span::raw(field.label())
        };
        let place = draw_field(frame, *row, name, Span::raw(editor.text()), editor.column());
        if focused {
            at = Some(place);
        }
    }
    frame.render_widget(Block::new().borders(Borders::TOP).border_style(dim), rule);

    let width = usize::from(body.width.max(1));
    let height = usize::from(body.height);
    compose.body_width.set(body.width.max(1));
    compose.body_rows.set(body.height.max(1));
    let wrapped = compose.body.rows(width);
    let (row, column) = compose.body.cursor(&wrapped, width);
    let mut top = compose.top.get();
    if row < top {
        top = row;
    } else if height > 0 && row >= top + height {
        top = row + 1 - height;
    }
    compose.top.set(top);
    let lines: Vec<Line> = wrapped
        .iter()
        .skip(top)
        .take(height)
        .map(|row| Line::raw(compose.body.slice(row)))
        .collect();
    frame.render_widget(Paragraph::new(lines), body);
    if compose.focus == Field::Body && height > 0 {
        let x = body.x + u16::try_from(column).unwrap_or(u16::MAX);
        let y = body.y + u16::try_from(row - top).unwrap_or(u16::MAX);
        at = Some((x, y));
    }

    let said = match compose.said() {
        Some(said) => Line::raw(format!(" {}", screen_line(said))),
        None => Line::from(vec![
            Span::styled(" Ctrl+Enter", theme.accent),
            Span::raw(" or "),
            Span::styled("Ctrl+S", theme.accent),
            Span::raw(" send  "),
            Span::styled("Tab", theme.accent),
            Span::raw(" next field  "),
            Span::styled("Ctrl+A", theme.accent),
            Span::raw(" attach  "),
            Span::styled("Esc", theme.accent),
            Span::raw(" close"),
        ]),
    };
    frame.render_widget(Paragraph::new(said), foot);
    if let Some(files) = &compose.files {
        at = Some(draw_files(frame, compose, files, theme));
    }
    if let (true, Some(at)) = (cursor, at) {
        frame.set_cursor_position(at);
    }
}

/// The files the message carries, listed over it while Ctrl+A has them
/// open: each with its size, the chosen one inverted, the original's said
/// so, then the path being typed to attach one, the names it may go on as
/// and what went wrong. Returns where the cursor stands in the path.
fn draw_files(
    frame: &mut Frame,
    compose: &Compose,
    files: &super::compose::Files,
    theme: &Theme,
) -> (u16, u16) {
    let dim = Style::new().add_modifier(Modifier::DIM);
    let listed = compose.listed();
    let mut rows: Vec<Line> = listed
        .iter()
        .enumerate()
        .map(|(index, file)| {
            let from = match (file.original, file.left_out) {
                (true, false) => " · from the original",
                (true, true) => " · from the original, left out",
                (false, _) => "",
            };
            let style = if files.chosen == Some(index) {
                Style::new().add_modifier(Modifier::REVERSED)
            } else if file.left_out {
                dim
            } else {
                Style::new()
            };
            Line::styled(
                format!(" {} · {}{from}", file.name, size_label(file.size)),
                style,
            )
        })
        .collect();
    if rows.is_empty() {
        rows.push(Line::styled(" No files yet.", dim));
    }
    let matches = (!files.matches.is_empty()).then(|| {
        let names: Vec<&str> = files
            .matches
            .iter()
            .map(|path| {
                let at = path
                    .trim_end_matches(std::path::is_separator)
                    .rfind(std::path::is_separator)
                    .map_or(0, |at| at + 1);
                &path[at..]
            })
            .collect();
        Line::styled(screen_line(&format!("  {}", names.join("  "))), dim)
    });
    let notice = files
        .notice
        .as_ref()
        .map(|notice| Line::styled(screen_line(notice), theme.accent));
    let heights = [
        rows.len(),
        1,
        1,
        usize::from(matches.is_some()),
        usize::from(notice.is_some()),
    ];
    let inner_rows: usize = heights.iter().sum();
    let area = frame.area();
    let width = area.width.saturating_sub(4).min(100);
    let area = centered(
        area,
        width,
        u16::try_from(inner_rows + 2).unwrap_or(u16::MAX),
    );
    let block = Block::bordered()
        .title(" Attachments ")
        .title_bottom(" Enter attaches · Tab completes · Delete takes one away · Esc closes ")
        .border_style(theme.accent);
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    let [list, _, path, shown, said] = Layout::vertical(
        heights.map(|rows| Constraint::Length(u16::try_from(rows).unwrap_or(u16::MAX))),
    )
    .areas(inner);
    frame.render_widget(Paragraph::new(rows), list);
    if let Some(matches) = matches {
        frame.render_widget(Paragraph::new(matches), shown);
    }
    if let Some(notice) = notice {
        frame.render_widget(Paragraph::new(notice), said);
    }
    let name = if files.chosen.is_none() {
        Span::styled("Attach", theme.accent.add_modifier(Modifier::BOLD))
    } else {
        Span::raw("Attach")
    };
    draw_field(
        frame,
        path,
        name,
        Span::raw(files.path.text()),
        files.path.column(),
    )
}

/// What the editor says under the fields: for an answer, what it answers,
/// that the message asked for replies to go elsewhere, whether the
/// original goes under the text and which of its files a forward carries;
/// and which files of this computer go with the message.
fn notes(compose: &Compose, theme: &Theme, dim: Style) -> Vec<Line<'static>> {
    let attached = (!compose.attached.is_empty()).then(|| {
        let names: Vec<String> = compose
            .listed()
            .into_iter()
            .filter(|file| !file.original)
            .map(|file| format!("{} ({})", file.name, size_label(file.size)))
            .collect();
        Line::styled(format!("Attached: {}", names.join(", ")), dim)
    });
    let Some(answering) = &compose.answering else {
        return attached.into_iter().collect();
    };
    let mut notes = vec![Line::styled(
        format!(
            "Answering “{}” from {}.",
            answering.subject, answering.sender
        ),
        dim,
    )];
    if let (Some(sender), OutgoingKind::Reply { .. }) = (&answering.redirected_from, &compose.kind)
    {
        let asked = format!(
            "The message asks for replies to go to {}, not to its sender, {sender}.",
            answering.to.join(", ")
        );
        notes.push(Line::styled(screen_line(&asked), theme.accent));
    }
    if answering.opened {
        notes.push(Line::styled(
            match compose.kind.quote() {
                Quote::Original => "The original goes under your text; Ctrl+Q leaves it out.",
                Quote::Omitted => "The original is left out; Ctrl+Q puts it under your text.",
            },
            dim,
        ));
    }
    let forwarded: Vec<String> = compose
        .listed()
        .into_iter()
        .filter(|file| file.original && !file.left_out)
        .map(|file| file.name)
        .collect();
    if !forwarded.is_empty() {
        notes.push(Line::styled(
            format!("Forwarding: {}", forwarded.join(", ")),
            dim,
        ));
    }
    notes.extend(attached);
    notes
}

/// One field of the message on one row: its name, then what it holds,
/// scrolled sideways so that `column`, the cursor's, shows. Returns where
/// on screen that column is.
fn draw_field(frame: &mut Frame, area: Rect, name: Span, text: Span, column: usize) -> (u16, u16) {
    let [label, value] =
        Layout::horizontal([Constraint::Length(LABEL_WIDTH), Constraint::Fill(1)]).areas(area);
    frame.render_widget(Paragraph::new(name), label);
    let hidden = column.saturating_sub(usize::from(value.width.saturating_sub(1)));
    frame.render_widget(
        Paragraph::new(text).scroll((0, u16::try_from(hidden).unwrap_or(u16::MAX))),
        value,
    );
    let x = value
        .x
        .saturating_add(u16::try_from(column - hidden).unwrap_or(u16::MAX));
    (x, value.y)
}

/// The open conversation's attachments over the panes, with their sizes,
/// the selected one inverted.
fn draw_attachments(frame: &mut Frame, app: &App, selected: usize, theme: &Theme) {
    let Some(reading) = &app.reading else {
        return;
    };
    let area = frame.area();
    let width = area.width.saturating_sub(4).min(100);
    let items = reading.attachments.iter().map(|attachment| {
        ListItem::new(format!(
            " {} · {}",
            attachment.name,
            size_label(attachment.size)
        ))
    });
    let rows = u16::try_from(reading.attachments.len() + 2).unwrap_or(u16::MAX);
    let area = centered(area, width, rows);
    let block = Block::bordered()
        .title(" Attachments ")
        .title_bottom(" Enter saves one to Downloads · Esc closes ")
        .border_style(theme.accent);
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    let mut state = ListState::default().with_selected(Some(selected));
    frame.render_widget(Clear, area);
    frame.render_stateful_widget(list, area, &mut state);
}

/// The open conversation's links over the panes, the selected one inverted,
/// or the question whether to open it, with all of its address.
fn draw_links(frame: &mut Frame, app: &App, pick: LinkPick, theme: &Theme) {
    let Some(reading) = &app.reading else {
        return;
    };
    let area = frame.area();
    let width = area.width.saturating_sub(4).min(100);
    if pick.confirming {
        let url = reading.links.get(pick.selected).map_or("", String::as_str);
        let question = Paragraph::new(vec![
            Line::raw("Open this link in your browser?"),
            Line::default(),
            Line::styled(screen_line(url), theme.accent),
            Line::default(),
            Line::raw("y opens it; any other key goes back."),
        ])
        .wrap(Wrap { trim: false });
        let rows = question.line_count(width.saturating_sub(2)) + 2;
        let area = centered(area, width, u16::try_from(rows).unwrap_or(u16::MAX));
        let block = Block::bordered()
            .title(" Open link ")
            .border_style(theme.accent);
        frame.render_widget(Clear, area);
        frame.render_widget(question.block(block), area);
        return;
    }
    let items = reading
        .links
        .iter()
        .enumerate()
        .map(|(index, url)| ListItem::new(format!("{:>2}. {}", index + 1, screen_line(url))));
    let rows = u16::try_from(reading.links.len() + 2).unwrap_or(u16::MAX);
    let area = centered(area, width, rows);
    let block = Block::bordered()
        .title(" Links ")
        .title_bottom(" Enter opens one · Esc closes ")
        .border_style(theme.accent);
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    let mut state = ListState::default().with_selected(Some(pick.selected));
    frame.render_widget(Clear, area);
    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_pane(frame: &mut Frame, app: &App, theme: &Theme, pane: Pane, area: Rect) {
    let focused = pane == app.focus;
    let mut block = Block::bordered().title(format!(" {} ", pane.title()));
    if focused {
        block = block.border_style(theme.accent);
    }
    let inner = block.inner(area);
    // The first row of its list it shows.
    let first = match pane {
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
            draw_list(frame, items, app.selected_place, focused, block, area)
        }
        // Without this, the keys would seem to do nothing in an empty list.
        Pane::Conversations if app.listed_nothing() => {
            let hint = match &app.search {
                Some(search) if matches!(search.query, Query::Local(_)) => vec![
                    Line::raw(format!(
                        "Nothing in the local index matches {}.",
                        screen_line(&search.words)
                    )),
                    Line::raw("`ruston-cli index` puts mail in it."),
                    Line::raw(format!("Esc goes back to {}.", screen_line(&search.back.1))),
                ],
                Some(search) => vec![
                    Line::raw(format!("Nothing matches {}.", screen_line(&search.words))),
                    Line::raw(format!("Esc goes back to {}.", screen_line(&search.back.1))),
                ],
                None => vec![
                    Line::raw(format!("No conversations in {}.", screen_line(&app.listed))),
                    Line::raw("Esc goes back to the folders."),
                ],
            };
            let hint = Paragraph::new(hint)
                .style(Style::new().add_modifier(Modifier::DIM))
                .wrap(Wrap { trim: true });
            frame.render_widget(hint.block(block), area);
            0
        }
        Pane::Conversations => {
            let width = usize::from(block.inner(area).width);
            let items = app.conversations.iter().map(|conversation| {
                let marked = app.marked.contains(&conversation.id);
                let labels = app.row_labels(conversation);
                conversation_row(conversation, marked, &labels, width, theme)
            });
            // What the marks at the start of a row say, where they show.
            let legend = Line::styled(ROW_MARKS, Style::new().add_modifier(Modifier::DIM));
            draw_list(
                frame,
                items,
                app.selected_conversation,
                focused,
                block.title_bottom(legend),
                area,
            )
        }
        Pane::Reader => draw_reader(frame, app, block, area),
    };
    // For the mouse to find what it points at.
    let mut drawn = app.drawn.get();
    drawn.note(pane, inner, first);
    app.drawn.set(drawn);
}

/// A list with its selection on `selected`, shown inverted in the focused
/// pane, which reads without colors too, and underlined elsewhere: bold
/// says a conversation is unread. Returns the first row it shows.
fn draw_list<'a>(
    frame: &mut Frame,
    items: impl Iterator<Item = ListItem<'a>>,
    selected: usize,
    focused: bool,
    block: Block,
    area: Rect,
) -> usize {
    let highlight = if focused {
        Style::new().add_modifier(Modifier::REVERSED)
    } else {
        Style::new().add_modifier(Modifier::UNDERLINED)
    };
    let list = List::new(items).block(block).highlight_style(highlight);
    let mut state = ListState::default().with_selected(Some(selected));
    frame.render_stateful_widget(list, area, &mut state);
    state.offset()
}

/// The open conversation, the labels it bears under its subject, the
/// messages the state folds in a row each, a run of them without the
/// separators between, and the quoted passages of the rest in a line each,
/// unless the state shows them, wrapped to the pane and scrolled as far as
/// the state asks, within what there is. Since only the drawing knows how many
/// rows the text wraps to, it tells the state how far it can scroll, which
/// message and line are at the top and where each message starts, and brings
/// there the message the state asks for, or back the line `z` keeps. Returns
/// the first row it shows.
fn draw_reader(frame: &mut Frame, app: &App, mut block: Block, area: Rect) -> usize {
    let inner = block.inner(area);
    let Some(reading) = &app.reading else {
        app.reader_bottom.set(0);
        app.in_view.set(None);
        app.message_rows.borrow_mut().clear();
        let hint = Line::styled(
            "Enter on a conversation opens it here.",
            Style::new().add_modifier(Modifier::DIM),
        );
        frame.render_widget(Paragraph::new(hint).block(block), area);
        return 0;
    };
    // Each with the line of the conversation's text it shows, if any.
    let mut lines = vec![(
        None,
        Line::styled(
            reading.subject.as_str(),
            Style::new().add_modifier(Modifier::BOLD),
        ),
    )];
    // From the list, which loads again after each change, so they follow it.
    let labels = app
        .conversations
        .iter()
        .find(|conversation| conversation.id == reading.id)
        .map(|conversation| app.label_names(&[conversation]))
        .unwrap_or_default();
    if !labels.is_empty() {
        let named = format!("Labels: {}", labels.join(", "));
        lines.push((None, Line::raw(screen_line(&named))));
    }
    lines.push((None, Line::default()));
    let dim = Style::new().add_modifier(Modifier::DIM);
    let width = usize::from(inner.width);
    // Where each message starts in `lines`, with the separator before it
    // and without.
    let mut starts = Vec::new();
    let mut rest = 0;
    for (index, message) in reading.messages.iter().enumerate() {
        let folds = app.folds(index);
        let separator = lines.len();
        if !(folds && index > 0 && app.folds(index - 1)) {
            lines.extend(raw(reading, rest..message.lines.start));
        }
        starts.push((separator, lines.len()));
        if folds {
            let line = Line::styled(cut(&message.line, width), dim);
            lines.push((Some(message.lines.start), line));
        } else {
            unfolded(&mut lines, reading, message.lines.clone(), app.show_quotes);
        }
        rest = message.lines.end;
    }
    unfolded(
        &mut lines,
        reading,
        rest..reading.lines.len(),
        app.show_quotes,
    );
    // The rows the lines wrap to before each of them, and in all at the end.
    let mut row_of = vec![0];
    for (_, line) in &lines {
        let wrapped = Paragraph::new(line.clone()).wrap(Wrap { trim: false });
        row_of.push(row_of[row_of.len() - 1] + wrapped.line_count(inner.width));
    }
    let starts: Vec<(usize, usize)> = starts
        .into_iter()
        .map(|(separator, own)| (row_of[separator], row_of[own]))
        .collect();
    let rows = row_of[lines.len()];
    let height = usize::from(inner.height);
    let bottom = rows.saturating_sub(height);
    // As the desktop lands on a message: unless it starts in the upper third
    // already, where a short conversation keeps its subject in view.
    if let Some(&(_, start)) = app.landing.take().and_then(|index| starts.get(index)) {
        let scroll = usize::from(app.scroll.get());
        if start < scroll || start > scroll + height / 3 {
            app.scroll
                .set(u16::try_from(start.min(bottom)).unwrap_or(u16::MAX));
        }
    }
    // As `z` keeps the place: the line at the top back there, as far into it
    // as it was, or the folded passage it went into.
    let kept = app.keep_place.take().then(|| app.top_line.get()).flatten();
    if let Some((line, into)) = kept {
        // The last line drawn from the text up to it: itself, or a fold.
        let back = lines
            .iter()
            .rposition(|(shows, _)| shows.is_some_and(|shows| shows <= line));
        if let Some(at) = back {
            let into = if lines[at].0 == Some(line) { into } else { 0 };
            let scroll = (row_of[at] + into).min(bottom);
            app.scroll.set(u16::try_from(scroll).unwrap_or(u16::MAX));
        }
    }
    let top = usize::from(app.scroll.get()).min(bottom);
    let line_at_top = row_of.partition_point(|&row| row <= top) - 1;
    app.top_line.set(
        lines
            .get(line_at_top)
            .and_then(|(shows, _)| shows.map(|line| (line, top - row_of[line_at_top]))),
    );
    let at_top = starts.iter().rposition(|&(separator, _)| separator <= top);
    app.in_view
        .set(at_top.or((!starts.is_empty()).then_some(0)));
    *app.message_rows.borrow_mut() = starts.iter().map(|&(_, own)| own).collect();
    let bottom = u16::try_from(bottom).unwrap_or(u16::MAX);
    app.reader_bottom.set(bottom);
    app.reader_rows.set(inner.height.max(1));
    if (0..reading.messages.len()).any(|index| reading.folds(index)) {
        let hint = if app.show_read {
            " e folds the messages read "
        } else {
            " e opens the messages read "
        };
        block = block.title_bottom(Line::styled(hint, dim));
    }
    let lines: Vec<Line> = lines.into_iter().map(|(_, line)| line).collect();
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(
        paragraph
            .scroll((app.scroll.get().min(bottom), 0))
            .block(block),
        area,
    );
    top
}

/// Adds the lines `range` of `reading`, each quoted passage that starts
/// among them folded into a line unless `show_quotes`, each with the line of
/// the text it shows, a passage's first for its fold.
fn unfolded<'a>(
    lines: &mut Vec<(Option<usize>, Line<'a>)>,
    reading: &'a Reading,
    range: Range<usize>,
    show_quotes: bool,
) {
    let mut rest = range.start;
    if !show_quotes {
        for quote in reading.quotes.iter().filter(|q| range.contains(&q.start)) {
            lines.extend(raw(reading, rest..quote.start));
            let said = format!("… {} quoted lines (z shows them)", quote.len());
            let fold = Line::styled(said, Style::new().add_modifier(Modifier::DIM));
            lines.push((Some(quote.start), fold));
            rest = quote.end.min(range.end);
        }
    }
    lines.extend(raw(reading, rest..range.end));
}

/// The lines `range` of `reading`'s text as they are, each with where it is
/// in the text, a quoted one dimmed: wrapped, its later rows lose the `>`
/// that says it quotes.
fn raw(reading: &Reading, range: Range<usize>) -> impl Iterator<Item = (Option<usize>, Line<'_>)> {
    range.map(|at| {
        let line = &reading.lines[at];
        let line = if is_quoted(line) {
            Line::styled(line.as_str(), Style::new().add_modifier(Modifier::DIM))
        } else {
            Line::raw(line.as_str())
        };
        (Some(at), line)
    })
}

/// One row of the list, `width` columns wide: whether it is marked (✓),
/// unread (•), starred (★) or has attachments (+), its day, who wrote it,
/// its subject and the `labels` it bears. A narrow row, as beside the other
/// panes, leaves out the day's year and cuts the sender short, so the
/// subject keeps the largest share of it. The labels get what the subject
/// leaves of the row, and at least a third of what follows the sender, the
/// subject cut short for them; those that still do not fit are counted.
/// Unread conversations are bold.
fn conversation_row(
    conversation: &Conversation,
    marked: bool,
    labels: &[String],
    width: usize,
    theme: &Theme,
) -> ListItem<'static> {
    let unread = conversation.num_unread > 0;
    let starred = conversation
        .labels
        .iter()
        .any(|label| label.id == label_ids::STARRED);
    let flags = format!(
        "{}{}{}{}",
        if marked { "✓" } else { " " },
        if unread { "•" } else { " " },
        if starred { "★" } else { " " },
        if conversation.num_attachments > 0 {
            "+"
        } else {
            " "
        },
    );
    let time = render::fmt_time(conversation.time);
    // YYYY-MM-DD, or in a narrow row MM-DD.
    let day = if width >= YEAR_WIDTH { 0..10 } else { 5..10 };
    let day = time.get(day).unwrap_or(&time);
    let sender = conversation.senders.first().map_or("-", |sender| {
        if sender.name.is_empty() {
            &sender.address
        } else {
            &sender.name
        }
    });
    let sender_width = (width / 4).min(SENDER_WIDTH);
    let sender = cut(&screen_line(sender), sender_width);
    let padding = " ".repeat(sender_width.saturating_sub(sender.width()));
    let style = if unread {
        Style::new().add_modifier(Modifier::BOLD)
    } else {
        Style::new()
    };
    let mut spans = vec![
        Span::raw(format!("{flags} {day}  ")),
        Span::styled(format!("{sender}{padding}  "), style),
    ];
    let subject = screen_line(render::subject_or_empty(&conversation.subject));
    let room = width.saturating_sub(spans.iter().map(Span::width).sum());
    let left = room.saturating_sub(subject.width() + 1);
    let tags = tags(labels, left.max(room / 3));
    if tags.is_empty() {
        spans.push(Span::styled(subject, style));
    } else {
        let subject = cut(&subject, room.saturating_sub(tags.width() + 1));
        spans.extend([
            Span::styled(subject, style),
            Span::raw(" "),
            Span::styled(tags, theme.label),
        ]);
    }
    ListItem::new(Line::from(spans))
}

/// The labels `names` as a row of the list shows them, in at most `room`
/// columns: each in brackets, as many as fit with the count of the rest, or
/// none when not even the first does.
fn tags(names: &[String], room: usize) -> String {
    (1..=names.len())
        .rev()
        .map(|shown| {
            let mut tags: Vec<String> = names[..shown]
                .iter()
                .map(|name| format!("[{}]", screen_line(name)))
                .collect();
            if shown < names.len() {
                tags.push(format!("+{}", names.len() - shown));
            }
            tags.join(" ")
        })
        .find(|tags| tags.width() <= room)
        .unwrap_or_default()
}

/// `text` in at most `columns` columns: cut short with … between whole
/// characters, as the terminal draws them, when it is longer.
fn cut(text: &str, columns: usize) -> String {
    if text.width() <= columns {
        return text.to_owned();
    }
    let mut kept = String::new();
    let mut used = 0;
    for character in text.graphemes(true) {
        used += character.width();
        // One column is for the …
        if used >= columns {
            break;
        }
        kept.push_str(character);
    }
    if columns > 0 {
        kept.push('…');
    }
    kept
}

fn draw_status(frame: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    if let Some(line) = &app.command_line {
        draw_command_line(frame, &line.text, area);
        return;
    }
    if app.going {
        draw_going(frame, theme, area);
        return;
    }
    let mut hints = Vec::new();
    // Only while there is a move to take back.
    if app.undo.is_some() {
        hints.extend([Span::styled("Z", theme.accent), Span::raw(" undo  ")]);
    }
    hints.extend([
        Span::styled("Tab", theme.accent),
        Span::raw(" panes  "),
        Span::styled("?", theme.accent),
        Span::raw(" keys  "),
        Span::styled("q", theme.accent),
        Span::raw(" quit "),
    ]);
    let hints = Line::from(hints);
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
    let detail = match app.marked.len() {
        0 => detail,
        marked => format!("{marked} marked · {detail}"),
    };
    let text = format!(" {} · {} · {detail}", app.account, app.listed);
    frame.render_widget(Paragraph::new(screen_line(&text)), state);
    frame.render_widget(Paragraph::new(hints), hints_area);
}

/// The command line in place of the status bar, with the cursor after what
/// is typed; a line longer than the bar shows its end.
fn draw_command_line(frame: &mut Frame, text: &str, area: Rect) {
    let line = Line::raw(format!(":{}", screen_line(text)));
    let width = u16::try_from(line.width()).unwrap_or(u16::MAX);
    let hidden = width.saturating_sub(area.width.saturating_sub(1));
    frame.render_widget(Paragraph::new(line).scroll((0, hidden)), area);
    frame.set_cursor_position((area.x + width - hidden, area.y));
}

/// The folders `g` goes to, each after the key that names it, in place of
/// the status bar until that key comes.
fn draw_going(frame: &mut Frame, theme: &Theme, area: Rect) {
    let spans: Vec<Span> = GO_TO
        .iter()
        .flat_map(|(key, _, name)| {
            [
                Span::styled(format!(" {key}"), theme.accent),
                Span::raw(format!(" {name} ")),
            ]
        })
        .collect();
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// The keys over the panes: in two columns side by side where the terminal
/// is wide enough, the first half of them in the first, and scrolled as far
/// as the state asks where they do not all fit, which the view tells the
/// state as it does for the reader.
fn draw_keys(frame: &mut Frame, app: &App, theme: &Theme) {
    let keys: Vec<&(&str, &str)> = KEYS.iter().chain(SUSPEND_KEYS).collect();
    let column = keys
        .iter()
        .map(|(keys, _)| keys.chars().count())
        .max()
        .unwrap_or(0);
    let lines: Vec<Line> = keys
        .iter()
        .map(|(keys, action)| {
            Line::from(vec![
                Span::styled(format!(" {keys:<column$} "), theme.accent),
                Span::raw(*action),
            ])
        })
        .collect();
    // A column is as wide as its widest line and a space; the border takes
    // two more.
    let width = lines.iter().map(Line::width).max().unwrap_or(0) + 1;
    let columns = if 2 * width + 2 <= usize::from(frame.area().width) {
        2
    } else {
        1
    };
    let rows = lines.len().div_ceil(columns);
    let area = centered(
        frame.area(),
        u16::try_from(columns * width + 2).unwrap_or(u16::MAX),
        u16::try_from(rows + 2).unwrap_or(u16::MAX),
    );
    let dim = Style::new().add_modifier(Modifier::DIM);
    let mut block = Block::bordered()
        .title(" Keys ")
        .title_bottom(Line::styled(SEARCH_FILTERS, dim))
        .border_style(theme.accent);
    let shown = block.inner(area).height;
    let bottom = u16::try_from(rows)
        .unwrap_or(u16::MAX)
        .saturating_sub(shown);
    app.help_bottom.set(bottom);
    app.help_rows.set(shown.max(1));
    if bottom > 0 {
        block = block.title_bottom(Line::styled(" j, k scroll ", dim).right_aligned());
    }
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    let each = Constraint::Length(u16::try_from(width).unwrap_or(u16::MAX));
    let halves = Layout::horizontal(vec![each; columns]).split(inner);
    for (half, place) in lines.chunks(rows).zip(halves.iter()) {
        let half = Paragraph::new(half.to_vec()).scroll((app.help_scroll.min(bottom), 0));
        frame.render_widget(half, *place);
    }
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
mod tests;
