//! HTML sanitization for message bodies, and plain text from them.

use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::LazyLock;

use html5ever::tendril::StrTendril;
use html5ever::tokenizer::states::RawKind;
use html5ever::tokenizer::{
    BufferQueue, Tag, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer, TokenizerOpts,
};

/// Whether a MIME type describes HTML, including optional charset parameters.
pub fn is_html_mime(mime_type: &str) -> bool {
    mime_type
        .split(';')
        .next()
        .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("text/html"))
}

/// Escape untrusted text before inserting it into HTML element content.
pub fn escape_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(character),
        }
    }
    out
}

/// The same sanitizer serves every body, so it is built once.
static SANITIZER: LazyLock<ammonia::Builder<'static>> = LazyLock::new(|| {
    let mut builder = ammonia::Builder::new();
    builder
        // Hidden elements go with their content, as in plain text; ammonia
        // already drops script and style that way.
        .add_clean_content_tags(HIDDEN)
        // A link may lead to the web or to an address, and nowhere else. Every
        // output keeps this, EML exports and quoted replies included, not only
        // the readers that filter links themselves.
        .url_schemes(HashSet::from(LINK_SCHEMES))
        .url_relative(ammonia::UrlRelative::Deny);
    builder
});

/// Remove active and hidden content while retaining safe HTML formatting.
/// Image tags may remain; renderers decide whether to display or fetch them.
/// Links keep only http, https and mailto URLs.
pub fn sanitize(html: &str) -> String {
    SANITIZER.clean(html).to_string()
}

/// Elements whose content is never read.
const HIDDEN: &[&str] = &[
    "head", "iframe", "noscript", "script", "style", "svg", "template", "textarea", "title",
];
/// Where a link may lead: the web or an address.
const LINK_SCHEMES: [&str; 3] = ["http", "https", "mailto"];
/// Elements that start a block of their own.
const BLOCKS: &[&str] = &[
    "address", "article", "aside", "div", "footer", "h1", "h2", "h3", "h4", "h5", "h6", "header",
    "main", "p", "section", "table", "tr",
];
/// Elements that have no content and no end tag.
const VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track",
    "wbr",
];
/// Deeper quotes and lists keep the prefix and indent of this level.
const MAX_TEXT_DEPTH: usize = 8;

/// Plain text from an HTML body, for recipients who can only receive text.
/// Blocks are separated by a blank line, and line breaks, list items and
/// quotes are kept. A link is written as its text followed by `<url>`. Hidden
/// content and images are dropped, and nothing is cut short.
pub(crate) fn to_text(html: &str) -> String {
    let tokenizer = Tokenizer::new(TextSink::default(), TokenizerOpts::default());
    let input = BufferQueue::default();
    input.push_back(StrTendril::from_slice(html));
    let _ = tokenizer.feed(&input);
    tokenizer.end();
    tokenizer.sink.0.take().finish()
}

#[derive(Default)]
struct TextSink(RefCell<TextBuilder>);

impl TokenSink for TextSink {
    type Handle = ();

    fn process_token(&self, token: Token, _line_number: u64) -> TokenSinkResult<()> {
        let mut text = self.0.borrow_mut();
        match token {
            Token::TagToken(tag) => return text.tag(&tag),
            Token::CharacterTokens(characters) => text.characters(&characters),
            _ => {}
        }
        TokenSinkResult::Continue
    }
}

#[derive(Default)]
struct TextBuilder {
    /// The finished blocks.
    out: String,
    /// The block being written.
    block: String,
    /// Whether a space separates the next word from the last one.
    space: bool,
    /// The hidden element being skipped.
    hidden: Option<String>,
    /// How many `<blockquote>` elements are open.
    quote_depth: usize,
    /// Open lists: `None` for bullets, or the next number.
    lists: Vec<Option<u32>>,
    /// The marker of the list item whose first block is being written.
    item: Option<String>,
    /// Whether the last block was a list item, so the next one follows it
    /// on the next line.
    after_item: bool,
    /// How many `<pre>` elements are open: their text keeps its spacing.
    pre_depth: usize,
    /// The open link's address, and where its text starts in `block`.
    link: Option<(String, usize)>,
}

impl TextBuilder {
    fn tag(&mut self, tag: &Tag) -> TokenSinkResult<()> {
        let name = &*tag.name;
        match tag.kind {
            TagKind::StartTag => {
                self.start(name, tag);
                if tag.self_closing && !VOID.contains(&name) {
                    self.end(name);
                }
                match name {
                    "script" => TokenSinkResult::RawData(RawKind::ScriptData),
                    "style" => TokenSinkResult::RawData(RawKind::Rawtext),
                    "title" | "textarea" => TokenSinkResult::RawData(RawKind::Rcdata),
                    _ => TokenSinkResult::Continue,
                }
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
                self.push_block("---", None);
            }
            "ul" | "ol" => {
                self.flush();
                self.lists.push((name == "ol").then_some(1));
            }
            "li" => {
                self.flush();
                self.item = Some(match self.lists.last_mut() {
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
                self.quote_depth += 1;
            }
            "pre" => {
                self.flush();
                self.pre_depth += 1;
            }
            "a" => {
                self.link = link_target(tag).map(|url| {
                    self.separate();
                    (url, self.block.len())
                });
            }
            "td" | "th" => self.space = !self.block.is_empty(),
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
            "pre" if self.pre_depth > 1 => self.pre_depth -= 1,
            "pre" if self.pre_depth == 1 => self.close_pre(),
            "a" => self.end_link(),
            "br" => self.line_break(),
            _ if BLOCKS.contains(&name) => self.flush(),
            _ => {}
        }
    }

    fn characters(&mut self, text: &str) {
        if self.hidden.is_some() {
            return;
        }
        if self.pre_depth > 0 {
            self.block.extend(
                text.chars()
                    .filter(|c| !c.is_control() || matches!(c, '\n' | '\t')),
            );
            return;
        }
        for c in text.chars() {
            if c.is_whitespace() {
                self.space = true;
            } else if !c.is_control() {
                self.separate();
                self.block.push(c);
            }
        }
    }

    /// Writes the space owed before the next word, unless the line is new.
    fn separate(&mut self) {
        if self.space && !self.block.is_empty() && !self.block.ends_with('\n') {
            self.block.push(' ');
        }
        self.space = false;
    }

    /// Ends the outermost `<pre>`, writing its text, spacing kept, as one
    /// block.
    fn close_pre(&mut self) {
        self.pre_depth = 0;
        // As in HTML, a newline right after <pre> is not content.
        let text = std::mem::take(&mut self.block);
        let text = text.strip_prefix('\n').unwrap_or(&text).trim_end();
        if !text.trim().is_empty() {
            let marker = self.item.take();
            self.push_block(text, marker);
        }
    }

    fn line_break(&mut self) {
        self.space = false;
        if self.pre_depth > 0 || !self.block.is_empty() {
            self.block.push('\n');
        }
    }

    fn end_link(&mut self) {
        let Some((url, start)) = self.link.take() else {
            return;
        };
        let text = self.block.get(start..).unwrap_or_default().trim();
        if text.is_empty() {
            self.separate();
            self.block.push('<');
            self.block.push_str(&url);
            self.block.push('>');
        } else if text != url && Some(text) != url.strip_prefix("mailto:") {
            self.block.push_str(" <");
            self.block.push_str(&url);
            self.block.push('>');
        }
    }

    fn flush(&mut self) {
        self.space = false;
        if let Some((_, start)) = &mut self.link {
            // A link that spans blocks keeps the text of its last one.
            *start = 0;
        }
        let block = std::mem::take(&mut self.block);
        let lines: Vec<&str> = block.lines().map(str::trim_end).collect();
        let text = lines.join("\n");
        let text = text.trim_matches('\n');
        if !text.is_empty() {
            let marker = self.item.take();
            self.push_block(text, marker);
        }
    }

    /// Adds a finished block, prefixed for the quotes and lists it is in.
    fn push_block(&mut self, text: &str, marker: Option<String>) {
        if !self.out.is_empty() {
            self.out.push_str(if self.after_item && marker.is_some() {
                "\n"
            } else {
                "\n\n"
            });
        }
        let quote = "> ".repeat(self.quote_depth.min(MAX_TEXT_DEPTH));
        let indent = "  ".repeat(self.lists.len().saturating_sub(1).min(MAX_TEXT_DEPTH));
        for (index, line) in text.lines().enumerate() {
            if index > 0 {
                self.out.push('\n');
            }
            self.out.push_str(&quote);
            if !self.lists.is_empty() {
                self.out.push_str(&indent);
                match (&marker, index) {
                    (Some(marker), 0) => {
                        self.out.push_str(marker);
                        self.out.push(' ');
                    }
                    _ => self.out.push_str("  "),
                }
            }
            self.out.push_str(line);
        }
        self.after_item = marker.is_some();
    }

    fn finish(mut self) -> String {
        self.end_link();
        // A <pre> still open ends with the body, unless hidden content left
        // open swallows that end as well: flush then writes what it held.
        if self.pre_depth > 0 && self.hidden.is_none() {
            self.close_pre();
        }
        self.flush();
        self.out
    }
}

/// The address of a link to the web or to a mailbox; any other is dropped.
fn link_target(tag: &Tag) -> Option<String> {
    let href = tag
        .attrs
        .iter()
        .find(|attribute| &*attribute.name.local == "href")?
        .value
        .trim();
    let url = reqwest::Url::parse(href).ok()?;
    LINK_SCHEMES
        .contains(&url.scheme())
        .then(|| url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_follows_paragraphs_and_line_breaks() {
        // The shape the desktop sends for what was typed.
        assert_eq!(
            to_text("<p>Hi Alex,<br>How are you?</p>\n<p>See you Thursday.</p>"),
            "Hi Alex,\nHow are you?\n\nSee you Thursday."
        );
        assert_eq!(
            to_text("<div>a   b\n\tc</div><div>Fish &amp; chips &lt;3</div>"),
            "a b c\n\nFish & chips <3"
        );
        assert_eq!(to_text("Bare <i>text</i>, no block"), "Bare text, no block");
    }

    #[test]
    fn text_keeps_lists_quotes_and_preformatted_lines() {
        assert_eq!(
            to_text("<ul><li>one</li><li>two<ol><li>first</li><li>second</li></ol></li></ul>"),
            "- one\n- two\n  1. first\n  2. second"
        );
        assert_eq!(
            to_text(
                "<p>Reply</p><blockquote><p>Original</p>\
                 <blockquote><p>Older</p></blockquote></blockquote>"
            ),
            "Reply\n\n> Original\n\n> > Older"
        );
        assert_eq!(
            to_text("<pre>\n  indented\n    code</pre><p>after</p>"),
            "  indented\n    code\n\nafter"
        );
        // Hostile nesting keeps the prefix of the deepest level kept.
        let deep = format!(
            "{}deep{}",
            "<blockquote>".repeat(40),
            "</blockquote>".repeat(40)
        );
        assert_eq!(to_text(&deep), format!("{}deep", "> ".repeat(8)));
    }

    #[test]
    fn a_pre_left_open_ends_with_the_body() {
        assert_eq!(
            to_text("<p>a</p><pre><pre>\n  x  \n    y  "),
            "a\n\n  x  \n    y"
        );
        // Hidden content left open keeps the <pre> open too, so its text
        // ends as an ordinary block.
        assert_eq!(to_text("<pre>\n  x  \n  y<script>z"), "  x\n  y");
    }

    #[test]
    fn text_names_where_links_go_and_drops_hidden_content() {
        assert_eq!(
            to_text(concat!(
                r#"<p>See <a href="https://proton.me">Proton</a>, "#,
                r#"<a href="https://example.org/">https://example.org/</a>, "#,
                r#"<a href="mailto:ana@example.org">ana@example.org</a> and "#,
                r#"<a href="javascript:evil()">this</a>.</p>"#
            )),
            "See Proton <https://proton.me/>, https://example.org/, ana@example.org and this."
        );
        assert_eq!(
            to_text(concat!(
                "<head><title>Title</title><style>p { color: red }</style></head>",
                "<script>document.write('<p>no</p>')</script>",
                r#"<p>Visible<img src="https://track.example/p.gif" alt="pixel"></p>"#
            )),
            "Visible"
        );
    }

    #[test]
    fn strips_active_content_keeps_formatting() {
        let dirty = r#"<b>hi</b><script>alert(1)</script><a href="javascript:evil()">x</a><img src="x" onerror="evil()"><p onclick="bad()">t</p>"#;
        let clean = sanitize(dirty);
        assert!(clean.contains("<b>hi</b>"));
        assert!(!clean.contains("<script"));
        assert!(!clean.to_lowercase().contains("javascript:"));
        assert!(!clean.to_lowercase().contains("onerror"));
        assert!(!clean.to_lowercase().contains("onclick"));
    }

    #[test]
    fn keeps_safe_links() {
        let clean = sanitize(r#"<a href="https://proton.me">link</a>"#);
        assert!(clean.contains("https://proton.me"));
    }

    #[test]
    fn links_lead_only_to_the_web_or_an_address() {
        let clean = sanitize(concat!(
            r#"<a href="https://proton.me">web</a><a href="mailto:ana@example.org">mail</a>"#,
            r#"<a href="ssh://host">shell</a><a href="magnet:?xt=urn:btih:x">torrent</a>"#,
            r#"<a href="webcal://cal.example/x">calendar</a><a href="sms:+123">text</a>"#,
            r#"<a href="/account/reset">relative</a><a href="ftp://files.example">files</a>"#,
        ));

        assert!(clean.contains(r#"href="https://proton.me""#), "{clean}");
        assert!(
            clean.contains(r#"href="mailto:ana@example.org""#),
            "{clean}"
        );
        for gone in [
            "ssh:",
            "magnet:",
            "webcal:",
            "sms:",
            "/account/reset",
            "ftp:",
        ] {
            assert!(!clean.contains(gone), "{gone} survived: {clean}");
        }
        // Only the destination goes; the words stay readable.
        for text in ["shell", "torrent", "calendar", "relative"] {
            assert!(clean.contains(text), "{text} was lost: {clean}");
        }
    }

    #[test]
    fn drops_hidden_elements_with_their_text() {
        let clean = sanitize(
            "<head><title>Hidden subject</title></head><p>Visible</p><noscript>Hidden</noscript>",
        );
        assert!(clean.contains("Visible"));
        assert!(!clean.contains("Hidden"));
    }

    #[test]
    fn recognizes_html_with_charset_parameters() {
        assert!(is_html_mime("text/html"));
        assert!(is_html_mime("Text/HTML; charset=UTF-8"));
        assert!(!is_html_mime("text/html-extra"));
        assert!(!is_html_mime("text/plain"));
    }
}
