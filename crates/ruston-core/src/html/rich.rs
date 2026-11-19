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

use super::{BLOCK_ELEMENTS, HIDDEN_ELEMENTS, VOID_ELEMENTS};

/// The most lists kept open within one another. A deeper one reads as the
/// deepest kept, so a hostile body cannot grow the stack without end.
pub const MAX_LIST_DEPTH: usize = 8;
/// How much input the tokenizer reads at a time, so a body cut short is not
/// read to its end.
const INPUT_CHUNK_BYTES: usize = 8 * 1024;
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
    /// however long the body is.
    pub fn plain_lines(&self) -> impl Iterator<Item = String> + '_ {
        self.blocks.iter().flat_map(|block| {
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
                BlockKind::Image { description } => format!("[Image: {description}]"),
                BlockKind::Rule => "---".to_owned(),
            };
            let quote = "> ".repeat(usize::from(block.quote_depth));
            text.lines()
                .map(|line| format!("{quote}{line}"))
                .collect::<Vec<_>>()
        })
    }
}

fn join(spans: &[RichSpan]) -> String {
    spans.iter().map(|span| span.text.as_str()).collect()
}

/// One block of a body, as a paragraph or a list item is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RichBlock {
    /// What the block is, with its content.
    pub kind: BlockKind,
    /// How deeply the block sits inside quotes.
    pub quote_depth: u8,
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
    /// An image that is not loaded; only its description is shown.
    Image {
        /// What the sender wrote to describe it.
        description: String,
    },
    /// A line across, between two parts of the body.
    Rule,
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
    /// Element whose content is being skipped.
    hidden: Option<String>,
    /// The bytes of text, and of what holds it, the body may take.
    limit: usize,
    /// The bytes taken so far, within `limit`.
    used: usize,
    /// Whether something was left out to stay within `limit`; the rest of
    /// the body is then ignored.
    truncated: bool,
}

impl Builder {
    fn new(limit: usize) -> Self {
        Self {
            blocks: Vec::new(),
            spans: Vec::new(),
            space: false,
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
            hidden: None,
            limit,
            used: 0,
            truncated: false,
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
        if self.hidden.is_some() {
            return;
        }
        if HIDDEN_ELEMENTS.contains(&name) {
            self.hidden = Some(name.to_owned());
            return;
        }

        match name {
            "br" => self.line_break(),
            "hr" => {
                self.flush();
                self.push(BlockKind::Rule);
            }
            "img" => self.image(tag),
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
            "td" | "th" => self.separate(),
            _ if BLOCK_ELEMENTS.contains(&name) => self.flush(),
            _ => {}
        }
    }

    fn end(&mut self, name: &str) {
        if let Some(hidden) = &self.hidden {
            if hidden == name {
                self.hidden = None;
            }
            return;
        }

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
            _ if BLOCK_ELEMENTS.contains(&name) => self.flush(),
            _ => {}
        }
    }

    fn text(&mut self, text: &str) {
        if self.hidden.is_some() {
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
                self.space = false;
                self.push_text(word);
            }
            if last.is_whitespace() {
                self.separate();
            }
        }
    }

    /// Writes pending space with the preceding run's style; `flush` drops it.
    fn separate(&mut self) {
        if !self.space && self.wants_space() {
            self.push_text(" ");
            self.space = true;
        }
    }

    /// Whether a space may separate what comes next from the text so far.
    fn wants_space(&self) -> bool {
        self.spans
            .last()
            .is_some_and(|span| !span.text.ends_with('\n'))
    }

    fn push_text(&mut self, text: &str) {
        let (strong, emphasis, code, struck) = (
            self.strong > 0,
            self.emphasis > 0,
            self.code > 0,
            self.struck > 0,
        );
        let continues = self.spans.last().is_some_and(|span| {
            span.strong == strong
                && span.emphasis == emphasis
                && span.code == code
                && span.struck == struck
                && span.link == self.link
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
        match self.spans.last_mut() {
            Some(span) if continues => {
                span.text.push_str(text);
                // Equal adjacent anchors may have separate allocations. Retain
                // the current target so later runs use Arc's pointer fast path.
                span.link = self.link.clone();
            }
            _ => self.spans.push(RichSpan {
                text: text.to_owned(),
                strong,
                emphasis,
                code,
                struck,
                link: self.link.clone(),
            }),
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

    /// Images are not loaded; described ones become a placeholder and
    /// decorative ones (no description, e.g. tracking pixels) are dropped.
    fn image(&mut self, tag: &Tag) {
        let Some(description) = attribute(tag, "alt")
            .map(str::trim)
            .filter(|alt| !alt.is_empty())
        else {
            return;
        };
        self.flush();
        let Some(description) = self.take(description, 0) else {
            return;
        };
        let description = description.to_owned();
        self.push(BlockKind::Image { description });
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
        if let Some(last) = self.spans.last_mut() {
            let length = last.text.trim_end().len();
            last.text.truncate(length);
        }
        self.spans.retain(|span| !span.text.is_empty());
        self.space = false;
        if self.spans.is_empty() {
            return;
        }

        let spans = std::mem::take(&mut self.spans);
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
        self.push(kind);
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

    fn push(&mut self, kind: BlockKind) {
        if self.limit.saturating_sub(self.used) < BLOCK_SIZE {
            self.truncated = true;
            return;
        }
        self.used += BLOCK_SIZE;
        self.blocks.push(RichBlock {
            kind,
            quote_depth: u8::try_from(self.quote_depth).unwrap_or(u8::MAX),
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
        }
    }
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
