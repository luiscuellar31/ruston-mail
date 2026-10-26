//! Text typed in the terminal interface, as the message being written holds
//! it: its lines, a cursor between whole characters, and the rows the lines
//! wrap to at a width. The view draws those rows and puts the cursor where
//! [`Editor::cursor`] says, so what is typed and what is drawn agree. Widths
//! are the columns a terminal gives each character, two for most Chinese
//! and Japanese ones, and the cursor never stops inside a character made of
//! several, such as a letter and its accent.

use crate::terminal::is_filtered;
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Text being typed, a line at a time, with the cursor's place in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Editor {
    /// Never empty: an empty text is one empty line.
    lines: Vec<String>,
    /// The cursor's line.
    line: usize,
    /// Its byte offset in that line, between two whole characters.
    at: usize,
    /// The column Up and Down keep to across rows shorter than it.
    goal: Option<usize>,
}

/// One row of the text as wrapped: its line and the part of it shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Row {
    pub(crate) line: usize,
    pub(crate) range: Range<usize>,
    /// Whether it ends its line, rather than going on in the next row.
    pub(crate) ends_line: bool,
}

impl Default for Editor {
    fn default() -> Self {
        Self {
            lines: vec![String::new()],
            line: 0,
            at: 0,
            goal: None,
        }
    }
}

impl Editor {
    /// A text that starts as `text`, the cursor at its end; what typing
    /// would not take is left out.
    pub(crate) fn with(text: &str) -> Self {
        let mut editor = Self::default();
        for c in text.chars() {
            if c == '\n' {
                editor.break_line();
            } else {
                editor.insert(c);
            }
        }
        editor
    }

    /// The text, its lines joined by line breaks.
    pub(crate) fn text(&self) -> String {
        self.lines.join("\n")
    }

    /// Whether only blanks were typed.
    pub(crate) fn is_blank(&self) -> bool {
        self.lines.iter().all(|line| line.trim().is_empty())
    }

    /// The part of the text `row` shows.
    pub(crate) fn slice(&self, row: &Row) -> &str {
        &self.lines[row.line][row.range.clone()]
    }

    /// The cursor's column in its line, unwrapped: where it stands in a
    /// field of one line.
    pub(crate) fn column(&self) -> usize {
        self.lines[self.line][..self.at].width()
    }

    /// Types `c` where the cursor is. What output filtering drops, such as
    /// a control or a mark that reorders text, is not taken, nor a tab or a
    /// line break, which keys of their own make.
    pub(crate) fn insert(&mut self, c: char) {
        if matches!(c, '\n' | '\t') || is_filtered(c) {
            return;
        }
        self.lines[self.line].insert(self.at, c);
        self.at += c.len_utf8();
        self.goal = None;
    }

    /// Breaks the line at the cursor, which goes to the start of the new
    /// one.
    pub(crate) fn break_line(&mut self) {
        let rest = self.lines[self.line].split_off(self.at);
        self.line += 1;
        self.lines.insert(self.line, rest);
        self.at = 0;
        self.goal = None;
    }

    /// Takes away the character before the cursor, or at the start of a
    /// line, the break before it.
    pub(crate) fn backspace(&mut self) {
        if self.at > 0 {
            let start = boundary_before(&self.lines[self.line], self.at);
            self.lines[self.line].replace_range(start..self.at, "");
            self.at = start;
        } else if self.line > 0 {
            let joined = self.lines.remove(self.line);
            self.line -= 1;
            self.at = self.lines[self.line].len();
            self.lines[self.line].push_str(&joined);
        }
        self.goal = None;
    }

    /// Takes away the character after the cursor, or at the end of a line,
    /// the break after it.
    pub(crate) fn delete(&mut self) {
        if self.at < self.lines[self.line].len() {
            let end = boundary_after(&self.lines[self.line], self.at);
            self.lines[self.line].replace_range(self.at..end, "");
        } else if self.line + 1 < self.lines.len() {
            let joined = self.lines.remove(self.line + 1);
            self.lines[self.line].push_str(&joined);
        }
        self.goal = None;
    }

    pub(crate) fn left(&mut self) {
        if self.at > 0 {
            self.at = boundary_before(&self.lines[self.line], self.at);
        } else if self.line > 0 {
            self.line -= 1;
            self.at = self.lines[self.line].len();
        }
        self.goal = None;
    }

    pub(crate) fn right(&mut self) {
        if self.at < self.lines[self.line].len() {
            self.at = boundary_after(&self.lines[self.line], self.at);
        } else if self.line + 1 < self.lines.len() {
            self.line += 1;
            self.at = 0;
        }
        self.goal = None;
    }

    /// To the start of the line, or with `whole`, of the text.
    pub(crate) fn home(&mut self, whole: bool) {
        if whole {
            self.line = 0;
        }
        self.at = 0;
        self.goal = None;
    }

    /// To the end of the line, or with `whole`, of the text.
    pub(crate) fn end(&mut self, whole: bool) {
        if whole {
            self.line = self.lines.len() - 1;
        }
        self.at = self.lines[self.line].len();
        self.goal = None;
    }

    /// The rows the text wraps to at `width` columns, in order.
    pub(crate) fn rows(&self, width: usize) -> Vec<Row> {
        let mut rows = Vec::new();
        for (line, text) in self.lines.iter().enumerate() {
            let wrapped = wrap(text, width);
            let last = wrapped.len() - 1;
            rows.extend(wrapped.into_iter().enumerate().map(|(index, range)| Row {
                line,
                range,
                ends_line: index == last,
            }));
        }
        rows
    }

    /// Where the cursor is drawn among `rows`, wrapped at `width`: which row
    /// and which column. Past a row's last column it is drawn at the start
    /// of the row after, where the next character goes, which may be a row
    /// the text has not reached yet.
    pub(crate) fn cursor(&self, rows: &[Row], width: usize) -> (usize, usize) {
        let (index, column) = self.place(rows);
        if column >= width.max(1) {
            (index + 1, 0)
        } else {
            (index, column)
        }
    }

    /// Moves `by` rows of the text wrapped at `width`, up when negative, as
    /// far as there are, keeping to the column it moved from; false when
    /// there was no row to move to.
    pub(crate) fn vertical(&mut self, width: usize, by: isize) -> bool {
        let rows = self.rows(width);
        let (index, column) = self.place(&rows);
        let target = index.saturating_add_signed(by).min(rows.len() - 1);
        if target == index {
            return false;
        }
        let goal = *self.goal.get_or_insert(column);
        let row = &rows[target];
        self.line = row.line;
        self.at = nearest(&self.lines[row.line], row, goal);
        true
    }

    /// The row the cursor is in, and its column there.
    fn place(&self, rows: &[Row]) -> (usize, usize) {
        let index = rows
            .iter()
            .position(|row| row.line == self.line && (self.at < row.range.end || row.ends_line))
            .unwrap_or(0);
        let start = rows
            .get(index)
            .map_or(0, |row| row.range.start.min(self.at));
        (index, self.lines[self.line][start..self.at].width())
    }
}

/// Where in `row` of `text` the cursor goes for the column `goal`: as near
/// as it gets without passing it. A row that goes on in the next one keeps
/// the cursor before its last character, since its end is where the next
/// row starts.
fn nearest(text: &str, row: &Row, goal: usize) -> usize {
    let mut column = 0;
    let mut last = row.range.start;
    for (offset, grapheme) in text[row.range.clone()].grapheme_indices(true) {
        let at = row.range.start + offset;
        let width = grapheme.width();
        if column + width > goal {
            return at;
        }
        column += width;
        last = at;
    }
    if row.ends_line { row.range.end } else { last }
}

/// The start of the character that ends at `at` in `text`.
fn boundary_before(text: &str, at: usize) -> usize {
    text[..at]
        .grapheme_indices(true)
        .next_back()
        .map_or(0, |(start, _)| start)
}

/// The end of the character that starts at `at` in `text`.
fn boundary_after(text: &str, at: usize) -> usize {
    text[at..]
        .graphemes(true)
        .next()
        .map_or(at, |grapheme| at + grapheme.len())
}

/// Where `line` breaks into rows of at most `width` columns: after the last
/// space that fits, or inside a word too long for a row. A row never starts
/// with a space: spaces past the width stay at the end of their row, where
/// nothing shows them.
fn wrap(line: &str, width: usize) -> Vec<Range<usize>> {
    let width = width.max(1);
    let mut rows = Vec::new();
    let (mut start, mut used) = (0, 0);
    // Where the row's last space ends, to break there.
    let mut after_space = None;
    for (at, grapheme) in line.grapheme_indices(true) {
        if grapheme == " " {
            used += 1;
            after_space = Some(at + 1);
            continue;
        }
        let wide = grapheme.width();
        if used + wide > width && at > start {
            let end = after_space.unwrap_or(at);
            rows.push(start..end);
            start = end;
            used = line[start..at].width();
            after_space = None;
            // What was carried over is still too wide: a word longer than
            // a row breaks where the row is full.
            if used + wide > width && at > start {
                rows.push(start..at);
                start = at;
                used = 0;
            }
        }
        used += wide;
    }
    rows.push(start..line.len());
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(text: &str) -> Editor {
        Editor::with(text)
    }

    /// The rows `editor` wraps to at `width`, as text.
    fn shown(editor: &Editor, width: usize) -> Vec<String> {
        editor
            .rows(width)
            .iter()
            .map(|row| editor.slice(row).to_owned())
            .collect()
    }

    #[test]
    fn lines_wrap_after_a_space_and_long_words_where_the_row_is_full() {
        let editor = typed("Hi Alex, the report is here.\n\nunbreakablewordhere");
        assert_eq!(
            shown(&editor, 10),
            [
                "Hi Alex, ",
                "the report ",
                "is here.",
                "",
                "unbreakabl",
                "ewordhere"
            ]
        );
        // Spaces past the width stay at the end of their row.
        assert_eq!(shown(&typed("abcd   efg"), 4), ["abcd   ", "efg"]);
        // A word that ends right at the width fits.
        assert_eq!(shown(&typed("abcd efgh"), 4), ["abcd ", "efgh"]);
        assert_eq!(shown(&Editor::default(), 4), [""]);
    }

    #[test]
    fn wide_characters_take_two_columns_and_are_never_split() {
        // Each takes two columns.
        let editor = typed("日本語のテキスト");
        assert_eq!(shown(&editor, 5), ["日本", "語の", "テキ", "スト"]);
        // A letter with a combining accent is one character.
        let mut accented = typed("cafe\u{301} ok");
        assert_eq!(shown(&accented, 4), ["cafe\u{301} ", "ok"]);
        accented.end(false);
        accented.left();
        accented.left();
        accented.left();
        accented.backspace();
        assert_eq!(accented.text(), "caf ok");
        // Even a row narrower than one of them takes it whole.
        assert_eq!(shown(&typed("日本"), 1), ["日", "本"]);
    }

    #[test]
    fn typing_breaking_and_deleting_go_across_lines() {
        let mut editor = typed("one two");
        editor.home(false);
        editor.right();
        editor.right();
        editor.right();
        editor.break_line();
        assert_eq!(editor.text(), "one\n two");
        editor.backspace();
        assert_eq!(editor.text(), "one two");
        editor.end(false);
        editor.delete();
        assert_eq!(editor.text(), "one two");
        editor.home(false);
        editor.delete();
        assert_eq!(editor.text(), "ne two");
        // Controls and marks that reorder text are not taken.
        editor.insert('\u{1b}');
        editor.insert('\u{202e}');
        editor.insert('\t');
        assert_eq!(editor.text(), "ne two");
        assert!(!editor.is_blank());
        assert!(typed("  \n ").is_blank());
    }

    #[test]
    fn the_cursor_is_drawn_where_the_next_character_goes() {
        let mut editor = typed("abcd efgh");
        let rows = editor.rows(4);
        // At the end of a full last row, at the start of the row after.
        assert_eq!(editor.cursor(&rows, 4), (2, 0));
        editor.home(false);
        assert_eq!(editor.cursor(&rows, 4), (0, 0));
        for _ in 0..5 {
            editor.right();
        }
        // After the space that ends a row, at the start of the next.
        assert_eq!(editor.cursor(&rows, 4), (1, 0));
        editor.right();
        assert_eq!(editor.cursor(&rows, 4), (1, 1));
        let wide = typed("日本");
        assert_eq!(wide.cursor(&wide.rows(10), 10), (0, 4));
    }

    #[test]
    fn up_and_down_move_by_rows_keeping_to_their_column() {
        let mut editor = typed("first line here\nab\nthird line");
        // "third line" at width 20 is the last row; the cursor is at its end.
        assert!(editor.vertical(20, -1));
        // "ab" is shorter: the cursor stops at its end.
        assert_eq!(editor.cursor(&editor.rows(20), 20), (1, 2));
        assert!(editor.vertical(20, -1));
        // Back on a long row, it returns to the column it left.
        assert_eq!(editor.cursor(&editor.rows(20), 20), (0, 10));
        assert!(!editor.vertical(20, -1));
        // Wrapped rows count as rows.
        let mut long = typed("abcd efgh ijkl");
        long.home(true);
        assert!(long.vertical(5, 1));
        assert_eq!(long.cursor(&long.rows(5), 5), (1, 0));
        assert!(long.vertical(5, 9));
        assert_eq!(long.cursor(&long.rows(5), 5), (2, 0));
        assert!(!long.vertical(5, 1));
    }

    #[test]
    fn home_and_end_go_to_the_ends_of_the_line_or_of_the_text() {
        let mut editor = typed("one\ntwo\nthree");
        editor.home(false);
        assert_eq!(editor.column(), 0);
        editor.end(false);
        assert_eq!(editor.column(), 5);
        editor.home(true);
        assert_eq!(editor.cursor(&editor.rows(80), 80), (0, 0));
        editor.end(true);
        assert_eq!(editor.cursor(&editor.rows(80), 80), (2, 5));
        // Left at the start of a line goes to the end of the one before.
        editor.home(false);
        editor.left();
        assert_eq!(editor.cursor(&editor.rows(80), 80), (1, 3));
    }
}
