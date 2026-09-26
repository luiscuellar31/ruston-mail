//! Readable terminal text from a decrypted HTML message.

use std::cell::RefCell;

use html5ever::tendril::StrTendril;
use html5ever::tokenizer::states::RawKind;
use html5ever::tokenizer::{
    BufferQueue, Tag, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer, TokenizerOpts,
};

const HIDDEN: &[&str] = &[
    "head", "iframe", "noscript", "script", "style", "svg", "template", "textarea", "title",
];
const BLOCKS: &[&str] = &[
    "address", "article", "aside", "div", "footer", "header", "main", "p", "section", "table", "tr",
];
const VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track",
    "wbr",
];

pub(super) fn to_markdown(html: &str) -> String {
    let tokenizer = Tokenizer::new(Sink::default(), TokenizerOpts::default());
    let input = BufferQueue::default();
    input.push_back(StrTendril::from_slice(html));
    let _ = tokenizer.feed(&input);
    tokenizer.end();
    std::mem::take(&mut *tokenizer.sink.0.borrow_mut()).finish()
}

#[derive(Default)]
struct Sink(RefCell<Builder>);

impl TokenSink for Sink {
    type Handle = ();

    fn process_token(&self, token: Token, _line_number: u64) -> TokenSinkResult<()> {
        let mut builder = self.0.borrow_mut();
        match token {
            Token::TagToken(tag) => return builder.tag(&tag),
            Token::CharacterTokens(text) => builder.text(&text),
            Token::EOFToken => builder.flush(),
            _ => {}
        }
        TokenSinkResult::Continue
    }
}

#[derive(Clone, Default, PartialEq, Eq)]
struct Style {
    bold: bool,
    italic: bool,
    code: bool,
    link: Option<String>,
}

struct Span {
    text: String,
    style: Style,
}

struct Block {
    text: String,
    list_item: bool,
}

#[derive(Default)]
struct Builder {
    blocks: Vec<Block>,
    spans: Vec<Span>,
    pending_space: bool,
    hidden: Option<String>,
    heading: Option<usize>,
    quote_depth: usize,
    lists: Vec<Option<u32>>,
    item_marker: Option<String>,
    pre_depth: u16,
    pre_text: String,
    bold: u16,
    italic: u16,
    code: u16,
    link: Option<String>,
}

impl Builder {
    fn tag(&mut self, tag: &Tag) -> TokenSinkResult<()> {
        let name = &*tag.name;
        match tag.kind {
            TagKind::StartTag => {
                self.start(name, tag);
                if tag.self_closing && !VOID.contains(&name) {
                    self.end(name);
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
        if HIDDEN.contains(&name) {
            self.hidden = Some(name.to_owned());
            return;
        }
        match name {
            "br" => self.line_break(),
            "hr" => {
                self.flush();
                self.push_block("---".to_owned(), false);
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.flush();
                self.heading = name[1..].parse().ok();
            }
            "ul" => {
                self.flush();
                self.lists.push(None);
            }
            "ol" => {
                self.flush();
                self.lists.push(Some(1));
            }
            "li" => {
                self.flush();
                self.item_marker = Some(match self.lists.last_mut() {
                    Some(Some(number)) => {
                        let marker = format!("{number}.");
                        *number = number.saturating_add(1);
                        marker
                    }
                    _ => "-".to_owned(),
                });
            }
            "blockquote" => {
                self.flush();
                self.quote_depth = self.quote_depth.saturating_add(1);
            }
            "pre" => {
                self.flush();
                self.pre_depth = self.pre_depth.saturating_add(1);
            }
            "b" | "strong" => self.bold = self.bold.saturating_add(1),
            "i" | "em" => self.italic = self.italic.saturating_add(1),
            "code" | "kbd" | "samp" => self.code = self.code.saturating_add(1),
            "a" => {
                self.consume_pending_space();
                self.link = link_target(tag);
            }
            "td" | "th" => self.pending_space = !self.spans.is_empty(),
            _ if BLOCKS.contains(&name) => self.flush(),
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
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.flush();
                self.heading = None;
            }
            "ul" | "ol" => {
                self.flush();
                self.lists.pop();
                self.item_marker = None;
            }
            "li" => {
                self.flush();
                self.item_marker = None;
            }
            "blockquote" => {
                self.flush();
                self.quote_depth = self.quote_depth.saturating_sub(1);
            }
            "pre" => self.end_pre(),
            "b" | "strong" => self.bold = self.bold.saturating_sub(1),
            "i" | "em" => self.italic = self.italic.saturating_sub(1),
            "code" | "kbd" | "samp" => self.code = self.code.saturating_sub(1),
            "a" => self.link = None,
            "br" => self.line_break(),
            _ if BLOCKS.contains(&name) => self.flush(),
            _ => {}
        }
    }

    fn text(&mut self, text: &str) {
        if self.hidden.is_some() {
            return;
        }
        if self.pre_depth > 0 {
            self.pre_text.extend(
                text.chars()
                    .filter(|c| !c.is_control() || matches!(c, '\n' | '\t')),
            );
            return;
        }
        for c in text.chars() {
            if c.is_whitespace() {
                self.pending_space = self
                    .spans
                    .last()
                    .is_some_and(|span| !span.text.ends_with('\n'));
            } else if !c.is_control() {
                if self.pending_space {
                    self.consume_pending_space();
                }
                self.push_char(
                    c,
                    Style {
                        bold: self.bold > 0,
                        italic: self.italic > 0,
                        code: self.code > 0,
                        link: self.link.clone(),
                    },
                );
            }
        }
    }

    fn push_char(&mut self, c: char, style: Style) {
        match self.spans.last_mut() {
            Some(span) if span.style == style => span.text.push(c),
            _ => self.spans.push(Span {
                text: c.to_string(),
                style,
            }),
        }
    }

    fn consume_pending_space(&mut self) {
        if self.pending_space && !self.spans.is_empty() {
            self.push_char(
                ' ',
                Style {
                    link: self.link.clone(),
                    ..Style::default()
                },
            );
        }
        self.pending_space = false;
    }

    fn line_break(&mut self) {
        self.pending_space = false;
        if self.pre_depth > 0 {
            self.pre_text.push('\n');
        } else if !self.spans.is_empty() {
            self.push_char('\n', Style::default());
        }
    }

    fn flush(&mut self) {
        self.pending_space = false;
        while let Some(last) = self.spans.last_mut() {
            last.text.truncate(last.text.trim_end().len());
            if last.text.is_empty() {
                self.spans.pop();
            } else {
                break;
            }
        }
        if self.spans.is_empty() {
            return;
        }

        let text = render_spans(std::mem::take(&mut self.spans));
        let (text, list_item) = if let Some(level) = self.heading {
            (format!("{} {text}", "#".repeat(level)), false)
        } else if let Some(marker) = self.item_marker.as_mut() {
            let depth = self.lists.len().saturating_sub(1);
            let text = format!("{}{} {text}", "  ".repeat(depth), marker);
            let list_item = !marker.is_empty();
            marker.clear();
            (text, list_item)
        } else {
            (text, false)
        };
        self.push_block(text, list_item);
    }

    fn end_pre(&mut self) {
        if self.pre_depth == 0 {
            return;
        }
        self.pre_depth -= 1;
        if self.pre_depth > 0 {
            return;
        }
        let text = std::mem::take(&mut self.pre_text);
        let text = text
            .strip_prefix('\n')
            .unwrap_or(&text)
            .trim_end_matches('\n');
        if !text.trim().is_empty() {
            let indented = text
                .lines()
                .map(|line| format!("    {line}"))
                .collect::<Vec<_>>()
                .join("\n");
            self.push_block(indented, false);
        }
    }

    fn push_block(&mut self, text: String, list_item: bool) {
        let prefix = "> ".repeat(self.quote_depth);
        let text = text
            .lines()
            .map(|line| format!("{prefix}{line}"))
            .collect::<Vec<_>>()
            .join("\n");
        self.blocks.push(Block { text, list_item });
    }

    fn finish(mut self) -> String {
        self.flush();
        if self.pre_depth > 0 {
            self.pre_depth = 1;
            self.end_pre();
        }
        let mut output = String::new();
        let mut prior_list_item = false;
        for block in self.blocks {
            if !output.is_empty() {
                output.push_str(if prior_list_item && block.list_item {
                    "\n"
                } else {
                    "\n\n"
                });
            }
            prior_list_item = block.list_item;
            output.push_str(&block.text);
        }
        output
    }
}

fn render_spans(spans: Vec<Span>) -> String {
    let mut output = String::new();
    let mut link: Option<String> = None;
    for span in spans {
        if span.style.link != link {
            if let Some(url) = link.take() {
                output.push_str(&format!("]({url})"));
            }
            if span.style.link.is_some() {
                output.push('[');
            }
            link = span.style.link.clone();
        }
        output.push_str(&render_span(span));
    }
    if let Some(url) = link {
        output.push_str(&format!("]({url})"));
    }
    output
}

fn render_span(span: Span) -> String {
    let mut text = span.text;
    if span.style.code {
        text = format!("`{text}`");
    }
    if span.style.italic {
        text = format!("*{text}*");
    }
    if span.style.bold {
        text = format!("**{text}**");
    }
    text
}

fn raw_content(name: &str) -> TokenSinkResult<()> {
    match name {
        "script" => TokenSinkResult::RawData(RawKind::ScriptData),
        "style" => TokenSinkResult::RawData(RawKind::Rawtext),
        "title" | "textarea" => TokenSinkResult::RawData(RawKind::Rcdata),
        _ => TokenSinkResult::Continue,
    }
}

fn link_target(tag: &Tag) -> Option<String> {
    let href = tag
        .attrs
        .iter()
        .find(|attribute| &*attribute.name.local == "href")?
        .value
        .trim();
    let url = url::Url::parse(href).ok()?;
    matches!(url.scheme(), "http" | "https" | "mailto")
        .then(|| url.to_string().replace('(', "%28").replace(')', "%29"))
}

#[cfg(test)]
mod tests {
    use super::to_markdown;

    #[test]
    fn renders_common_html_as_readable_markdown() {
        let html = "<h2>Update</h2><p>Hello <b>team</b>, <i>please</i> read \
                    <a href='https://example.test/info'>the details</a>.</p>\
                    <blockquote><p>Quoted reply</p></blockquote>\
                    <pre>let x = 1;\n  println!(x);</pre>";
        assert_eq!(
            to_markdown(html),
            "## Update\n\nHello **team**, *please* read [the details](https://example.test/info).\n\n> Quoted reply\n\n    let x = 1;\n      println!(x);"
        );
    }

    #[test]
    fn omits_hidden_content_images_and_terminal_controls() {
        let html = "<head><title>Hidden</title></head><p>Fish &amp; chips &#27; safe</p>\
                    <img src='https://tracker.test/open.gif' width='1' height='1'>\
                    <script>steal()</script><style>p { color: red }</style>\
                    <p><a href='javascript:alert(1)'>no script link</a></p>";
        let text = to_markdown(html);
        assert_eq!(text, "Fish & chips safe\n\nno script link");
        assert!(!text.contains("tracker.test"));
        assert!(!text.contains('\u{1b}'));
    }

    #[test]
    fn keeps_lists_and_line_breaks() {
        assert_eq!(
            to_markdown("<ul><li>First</li><li>Second<br>line</li></ul>"),
            "- First\n- Second\nline"
        );
    }
}
