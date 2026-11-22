//! The readable structure of a sanitized HTML body, which the desktop and the
//! terminal both draw: blocks of styled text in reading order. Images are
//! never loaded, only links to the web or to an address stay clickable, and
//! nothing a sender hides in `head`, `script` or `style` is read.

use std::cell::RefCell;
use std::sync::Arc;

use html5ever::tendril::StrTendril;
use html5ever::tokenizer::states::RawKind;
use html5ever::tokenizer::{
    BufferQueue, Tag, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer, TokenizerOpts,
};

use super::{BLOCK_ELEMENTS, HIDDEN_ELEMENTS, Skipping, VOID_ELEMENTS, declarations, hides};

/// The most lists kept open within one another. A deeper one reads as the
/// deepest kept, so a hostile body cannot grow the stack without end.
pub const MAX_LIST_DEPTH: usize = 8;
/// How much input the tokenizer reads at a time, so a body cut short is not
/// read to its end.
const INPUT_CHUNK_BYTES: usize = 8 * 1024;
/// The most elements whose style is followed within one another; a deeper
/// one reads as if it had none.
const MAX_STYLE_DEPTH: usize = 64;
/// The most tables kept open within one another; a deeper one reads as the
/// paragraphs of its rows.
const MAX_TABLE_DEPTH: usize = 32;
/// The longest text a link drawn as a button may hold; a longer one is a
/// passage on a colored ground.
const MAX_BUTTON_CHARS: usize = 60;
/// The longest a cell of a table of data may be. A longer one is a page laid
/// out in columns, which reads better as paragraphs.
const MAX_CELL_CHARS: usize = 120;
/// What a block takes besides its text. Text always leaves room for one, so
/// a body cut short still ends with the block it was reading.
const BLOCK_SIZE: usize = std::mem::size_of::<RichBlock>();

/// The readable structure of an HTML body, in reading order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RichBody {
    /// The blocks, in reading order.
    pub blocks: Vec<RichBlock>,
    /// Whether the body was cut short to stay within the size asked for.
    pub truncated: bool,
    /// Images with no description, which no block stands for. Tracking
    /// pixels, one pixel wide or tall, and images the sender hid are not
    /// counted: they were never meant to be seen.
    pub undescribed_images: usize,
}

impl RichBody {
    /// The body as plain text, one block per line. Styles and links are
    /// dropped; structure is kept with markers and quote prefixes.
    pub fn plain_text(&self) -> String {
        let mut out = String::new();
        for line in self.plain_lines() {
            out.push_str(&line);
            out.push('\n');
        }
        out
    }

    /// The lines of [`Self::plain_text`], each block worked out only once
    /// the lines before it are read, so taking the first few is cheap
    /// however long the body is. Text the sender hid is left out, as a
    /// reader that hides it shows the body.
    pub fn plain_lines(&self) -> impl Iterator<Item = String> + '_ {
        self.blocks
            .iter()
            .filter(|block| !block.hidden)
            .flat_map(|block| {
                let text = match &block.kind {
                    BlockKind::Paragraph(spans) | BlockKind::Heading { spans, .. } => join(spans),
                    BlockKind::ListItem {
                        marker: ListMarker::Continuation,
                        spans,
                        ..
                    } => join(spans),
                    BlockKind::ListItem { marker, spans, .. } => {
                        format!("{} {}", marker.label(), join(spans))
                    }
                    BlockKind::Preformatted(text) => text.clone(),
                    BlockKind::Image { description, .. } if description.is_empty() => String::new(),
                    BlockKind::Image { description, .. } => format!("[Image: {description}]"),
                    BlockKind::Rule => "---".to_owned(),
                    // A row to a line, its cells apart as words are.
                    BlockKind::Table(rows) => rows
                        .iter()
                        .map(|row| {
                            let cells: Vec<String> =
                                row.cells.iter().map(|cell| join(cell)).collect();
                            cells.join(" ")
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                };
                let quote = "> ".repeat(usize::from(block.quote_depth));
                text.lines()
                    .map(|line| format!("{quote}{line}"))
                    .collect::<Vec<_>>()
            })
    }
}

/// The text of `spans` the sender did not hide; a space hidden text left at
/// the end goes too.
fn join(spans: &[RichSpan]) -> String {
    let text: String = spans
        .iter()
        .filter(|span| !span.hidden)
        .map(|span| span.text.as_str())
        .collect();
    text.trim_end().to_owned()
}

/// One block of a body, as a paragraph or a list item is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RichBlock {
    /// What the block is, with its content.
    pub kind: BlockKind,
    /// How deeply the block sits inside quotes.
    pub quote_depth: u8,
    /// Whether the sender hid all of it, as a newsletter's preview text is
    /// hidden; a reader may leave it out, or show it set apart.
    pub hidden: bool,
    /// Whether the sender's style, or a `center` element, centers it.
    pub centered: bool,
}

/// What a block is, with its content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockKind {
    /// Text that wraps, in runs of one style each.
    Paragraph(Vec<RichSpan>),
    /// A heading.
    Heading {
        /// Its level, from 1 for the largest to 6.
        level: u8,
        /// Its text.
        spans: Vec<RichSpan>,
    },
    /// A block of a list item.
    ListItem {
        /// What the block starts with.
        marker: ListMarker,
        /// How many lists deep the item sits, from 1 to [`MAX_LIST_DEPTH`].
        depth: u8,
        /// Its text.
        spans: Vec<RichSpan>,
    },
    /// Text whose spacing matters, shown in a monospace font.
    Preformatted(String),
    /// An image. One on the web is never loaded, and only its description
    /// shows; one that travels in the message may be shown.
    Image {
        /// What the sender wrote to describe it, if anything.
        description: String,
        /// The part of the message it is, named as that part's
        /// `Content-ID`, when it travels in the message.
        content_id: Option<String>,
    },
    /// A line across, between two parts of the body.
    Rule,
    /// A table of data, as a receipt's lines or a timetable are. A table
    /// that only lays a page out reads as the paragraphs of its rows.
    Table(Vec<TableRow>),
}

/// A row of a table of data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableRow {
    /// Its cells, in order, each a run of styled text.
    pub cells: Vec<Vec<RichSpan>>,
    /// Whether all its cells are headings.
    pub header: bool,
}

/// What a block of a list item starts with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListMarker {
    /// The first block of a bulleted list's item.
    Bullet,
    /// The first block of an ordered list's item, with its number.
    Number(u32),
    /// A later block of the same item, which keeps its indent alone.
    Continuation,
}

impl ListMarker {
    /// The marker as the desktop shows it, and plain text writes it: a
    /// bullet, a number with its period, or nothing.
    pub fn label(self) -> String {
        match self {
            Self::Bullet => "•".to_owned(),
            Self::Number(number) => format!("{number}."),
            Self::Continuation => String::new(),
        }
    }
}

/// A run of text sharing one style.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RichSpan {
    /// The text, with line breaks as `\n`.
    pub text: String,
    /// Bold.
    pub strong: bool,
    /// Italic.
    pub emphasis: bool,
    /// Code, in a monospace font.
    pub code: bool,
    /// Struck through, as a price no longer asked is.
    pub struck: bool,
    /// Only absolute `http`, `https` and `mailto` links are kept.
    /// Styled runs of the same anchor share its normalized target.
    pub link: Option<Arc<str>>,
    /// Whether the sender hid it, as a letter slipped into a word to fool a
    /// filter is hidden; a reader may leave it out, or show it set apart.
    pub hidden: bool,
    /// What the sender's style made it, besides what its tags did.
    pub styled: SenderStyle,
    /// Whether the sender drew its link as a button: alone, and not long, on
    /// a background of its own.
    pub button: bool,
}

/// What the sender's style made a run, apart from what its tags did, so a
/// reader can leave it out: newsletters set these with styles more often
/// than with tags.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SenderStyle {
    /// Bold.
    pub strong: bool,
    /// Italic.
    pub emphasis: bool,
    /// Struck through.
    pub struck: bool,
}

/// The structure of a sanitized HTML body, however long it is.
pub fn parse(html: &str) -> RichBody {
    parse_within(html, usize::MAX)
}

/// The structure of a sanitized HTML body, keeping at most `limit` bytes of
/// text with what holds it; a longer body is cut short, and says so.
pub fn parse_within(html: &str, limit: usize) -> RichBody {
    let tokenizer = Tokenizer::new(
        Sink(RefCell::new(Builder::new(limit))),
        TokenizerOpts::default(),
    );
    let input = BufferQueue::default();
    let mut remaining = html;
    while !remaining.is_empty() && !tokenizer.sink.0.borrow().truncated {
        let end = remaining.floor_char_boundary(INPUT_CHUNK_BYTES);
        input.push_back(StrTendril::from_slice(&remaining[..end]));
        // The sink never pauses for a script, so one feed reads the chunk.
        let _ = tokenizer.feed(&input);
        remaining = &remaining[end..];
    }
    tokenizer.end();
    std::mem::replace(&mut *tokenizer.sink.0.borrow_mut(), Builder::new(0)).finish()
}

struct Sink(RefCell<Builder>);

impl TokenSink for Sink {
    type Handle = ();

    fn process_token(&self, token: Token, _line_number: u64) -> TokenSinkResult<()> {
        let mut builder = self.0.borrow_mut();
        if builder.truncated {
            return TokenSinkResult::Continue;
        }
        match token {
            Token::TagToken(tag) => return builder.tag(&tag),
            Token::CharacterTokens(text) => builder.text(&text),
            Token::EOFToken => builder.flush(),
            _ => {}
        }
        TokenSinkResult::Continue
    }
}

/// An element whose style sets how the text within it reads, until it ends.
struct StyleFrame {
    name: String,
    /// Elements of the same name opened within it and still open.
    nested: usize,
    strong: Option<bool>,
    emphasis: Option<bool>,
    /// A line through text goes through all of it, whatever lies within.
    struck: bool,
    centered: Option<bool>,
    /// Whether it has a background of its own, as a button does.
    background: bool,
    /// How many blocks, and runs of the block being read, came before it.
    start: (usize, usize),
}

impl StyleFrame {
    /// What `tag`'s style, its `align` or its being `center` says of the
    /// text within it, if anything. A table's `align` places the table, not
    /// its text, and the style has the last word.
    fn of(name: &str, tag: &Tag) -> Option<Self> {
        let mut frame = Self {
            name: name.to_owned(),
            nested: 0,
            strong: None,
            emphasis: None,
            struck: false,
            centered: (name == "center").then_some(true).or_else(|| {
                attribute(tag, "align")
                    .filter(|_| name != "table")
                    .map(|align| align.trim().eq_ignore_ascii_case("center"))
            }),
            background: attribute(tag, "bgcolor")
                .is_some_and(|color| colors(&color.to_ascii_lowercase())),
            start: (0, 0),
        };
        for (property, value) in declarations(tag) {
            match property.as_str() {
                "background-color" => frame.background = colors(&value),
                "font-weight" => {
                    frame.strong = match value.as_str() {
                        "bold" | "bolder" => Some(true),
                        "normal" | "lighter" => Some(false),
                        number => number.parse::<u16>().ok().map(|weight| weight >= 600),
                    };
                }
                "font-style" if value == "italic" || value.starts_with("oblique") => {
                    frame.emphasis = Some(true);
                }
                "font-style" if value == "normal" => frame.emphasis = Some(false),
                "text-decoration" | "text-decoration-line" => {
                    frame.struck |= value.split_whitespace().any(|word| word == "line-through");
                }
                "text-align" => {
                    frame.centered = Some(matches!(value.as_str(), "center" | "-webkit-center"));
                }
                _ => {}
            }
        }
        let says = frame.strong.is_some()
            || frame.emphasis.is_some()
            || frame.struck
            || frame.centered.is_some()
            || frame.background;
        says.then_some(frame)
    }
}

/// Whether a background of `color` shows on a page: white and the keywords
/// that give none do not.
fn colors(color: &str) -> bool {
    let color: String = color.split_whitespace().collect();
    !matches!(
        color.as_str(),
        "" | "transparent"
            | "inherit"
            | "initial"
            | "unset"
            | "none"
            | "white"
            | "#fff"
            | "#ffffff"
            | "rgb(255,255,255)"
            | "rgba(0,0,0,0)"
    )
}

/// A table being read: where its blocks start, as the paragraphs its rows
/// make, and its cells, kept in case it turns out to hold data.
struct OpenTable {
    start: usize,
    rows: Vec<TableRow>,
    row: Option<TableRow>,
    cell: Option<Vec<RichSpan>>,
    /// Each row's width, a cell that spans several columns counted so.
    widths: Vec<usize>,
    /// The width of the row being read.
    width: usize,
    /// Whether it may still hold data: nothing but short text in its cells,
    /// and no table within it.
    data: bool,
}

impl OpenTable {
    fn new(start: usize) -> Self {
        Self {
            start,
            rows: Vec::new(),
            row: None,
            cell: None,
            widths: Vec::new(),
            width: 0,
            data: true,
        }
    }

    fn start_row(&mut self) {
        self.end_row();
        self.row = Some(TableRow {
            cells: Vec::new(),
            header: true,
        });
    }

    fn start_cell(&mut self, header: bool, columns: usize) {
        self.end_cell();
        let row = self.row.get_or_insert_with(|| TableRow {
            cells: Vec::new(),
            header: true,
        });
        row.header &= header;
        self.width += columns;
        self.cell = Some(Vec::new());
    }

    fn end_cell(&mut self) {
        let Some(mut cell) = self.cell.take() else {
            return;
        };
        if let Some(last) = cell.last_mut() {
            let length = last.text.trim_end().len();
            last.text.truncate(length);
        }
        cell.retain(|span| !span.text.is_empty());
        let shown: usize = cell
            .iter()
            .filter(|span| !span.hidden)
            .map(|span| span.text.chars().count())
            .sum();
        self.data &= shown <= MAX_CELL_CHARS;
        if let Some(row) = &mut self.row {
            row.cells.push(cell);
        }
    }

    fn end_row(&mut self) {
        self.end_cell();
        if let Some(row) = self.row.take()
            && !row.cells.is_empty()
        {
            self.rows.push(row);
            self.widths.push(self.width);
        }
        self.width = 0;
    }

    /// Whether, read whole, it holds data rather than laying a page out: two
    /// rows or more, two columns or more, and every row as wide as the
    /// others, unless a row of headings says what the columns are.
    fn holds_data(&self) -> bool {
        self.data
            && self.rows.len() >= 2
            && self.widths.iter().copied().max().unwrap_or(0) >= 2
            && (self.widths.windows(2).all(|pair| pair[0] == pair[1])
                || self.rows.iter().any(|row| row.header))
    }
}

/// A list open around the text being read: the number of an ordered one's
/// next item, and the marker of the item it sits in, which the text after it
/// goes back to.
struct OpenList {
    next: Option<u32>,
    within: Option<ListMarker>,
}

/// One body's conversion: the blocks read so far, and what the elements
/// still open say about the text to come.
struct Builder {
    blocks: Vec<RichBlock>,
    spans: Vec<RichSpan>,
    /// A separating space has already been written since the last visible
    /// character.
    space: bool,
    /// Whether the text shown last in this block ends in a space, or none is
    /// shown yet: kept as runs are written, so a space after hidden text
    /// need not look back over every run for it.
    shown_ends_in_space: bool,
    strong: u16,
    emphasis: u16,
    code: u16,
    struck: u16,
    link: Option<Arc<str>>,
    heading: Option<u8>,
    /// The lists kept open, innermost last, at most [`MAX_LIST_DEPTH`].
    lists: Vec<OpenList>,
    /// How many lists are open beyond those kept, so that their ends close
    /// them before any of `lists`.
    omitted_lists: usize,
    /// The marker of the next block of the list item being read.
    item: Option<ListMarker>,
    /// How many quotes are open, counted in full so that their ends unwind
    /// to the right level; blocks keep at most `u8::MAX`.
    quote_depth: usize,
    pre: u16,
    pre_text: String,
    /// Element whose content is never read, as `script`'s is.
    hidden: Skipping,
    /// Element the sender hid, whose content is read but marked hidden.
    sender_hidden: Skipping,
    /// Whether the void element being read, as an image is, was hidden.
    void_hidden: bool,
    /// The elements open whose style sets how text reads, innermost last, at
    /// most [`MAX_STYLE_DEPTH`].
    frames: Vec<StyleFrame>,
    /// Whether the block being read is centered: what held when its text
    /// began, since a cell's row ends after the cell does.
    centered: bool,
    /// The tables open, innermost last, at most [`MAX_TABLE_DEPTH`].
    tables: Vec<OpenTable>,
    /// How many tables are open beyond those kept, so that their ends close
    /// them before any of `tables`.
    omitted_tables: usize,
    /// The bytes of text, and of what holds it, the body may take.
    limit: usize,
    /// The bytes taken so far, within `limit`.
    used: usize,
    /// Whether something was left out to stay within `limit`; the rest of
    /// the body is then ignored.
    truncated: bool,
    /// See [`RichBody::undescribed_images`].
    undescribed_images: usize,
}

impl Builder {
    fn new(limit: usize) -> Self {
        Self {
            blocks: Vec::new(),
            spans: Vec::new(),
            space: false,
            shown_ends_in_space: true,
            strong: 0,
            emphasis: 0,
            code: 0,
            struck: 0,
            link: None,
            heading: None,
            lists: Vec::new(),
            omitted_lists: 0,
            item: None,
            quote_depth: 0,
            pre: 0,
            pre_text: String::new(),
            hidden: Skipping::default(),
            sender_hidden: Skipping::default(),
            void_hidden: false,
            frames: Vec::new(),
            centered: false,
            tables: Vec::new(),
            omitted_tables: 0,
            limit,
            used: 0,
            truncated: false,
            undescribed_images: 0,
        }
    }

    fn tag(&mut self, tag: &Tag) -> TokenSinkResult<()> {
        let name = &*tag.name;
        match tag.kind {
            TagKind::StartTag => {
                self.start(name, tag);
                if tag.self_closing && !VOID_ELEMENTS.contains(&name) {
                    self.end(name);
                    return TokenSinkResult::Continue;
                }
                raw_content(name)
            }
            TagKind::EndTag => {
                self.end(name);
                TokenSinkResult::Continue
            }
        }
    }

    fn start(&mut self, name: &str, tag: &Tag) {
        if self.hidden.active() {
            self.hidden.open(tag);
            return;
        }
        if HIDDEN_ELEMENTS.contains(&name) {
            self.hidden.start(name);
            return;
        }
        if self.sender_hidden.active() {
            self.sender_hidden.open(tag);
        } else if hides(tag) {
            if VOID_ELEMENTS.contains(&name) {
                self.void_hidden = true;
            } else {
                self.sender_hidden.start(name);
            }
        }
        self.start_element(name, tag);
        self.void_hidden = false;
        // Its style applies within it, after the block before it ended.
        if VOID_ELEMENTS.contains(&name) {
            return;
        }
        match StyleFrame::of(name, tag).filter(|_| self.frames.len() < MAX_STYLE_DEPTH) {
            Some(mut frame) => {
                frame.start = (self.blocks.len(), self.spans.len());
                self.frames.push(frame);
            }
            None => {
                if let Some(top) = self.frames.last_mut()
                    && top.name == name
                {
                    top.nested += 1;
                }
            }
        }
    }

    fn start_element(&mut self, name: &str, tag: &Tag) {
        // A table with more than short text in it lays a page out.
        if matches!(
            name,
            "hr" | "img"
                | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
                | "ul"
                | "ol"
                | "li"
                | "blockquote"
                | "pre"
        ) && let Some(table) = self.tables.last_mut()
        {
            table.data = false;
        }
        match name {
            "br" => self.line_break(),
            "hr" => {
                self.flush();
                self.push(BlockKind::Rule);
            }
            "img" => self.image(tag),
            "table" => {
                self.flush();
                if let Some(outer) = self.tables.last_mut() {
                    outer.data = false;
                }
                if self.tables.len() < MAX_TABLE_DEPTH {
                    self.tables.push(OpenTable::new(self.blocks.len()));
                } else {
                    self.omitted_tables = self.omitted_tables.saturating_add(1);
                }
            }
            "tr" => {
                self.flush();
                if let Some(table) = self.open_table() {
                    table.start_row();
                }
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.flush();
                self.heading = name[1..].parse().ok();
            }
            "ul" => {
                self.flush();
                self.start_list(None);
            }
            "ol" => {
                self.flush();
                self.start_list(Some(1));
            }
            "li" => {
                self.flush();
                self.item = Some(self.next_marker());
            }
            "blockquote" => {
                self.flush();
                self.quote_depth = self.quote_depth.saturating_add(1);
            }
            "pre" => {
                self.flush();
                self.pre = self.pre.saturating_add(1);
            }
            "b" | "strong" => self.strong = self.strong.saturating_add(1),
            "i" | "em" | "cite" => self.emphasis = self.emphasis.saturating_add(1),
            "code" | "kbd" | "samp" | "tt" => self.code = self.code.saturating_add(1),
            "s" | "strike" | "del" => self.struck = self.struck.saturating_add(1),
            // A target longer than the whole body may be could not be kept.
            "a" => self.link = link_target(tag).filter(|link| link.len() <= self.limit),
            // Table cells read left to right, separated like words.
            "td" | "th" => {
                self.separate();
                let columns = attribute(tag, "colspan")
                    .and_then(|columns| columns.trim().parse::<usize>().ok())
                    .unwrap_or(1)
                    .clamp(1, MAX_TABLE_DEPTH);
                if let Some(table) = self.open_table() {
                    table.start_cell(name == "th", columns);
                }
            }
            _ if BLOCK_ELEMENTS.contains(&name) => self.flush(),
            _ => {}
        }
    }

    /// The innermost table kept open, unless one too deep to keep is.
    fn open_table(&mut self) -> Option<&mut OpenTable> {
        if self.omitted_tables > 0 {
            return None;
        }
        self.tables.last_mut()
    }

    fn end(&mut self, name: &str) {
        if self.hidden.active() {
            self.hidden.close(name);
            return;
        }
        self.end_element(name);
        // The element ends hidden and styled, its last block included.
        self.sender_hidden.close(name);
        if let Some(top) = self.frames.last_mut()
            && top.name == name
        {
            if top.nested > 0 {
                top.nested -= 1;
            } else if let Some(frame) = self.frames.pop()
                && frame.background
            {
                self.mark_button(frame.start);
            }
        }
    }

    /// Marks as a button the link an element with a background of its own
    /// held, when it held nothing else and not much: the blocks and runs
    /// read since `start`, all text of one link.
    fn mark_button(&mut self, (block, run): (usize, usize)) {
        let flushed = self.blocks.len() > block;
        let mut runs: Vec<&mut RichSpan> = Vec::new();
        for (index, read) in self.blocks.iter_mut().enumerate().skip(block) {
            let (BlockKind::Paragraph(spans)
            | BlockKind::Heading { spans, .. }
            | BlockKind::ListItem { spans, .. }) = &mut read.kind
            else {
                return;
            };
            let from = if index == block {
                run.min(spans.len())
            } else {
                0
            };
            runs.extend(spans[from..].iter_mut());
        }
        let from = if flushed {
            0
        } else {
            run.min(self.spans.len())
        };
        runs.extend(self.spans[from..].iter_mut());
        runs.retain(|span| !span.hidden && !span.text.trim().is_empty());
        let Some(link) = runs.first().and_then(|span| span.link.clone()) else {
            return;
        };
        let length: usize = runs.iter().map(|span| span.text.chars().count()).sum();
        if length <= MAX_BUTTON_CHARS && runs.iter().all(|span| span.link.as_ref() == Some(&link)) {
            for span in runs {
                span.button = true;
            }
        }
    }

    /// What the sender's style makes the text read now.
    fn sender_style(&self) -> SenderStyle {
        let innermost = |says: fn(&StyleFrame) -> Option<bool>| {
            self.frames.iter().rev().find_map(says).unwrap_or(false)
        };
        SenderStyle {
            strong: innermost(|frame| frame.strong),
            emphasis: innermost(|frame| frame.emphasis),
            struck: self.frames.iter().any(|frame| frame.struck),
        }
    }

    fn end_element(&mut self, name: &str) {
        match name {
            "br" => self.line_break(),
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.flush();
                self.heading = None;
            }
            "ul" | "ol" => {
                self.flush();
                // The item the list sat in goes on, under its own text.
                self.item = if self.omitted_lists > 0 {
                    self.omitted_lists -= 1;
                    None
                } else {
                    self.lists.pop().and_then(|list| list.within)
                };
            }
            "li" => {
                self.flush();
                self.item = None;
            }
            "blockquote" => {
                self.flush();
                self.quote_depth = self.quote_depth.saturating_sub(1);
            }
            "pre" => self.end_pre(),
            "b" | "strong" => self.strong = self.strong.saturating_sub(1),
            "i" | "em" | "cite" => self.emphasis = self.emphasis.saturating_sub(1),
            "code" | "kbd" | "samp" | "tt" => self.code = self.code.saturating_sub(1),
            "s" | "strike" | "del" => self.struck = self.struck.saturating_sub(1),
            "a" => self.link = None,
            "td" | "th" => {
                if let Some(table) = self.open_table() {
                    table.end_cell();
                }
            }
            "tr" => {
                self.flush();
                if let Some(table) = self.open_table() {
                    table.end_row();
                }
            }
            "table" => {
                self.flush();
                if self.omitted_tables > 0 {
                    self.omitted_tables -= 1;
                } else if let Some(mut table) = self.tables.pop() {
                    table.end_row();
                    // Read whole, a table of data takes the place of the
                    // paragraphs its rows made.
                    if table.holds_data() {
                        self.blocks.truncate(table.start);
                        self.push(BlockKind::Table(table.rows));
                    }
                }
            }
            _ if BLOCK_ELEMENTS.contains(&name) => self.flush(),
            _ => {}
        }
    }

    fn text(&mut self, text: &str) {
        if self.hidden.active() {
            return;
        }
        if self.pre > 0 {
            // A terminal would obey a control character; spacing stays.
            for character in text
                .chars()
                .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
            {
                self.push_pre(character);
            }
            return;
        }

        // Collapse whitespace runs like a browser does. A control character
        // separates words too, without a space.
        for run in text.split_inclusive(|c: char| c.is_whitespace() || c.is_control()) {
            let last = run
                .chars()
                .next_back()
                .expect("split_inclusive never yields an empty run");
            let separator = last.is_whitespace() || last.is_control();
            let word = if separator {
                &run[..run.len() - last.len_utf8()]
            } else {
                run
            };
            if !word.is_empty() {
                // Text outside a cell, within a table, lays it out.
                if let Some(table) = self.open_table()
                    && table.cell.is_none()
                {
                    table.data = false;
                }
                self.space = false;
                self.push_text(word);
            }
            if last.is_whitespace() {
                self.separate();
            }
        }
    }

    /// Writes pending space with the preceding run's style; `flush` drops it.
    /// A space after hidden text, beside one already shown before it, is
    /// hidden with it, so leaving that text out leaves a single space.
    fn separate(&mut self) {
        if !self.space && self.wants_space() {
            let beside_space = !self.marking()
                && self.spans.last().is_some_and(|span| span.hidden)
                && self.shown_ends_in_space;
            self.push_run(" ", self.marking() || beside_space);
            self.space = true;
        }
    }

    /// Whether what is read now is something the sender hid.
    fn marking(&self) -> bool {
        self.sender_hidden.active() || self.void_hidden
    }

    /// Whether a space may separate what comes next from the text so far.
    fn wants_space(&self) -> bool {
        self.spans
            .last()
            .is_some_and(|span| !span.text.ends_with('\n'))
    }

    fn push_text(&mut self, text: &str) {
        self.push_run(text, self.marking());
    }

    fn push_run(&mut self, text: &str, hidden: bool) {
        let (strong, emphasis, code, struck) = (
            self.strong > 0,
            self.emphasis > 0,
            self.code > 0,
            self.struck > 0,
        );
        let styled = self.sender_style();
        let continues = self.spans.last().is_some_and(|span| {
            span.strong == strong
                && span.emphasis == emphasis
                && span.code == code
                && span.struck == struck
                && span.link == self.link
                && span.hidden == hidden
                && span.styled == styled
                && !span.button
        });
        // A new run costs its own storage as well as its text, so many tiny
        // style changes cannot grow the body past its limit.
        let overhead = if continues {
            0
        } else {
            std::mem::size_of::<RichSpan>() + self.link.as_ref().map_or(0, |link| link.len())
        };
        let Some(text) = self.take(text, overhead) else {
            return;
        };
        if self.spans.is_empty() {
            self.centered = self.centers();
        }
        if !hidden && !text.is_empty() {
            self.shown_ends_in_space = text.ends_with(char::is_whitespace);
        }
        let run = RichSpan {
            text: text.to_owned(),
            strong,
            emphasis,
            code,
            struck,
            link: self.link.clone(),
            hidden,
            styled,
            button: false,
        };
        // A cell keeps its own copy, in case its table holds data, which ends
        // once the table is known to hold none; a line break within it reads
        // as a space there.
        if let Some(cell) = self
            .open_table()
            .filter(|table| table.data)
            .and_then(|table| table.cell.as_mut())
        {
            let text = if text == "\n" { " " } else { text };
            match cell.last_mut() {
                Some(last) if same_style(last, &run) => last.text.push_str(text),
                _ => cell.push(RichSpan {
                    text: text.to_owned(),
                    ..run.clone()
                }),
            }
        }
        match self.spans.last_mut() {
            Some(span) if continues => {
                span.text.push_str(text);
                // Equal adjacent anchors may have separate allocations. Retain
                // the current target so later runs use Arc's pointer fast path.
                span.link = run.link;
            }
            _ => self.spans.push(run),
        }
    }

    fn push_pre(&mut self, character: char) {
        let mut buffer = [0; 4];
        if let Some(text) = self.take(character.encode_utf8(&mut buffer), 0) {
            self.pre_text.push_str(text);
        }
    }

    /// The part of `text` that fits in what is left of the limit, after
    /// `overhead` and the room its block needs, cut at a character; anything
    /// left out ends the body.
    fn take<'t>(&mut self, text: &'t str, overhead: usize) -> Option<&'t str> {
        let room = self
            .limit
            .saturating_sub(self.used)
            .saturating_sub(overhead)
            .saturating_sub(BLOCK_SIZE);
        let end = text.floor_char_boundary(room);
        if end < text.len() {
            self.truncated = true;
        }
        if end == 0 {
            return None;
        }
        self.used += overhead + end;
        Some(&text[..end])
    }

    fn line_break(&mut self) {
        if self.pre > 0 {
            self.push_pre('\n');
        } else if !self.spans.is_empty() {
            self.push_text("\n");
            self.space = false;
        }
    }

    /// Images are not loaded; described ones become a placeholder, and the
    /// others are only counted.
    fn image(&mut self, tag: &Tag) {
        let description = attribute(tag, "alt").map(str::trim).unwrap_or_default();
        // A part of the message itself, which a reader may show.
        let content_id = attribute(tag, "src")
            .map(str::trim)
            .and_then(|source| {
                let (scheme, id) = source.split_once(':')?;
                scheme.eq_ignore_ascii_case("cid").then_some(id)
            })
            .filter(|id| !id.is_empty());
        let pixel = ["width", "height"].into_iter().any(|side| {
            attribute(tag, side)
                .and_then(|size| size.trim().trim_end_matches("px").parse::<u32>().ok())
                .is_some_and(|size| size <= 1)
        });
        if description.is_empty() && (content_id.is_none() || pixel) {
            if !pixel && !self.marking() {
                self.undescribed_images = self.undescribed_images.saturating_add(1);
            }
            return;
        }
        self.flush();
        let Some(description) = self.take_all(description) else {
            return;
        };
        let content_id = match content_id {
            Some(id) => match self.take_all(id) {
                Some(id) => Some(id),
                None => return,
            },
            None => None,
        };
        self.push(BlockKind::Image {
            description,
            content_id,
        });
    }

    /// `text`, if all of it fits in what is left of the limit.
    fn take_all(&mut self, text: &str) -> Option<String> {
        if text.is_empty() {
            return Some(String::new());
        }
        let kept = self.take(text, 0)?;
        (kept.len() == text.len()).then(|| kept.to_owned())
    }

    fn start_list(&mut self, next: Option<u32>) {
        if self.lists.len() < MAX_LIST_DEPTH {
            let within = self.item.take();
            self.lists.push(OpenList { next, within });
        } else {
            self.omitted_lists = self.omitted_lists.saturating_add(1);
        }
    }

    fn next_marker(&mut self) -> ListMarker {
        let next = self
            .lists
            .last_mut()
            .filter(|_| self.omitted_lists == 0)
            .and_then(|list| list.next.as_mut());
        match next {
            Some(number) => {
                let marker = ListMarker::Number(*number);
                *number = number.saturating_add(1);
                marker
            }
            None => ListMarker::Bullet,
        }
    }

    /// Ends the current paragraph, if it has any text.
    fn flush(&mut self) {
        // A block ending within a cell ends where a space would there.
        if let Some(last) = self
            .open_table()
            .and_then(|table| table.cell.as_mut())
            .and_then(|cell| cell.last_mut())
            .filter(|last| !last.text.ends_with(char::is_whitespace))
        {
            last.text.push(' ');
        }
        if let Some(last) = self.spans.last_mut() {
            let length = last.text.trim_end().len();
            last.text.truncate(length);
        }
        self.spans.retain(|span| !span.text.is_empty());
        self.space = false;
        self.shown_ends_in_space = true;
        if self.spans.is_empty() {
            return;
        }

        let spans = std::mem::take(&mut self.spans);
        let hidden = spans.iter().all(|span| span.hidden);
        let kind = if let Some(level) = self.heading {
            BlockKind::Heading { level, spans }
        } else if let Some(marker) = self.item.take() {
            // Later blocks of the same item keep the indent, not the marker.
            self.item = Some(ListMarker::Continuation);
            BlockKind::ListItem {
                marker,
                depth: u8::try_from(self.lists.len().max(1)).unwrap_or(u8::MAX),
                spans,
            }
        } else {
            BlockKind::Paragraph(spans)
        };
        self.push_block(kind, hidden, self.centered);
    }

    fn end_pre(&mut self) {
        if self.pre == 0 {
            return;
        }
        self.pre -= 1;
        if self.pre > 0 {
            return;
        }

        let text = std::mem::take(&mut self.pre_text);
        // A newline right after `<pre>` is not part of the content.
        let text = text.strip_prefix('\n').unwrap_or(&text).trim_end();
        if !text.trim().is_empty() {
            self.push(BlockKind::Preformatted(text.to_owned()));
        }
    }

    /// A block that is not text, hidden if the sender hid what it is in.
    fn push(&mut self, kind: BlockKind) {
        self.push_block(kind, self.marking(), self.centers());
    }

    /// Whether the sender's style centers what is read now.
    fn centers(&self) -> bool {
        self.frames
            .iter()
            .rev()
            .find_map(|frame| frame.centered)
            .unwrap_or(false)
    }

    fn push_block(&mut self, kind: BlockKind, hidden: bool, centered: bool) {
        if self.limit.saturating_sub(self.used) < BLOCK_SIZE {
            self.truncated = true;
            return;
        }
        self.used += BLOCK_SIZE;
        self.blocks.push(RichBlock {
            kind,
            quote_depth: u8::try_from(self.quote_depth).unwrap_or(u8::MAX),
            hidden,
            centered,
        });
    }

    fn finish(mut self) -> RichBody {
        self.flush();
        if self.pre > 0 {
            // A <pre> left open ends with the body: at depth one, end_pre
            // closes all its levels at once.
            self.pre = 1;
            self.end_pre();
        }
        RichBody {
            blocks: self.blocks,
            truncated: self.truncated,
            undescribed_images: self.undescribed_images,
        }
    }
}

/// Whether two runs read alike, and so make one.
fn same_style(one: &RichSpan, other: &RichSpan) -> bool {
    one.strong == other.strong
        && one.emphasis == other.emphasis
        && one.code == other.code
        && one.struck == other.struck
        && one.link == other.link
        && one.hidden == other.hidden
        && one.styled == other.styled
        && one.button == other.button
}

/// Keeps the tokenizer from reading markup inside raw-text elements.
fn raw_content(name: &str) -> TokenSinkResult<()> {
    match name {
        "script" => TokenSinkResult::RawData(RawKind::ScriptData),
        "style" => TokenSinkResult::RawData(RawKind::Rawtext),
        "title" | "textarea" => TokenSinkResult::RawData(RawKind::Rcdata),
        _ => TokenSinkResult::Continue,
    }
}

fn attribute<'t>(tag: &'t Tag, name: &str) -> Option<&'t str> {
    tag.attrs
        .iter()
        .find(|attribute| &*attribute.name.local == name)
        .map(|attribute| &*attribute.value)
}

/// Only absolute web and mail links stay clickable.
fn link_target(tag: &Tag) -> Option<Arc<str>> {
    let url = url::Url::parse(attribute(tag, "href")?.trim()).ok()?;
    matches!(url.scheme(), "http" | "https" | "mailto").then(|| Arc::from(url.as_str()))
}

#[cfg(test)]
mod tests;
