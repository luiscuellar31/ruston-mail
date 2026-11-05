//! How the interface looks: the three panes side by side, or the focused one
//! alone in a narrow terminal, a status bar, and the keys over them on `?`.
//! It draws from the state alone, so it can be tested on a `TestBackend`.
//! Every piece of text that comes with the mail goes through
//! `terminal::screen_line` first.

use super::app::{App, GO_TO, LinkPick, Pane, Query, Question};
use super::compose::{Checked, Compose, Field};
use super::reading::{is_quoted, size_label};
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

/// How many columns a conversation's sender takes in its row.
const SENDER_WIDTH: usize = 18;

/// How many conversations a question names before it counts the rest.
const NAMED: usize = 5;

/// The width of a field's name, before what it holds.
const LABEL_WIDTH: u16 = 9;

/// Each key and what it does, as the list of keys shows them: Proton Mail's
/// where it has one, and keys it leaves free for what only a terminal needs.
const KEYS: [(&str, &str); 21] = [
    ("Tab, Shift+Tab", "next or previous pane"),
    ("j, k, Up, Down", "move, or scroll the reader"),
    ("Enter, Right, Left", "open it, or go back a pane"),
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
        // Without this, the keys would seem to do nothing in an empty list.
        Pane::Conversations if app.listed_nothing() => {
            let hint = match &app.search {
                Some(search) if search.query == Query::Local => vec![
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
            );
        }
        Pane::Reader => draw_reader(frame, app, block, area),
    }
}

/// A list with its selection on `selected`, shown inverted in the focused
/// pane, which reads without colors too, and underlined elsewhere: bold
/// says a conversation is unread.
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
        Style::new().add_modifier(Modifier::UNDERLINED)
    };
    let list = List::new(items).block(block).highlight_style(highlight);
    let mut state = ListState::default().with_selected(Some(selected));
    frame.render_stateful_widget(list, area, &mut state);
}

/// The open conversation, the labels it bears under its subject, its
/// messages already read but the newest folded into a row each, and its
/// other quoted passages into a line each, unless the state shows them,
/// wrapped to the pane and scrolled as far as
/// the state asks, within what there is. Since only the drawing knows how many
/// rows the text wraps to, it tells the state how far it can scroll.
fn draw_reader(frame: &mut Frame, app: &App, mut block: Block, area: Rect) {
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
    let mut lines = vec![Line::styled(
        reading.subject.as_str(),
        Style::new().add_modifier(Modifier::BOLD),
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
        lines.push(Line::raw(screen_line(&named)));
    }
    lines.push(Line::default());
    let dim = Style::new().add_modifier(Modifier::DIM);
    // What folds, each with the line that stands for it: the messages read,
    // a row each, then the quoted passages of the rest.
    let mut folds: Vec<(Range<usize>, Line)> = Vec::new();
    if !app.show_read {
        let width = usize::from(inner.width);
        folds.extend(reading.folded.iter().map(|message| {
            let line = Line::styled(cut(&message.line, width), dim);
            (message.lines.clone(), line)
        }));
    }
    if !app.show_quotes {
        let quotes: Vec<(Range<usize>, Line)> = reading
            .quotes
            .iter()
            .filter(|quote| {
                !folds
                    .iter()
                    .any(|(message, _)| message.contains(&quote.start))
            })
            .map(|quote| {
                let said = format!("… {} quoted lines (z shows them)", quote.len());
                (quote.clone(), Line::styled(said, dim))
            })
            .collect();
        folds.extend(quotes);
    }
    folds.sort_by_key(|(lines, _)| lines.start);
    let mut rest = 0;
    for (folded, line) in folds {
        lines.extend(raw(&reading.lines[rest..folded.start]));
        lines.push(line);
        rest = folded.end;
    }
    lines.extend(raw(&reading.lines[rest..]));
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    let rows = paragraph.line_count(inner.width);
    let bottom = u16::try_from(rows.saturating_sub(usize::from(inner.height))).unwrap_or(u16::MAX);
    app.reader_bottom.set(bottom);
    app.reader_rows.set(inner.height.max(1));
    if !reading.folded.is_empty() {
        let hint = if app.show_read {
            " e folds the messages read "
        } else {
            " e opens the messages read "
        };
        block = block.title_bottom(Line::styled(hint, dim));
    }
    frame.render_widget(
        paragraph.scroll((app.scroll.min(bottom), 0)).block(block),
        area,
    );
}

/// Each of `lines` as it is, a quoted one dimmed: wrapped, its later rows
/// lose the `>` that says it quotes.
fn raw(lines: &[String]) -> impl Iterator<Item = Line<'_>> {
    lines.iter().map(|line| {
        if is_quoted(line) {
            Line::styled(line.as_str(), Style::new().add_modifier(Modifier::DIM))
        } else {
            Line::raw(line.as_str())
        }
    })
}

/// One row of the list, `width` columns wide: whether it is marked (✓),
/// unread (•), starred (★) or has attachments (+), its day, who wrote it,
/// its subject and the `labels` it bears. The labels get what the subject
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
    let mut spans = vec![
        Span::raw(format!("{flags} {day}  ")),
        Span::styled(
            format!("{:<SENDER_WIDTH$.SENDER_WIDTH$}  ", screen_line(sender)),
            style,
        ),
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
mod tests {
    use super::super::app::Message;
    use super::super::compose::AnswerKind;
    use super::super::reading::Reading;
    use super::*;
    use crate::mailbox::{Outgoing, Place, PlaceKind};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::Backend;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ruston_core::Recipient;
    use ruston_core::model::ConversationLabel;

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
            screen.contains(" •   2024-04-24  alex@example.test   Quarterly│"),
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
        assert!(
            screen.contains("✓•   2024-04-24  alex@example.test"),
            "{screen}"
        );
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
        assert!(
            screen.contains(" ★+ 2024-04-24  alex@example.test"),
            "{screen}"
        );
    }

    /// One of the account's labels.
    fn label(id: &str, name: &str) -> Place {
        Place {
            id: id.into(),
            name: name.into(),
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
        let screen = text(&drawn(120, 30, &app, &Theme::new(false)));
        assert!(
            screen.contains(" •   2024-04-24  alex@example.test   Quarterly│"),
            "{screen}"
        );
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
            folded: Vec::new(),
            answer: None,
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
    fn the_reader_names_the_labels_the_conversation_bears() {
        let mut app = with_mail("alex@example.test", "Quarterly report");
        let label = |id: &str, name: &str| Place {
            id: id.into(),
            name: name.into(),
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
            folded: Vec::new(),
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
            folded: Vec::new(),
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
    fn the_reader_folds_the_messages_read_but_the_newest_until_e_shows_them() {
        use ruston_core::{ConversationMessage, FullMessage, MessageMetadata, Verdict};
        let message = |sender: &str, hours: i64, unread: i64, body: &str| {
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
        };
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

        let e = crossterm::event::Event::Key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE));
        if let Some(message) = app.message_for(&e) {
            app.update(message);
        }
        let screen = text(&drawn(200, 40, &app, &Theme::new(false)));
        assert!(screen.contains("Date:    2024-04-24 23:06"), "{screen}");
        assert!(screen.contains("Second thoughts"), "{screen}");
        assert!(!screen.contains('▸'), "{screen}");
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
            folded: Vec::new(),
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
            folded: Vec::new(),
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
            folded: Vec::new(),
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
            folded: Vec::new(),
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
        let selected = style_of("sam@example.test");
        assert!(selected.contains(Modifier::UNDERLINED) && !selected.contains(Modifier::BOLD));
        let unread = style_of("alex@example.test");
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
            // With what `:search` takes beside its words.
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
}
