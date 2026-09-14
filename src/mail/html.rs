//! Converts proton-core-sanitized HTML into Ruston's rich body model.
//! Preserves readable structure and inline styles without CSS or remote content.

use std::cell::RefCell;

use html5ever::tendril::StrTendril;
use html5ever::tokenizer::states::RawKind;
use html5ever::tokenizer::{
    BufferQueue, Tag, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer, TokenizerOpts,
};

use super::model::{BlockKind, RichBlock, RichBody, RichSpan};

/// Elements that start and end a paragraph.
const BLOCKS: &[&str] = &[
    "address",
    "article",
    "aside",
    "center",
    "dd",
    "div",
    "dl",
    "dt",
    "figcaption",
    "figure",
    "footer",
    "form",
    "header",
    "main",
    "nav",
    "p",
    "section",
    "table",
    "tbody",
    "tfoot",
    "thead",
    "tr",
];

/// Elements whose content is never shown.
const HIDDEN: &[&str] = &[
    "head", "noscript", "script", "style", "template", "textarea", "title",
];

/// Elements that have no content and no end tag.
const VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track",
    "wbr",
];

/// Parses a sanitized HTML body.
pub(super) fn parse(html: &str) -> RichBody {
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

#[derive(Default)]
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
    link: Option<String>,
    heading: Option<u8>,
    /// Open lists, innermost last: the next number of ordered ones.
    lists: Vec<Option<u32>>,
    /// Marker for the next block of the current list item; empty once used.
    item: Option<String>,
    quote_depth: u8,
    pre: u16,
    pre_text: String,
    /// Element whose content is being skipped.
    hidden: Option<String>,
}

impl Builder {
    fn tag(&mut self, tag: &Tag) -> TokenSinkResult<()> {
        let name = &*tag.name;
        match tag.kind {
            TagKind::StartTag => {
                self.start(name, tag);
                if tag.self_closing && !VOID.contains(&name) {
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
        if HIDDEN.contains(&name) {
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
                self.lists.push(None);
            }
            "ol" => {
                self.flush();
                self.lists.push(Some(1));
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
            "a" => self.link = link_target(tag),
            // Table cells read left to right, separated like words.
            "td" | "th" => self.separate(),
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
            "br" => self.line_break(),
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.flush();
                self.heading = None;
            }
            "ul" | "ol" => {
                self.flush();
                self.lists.pop();
                self.item = None;
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
            _ if BLOCKS.contains(&name) => self.flush(),
            _ => {}
        }
    }

    fn text(&mut self, text: &str) {
        if self.hidden.is_some() {
            return;
        }
        if self.pre > 0 {
            self.pre_text.push_str(text);
            return;
        }

        // Collapse whitespace runs like a browser does.
        for c in text.chars() {
            if c.is_whitespace() {
                self.separate();
                continue;
            }
            self.space = false;
            self.push_char(c);
        }
    }

    /// Writes pending space with the preceding run's style; `flush` drops it.
    fn separate(&mut self) {
        if !self.space && self.wants_space() {
            self.push_char(' ');
            self.space = true;
        }
    }

    /// Whether a space may separate what comes next from the text so far.
    fn wants_space(&self) -> bool {
        self.spans
            .last()
            .is_some_and(|span| !span.text.ends_with('\n'))
    }

    fn push_char(&mut self, c: char) {
        let (strong, emphasis, code, struck) = (
            self.strong > 0,
            self.emphasis > 0,
            self.code > 0,
            self.struck > 0,
        );
        match self.spans.last_mut() {
            Some(span)
                if span.strong == strong
                    && span.emphasis == emphasis
                    && span.code == code
                    && span.struck == struck
                    && span.link == self.link =>
            {
                span.text.push(c);
            }
            _ => self.spans.push(RichSpan {
                text: c.to_string(),
                strong,
                emphasis,
                code,
                struck,
                link: self.link.clone(),
            }),
        }
    }

    fn line_break(&mut self) {
        if self.pre > 0 {
            self.pre_text.push('\n');
        } else if !self.spans.is_empty() {
            self.push_char('\n');
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
        self.push(BlockKind::Image {
            description: description.to_owned(),
        });
    }

    fn next_marker(&mut self) -> String {
        match self.lists.last_mut() {
            Some(Some(number)) => {
                let marker = format!("{number}.");
                *number = number.saturating_add(1);
                marker
            }
            _ => "•".to_owned(),
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
            self.item = Some(String::new());
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
        self.blocks.push(RichBlock {
            kind,
            quote_depth: self.quote_depth,
        });
    }

    fn finish(mut self) -> RichBody {
        self.flush();
        if self.pre > 0 {
            self.pre = 1;
            self.end_pre();
        }
        RichBody {
            blocks: self.blocks,
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
fn link_target(tag: &Tag) -> Option<String> {
    let url = url::Url::parse(attribute(tag, "href")?.trim()).ok()?;
    matches!(url.scheme(), "http" | "https" | "mailto").then(|| url.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One line per block: kind, quote depth as `>`, and its text.
    fn outline(html: &str) -> Vec<String> {
        let text = |spans: &[RichSpan]| spans.iter().map(|s| s.text.as_str()).collect::<String>();
        parse(html)
            .blocks
            .iter()
            .map(|block| {
                let line = match &block.kind {
                    BlockKind::Paragraph(spans) => format!("p {}", text(spans)),
                    BlockKind::Heading { level, spans } => format!("h{level} {}", text(spans)),
                    BlockKind::ListItem {
                        marker,
                        depth,
                        spans,
                    } => format!("li{depth} {marker} {}", text(spans)),
                    BlockKind::Preformatted(content) => format!("pre {content}"),
                    BlockKind::Image { description } => format!("img {description}"),
                    BlockKind::Rule => "hr".to_owned(),
                };
                format!("{}{line}", ">".repeat(usize::from(block.quote_depth)))
            })
            .collect()
    }

    fn spans(html: &str) -> Vec<RichSpan> {
        match parse(html)
            .blocks
            .into_iter()
            .next()
            .map(|block| block.kind)
        {
            Some(BlockKind::Paragraph(spans)) => spans,
            other => panic!("expected a paragraph, got {other:?}"),
        }
    }

    #[test]
    fn paragraphs_and_line_breaks() {
        let html = "<div><p>Hello <b>Alex</b>,</p><p>Line one<br>Line two</p></div>";

        assert_eq!(outline(html), ["p Hello Alex,", "p Line one\nLine two"]);
    }

    #[test]
    fn headings_lists_and_quotes_keep_their_structure() {
        let html = "<h1>Title</h1><ul><li>One</li><li>Two<ol><li>Nested</li></ol></li></ul>\
                    <hr><blockquote><p>Quoted</p><blockquote>Deeper</blockquote></blockquote>";

        assert_eq!(
            outline(html),
            [
                "h1 Title",
                "li1 • One",
                "li1 • Two",
                "li2 1. Nested",
                "hr",
                ">p Quoted",
                ">>p Deeper",
            ]
        );
    }

    #[test]
    fn inline_styles_become_spans() {
        let spans = spans(
            "<p>Plain <strong>bold</strong> <em>italic</em> <code>code</code> <s>old</s> \
             <a href=\"https://example.com/x\">link</a></p>",
        );
        let styled = |word: &str| spans.iter().find(|s| s.text.trim() == word).unwrap();

        assert!(!styled("Plain").strong && styled("Plain").link.is_none());
        assert!(styled("bold").strong);
        assert!(styled("italic").emphasis);
        assert!(styled("code").code);
        assert!(styled("old").struck);
        assert_eq!(
            styled("link").link.as_deref(),
            Some("https://example.com/x")
        );
    }

    #[test]
    fn only_web_and_mail_links_stay_clickable() {
        let spans = spans(
            "<p><a href=\"javascript:alert(1)\">a</a> <a href=\"/relative\">b</a> \
             <a href=\"mailto:team@example.org\">c</a> <a href=\"ftp://example.com\">d</a></p>",
        );
        let link = |word: &str| {
            spans
                .iter()
                .find(|s| s.text.trim() == word)
                .and_then(|s| s.link.clone())
        };

        assert_eq!(link("a"), None);
        assert_eq!(link("b"), None);
        assert_eq!(link("c").as_deref(), Some("mailto:team@example.org"));
        assert_eq!(link("d"), None);
    }

    #[test]
    fn preformatted_text_keeps_its_spacing() {
        let html = "<pre>\n  let x = 1;\n    indented\n</pre><p>After</p>";

        assert_eq!(outline(html), ["pre   let x = 1;\n    indented", "p After"]);
    }

    #[test]
    fn hidden_content_is_dropped_and_entities_decoded() {
        let html = "<head><title>Subject</title><style>p { color: red } a < b</style></head>\
                    <p>Fish &amp; chips &lt;3 &copy; &#x263A;</p>\
                    <script>document.write(\"<p>x</p>\")</script>";

        assert_eq!(outline(html), ["p Fish & chips <3 © ☺"]);
    }

    #[test]
    fn images_keep_only_their_description() {
        let html = "<p>Logo <img src=\"https://tracker.example/p.gif\" alt=\"\"> text</p>\
                    <img src=\"cid:logo\" alt=\" Company logo \">";

        assert_eq!(outline(html), ["p Logo text", "img Company logo"]);
    }

    #[test]
    fn tables_read_row_by_row() {
        let html = "<table><tr><td>Name</td><td>Value</td></tr>\
                    <tr><th>A</th><th>B</th></tr></table>";

        assert_eq!(outline(html), ["p Name Value", "p A B"]);
    }

    #[test]
    fn malformed_or_empty_input_is_handled() {
        assert!(parse("").blocks.is_empty());
        assert_eq!(outline("Text <b unterminated"), ["p Text"]);
        assert_eq!(outline("Plain text only"), ["p Plain text only"]);
        assert_eq!(outline(&"<b>".repeat(70_000)), Vec::<String>::new());
    }

    #[test]
    fn text_follows_reading_order() {
        let body = parse("<p>One <b>two</b></p><img alt=\"Pic\"><pre>code</pre>");

        assert_eq!(body.plain_text(), "One two\n[Image: Pic]\ncode\n");
    }

    #[test]
    fn a_styled_run_never_owns_the_space_beside_it() {
        let body =
            parse("<p>and a <s>withdrawn</s> point, plus <code>settings.json</code> here</p>");
        let BlockKind::Paragraph(spans) = &body.blocks[0].kind else {
            panic!("expected a paragraph");
        };
        for span in spans {
            assert!(
                !(span.code || span.struck) || span.text.trim() == span.text,
                "{span:?} carries a space its background or strikethrough would cover"
            );
        }
        assert_eq!(
            spans
                .iter()
                .map(|span| span.text.as_str())
                .collect::<String>(),
            "and a withdrawn point, plus settings.json here"
        );
    }

    #[test]
    fn a_space_inside_a_styled_run_stays_inside_it() {
        let body = parse("<p><code>cargo test</code></p>");
        let BlockKind::Paragraph(spans) = &body.blocks[0].kind else {
            panic!("expected a paragraph");
        };
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].text, "cargo test");
        assert!(spans[0].code);
    }
}
