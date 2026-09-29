//! Readable terminal text from a decrypted HTML message.

use std::cell::RefCell;
use std::rc::Rc;

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
const MAX_RENDER_DEPTH: usize = 8;
const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
const TRUNCATION_NOTICE: &str =
    "\n\n[Message truncated. Use --format raw or --output to read the full body.]";
const MAX_CONTENT_BYTES: usize = MAX_OUTPUT_BYTES - TRUNCATION_NOTICE.len();
const INPUT_CHUNK_BYTES: usize = 8 * 1024;

pub(super) fn to_markdown(html: &str) -> String {
    let tokenizer = Tokenizer::new(Sink::default(), TokenizerOpts::default());
    let input = BufferQueue::default();
    let mut remaining = html;
    while !remaining.is_empty() && !tokenizer.sink.0.borrow().truncated {
        let mut end = remaining.len().min(INPUT_CHUNK_BYTES);
        while !remaining.is_char_boundary(end) {
            end -= 1;
        }
        input.push_back(StrTendril::from_slice(&remaining[..end]));
        let _ = tokenizer.feed(&input);
        remaining = &remaining[end..];
    }
    tokenizer.end();
    std::mem::take(&mut *tokenizer.sink.0.borrow_mut()).finish()
}

#[derive(Default)]
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

#[derive(Clone, Default, PartialEq, Eq)]
struct Style {
    bold: bool,
    italic: bool,
    code: bool,
    link: Option<Rc<str>>,
}

struct Span {
    text: String,
    style: Style,
}

/// Keep output and temporary text within the same byte budget, including UTF-8.
struct BoundedText {
    text: String,
    limit: usize,
    truncated: bool,
}

impl Default for BoundedText {
    fn default() -> Self {
        Self::new(MAX_CONTENT_BYTES)
    }
}

impl BoundedText {
    fn new(limit: usize) -> Self {
        Self {
            text: String::new(),
            limit,
            truncated: false,
        }
    }

    fn remaining(&self) -> usize {
        self.limit.saturating_sub(self.text.len())
    }

    fn push(&mut self, text: &str) {
        if self.truncated {
            return;
        }
        let mut end = text.len().min(self.remaining());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        self.text.push_str(&text[..end]);
        self.truncated = end != text.len();
    }

    fn push_char(&mut self, c: char) {
        self.push(c.encode_utf8(&mut [0; 4]));
    }
}

#[derive(Default)]
struct Builder {
    output: BoundedText,
    prior_list_item: bool,
    truncated: bool,
    spans: Vec<Span>,
    pending_bytes: usize,
    pending_space: bool,
    hidden: Option<String>,
    heading: Option<usize>,
    quote_depth: usize,
    lists: Vec<Option<u32>>,
    omitted_list_depth: usize,
    item_marker: Option<String>,
    pre_depth: usize,
    pre_text: BoundedText,
    bold: usize,
    italic: usize,
    code: usize,
    link: Option<Rc<str>>,
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
                self.start_list(None);
            }
            "ol" => {
                self.flush();
                self.start_list(Some(1));
            }
            "li" => {
                self.flush();
                self.item_marker = Some(
                    match self
                        .lists
                        .last_mut()
                        .filter(|_| self.omitted_list_depth == 0)
                    {
                        Some(Some(number)) => {
                            let marker = format!("{number}.");
                            *number = number.saturating_add(1);
                            marker
                        }
                        _ => "-".to_owned(),
                    },
                );
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
                if self.omitted_list_depth > 0 {
                    self.omitted_list_depth -= 1;
                } else {
                    self.lists.pop();
                }
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
            for c in text
                .chars()
                .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
            {
                self.pre_text.push_char(c);
                if self.pre_text.truncated {
                    self.truncated = true;
                    break;
                }
            }
            return;
        }
        let style = Style {
            bold: self.bold > 0,
            italic: self.italic > 0,
            code: self.code > 0,
            link: self.link.clone(),
        };
        for run in text.split_inclusive(|c: char| c.is_whitespace() || c.is_control()) {
            if self.truncated {
                break;
            }
            let last = run.chars().next_back().unwrap();
            let separator = last.is_whitespace() || last.is_control();
            let word = if separator {
                &run[..run.len() - last.len_utf8()]
            } else {
                run
            };
            if !word.is_empty() {
                self.consume_pending_space();
                self.push_text(word, style.clone());
            }
            if last.is_whitespace() {
                self.pending_space = self
                    .spans
                    .last()
                    .is_some_and(|span| !span.text.ends_with('\n'));
            }
        }
    }

    fn push_text(&mut self, text: &str, style: Style) {
        if self.truncated || text.is_empty() {
            return;
        }
        let new_span = self.spans.last().is_none_or(|span| span.style != style);
        // Count span storage as well as text, so many tiny style changes cannot
        // allocate an unbounded staging vector before the next block flush.
        let overhead = if new_span {
            std::mem::size_of::<Span>() + style.link.as_ref().map_or(0, |link| link.len())
        } else {
            0
        };
        let available = self.output.remaining().saturating_sub(self.pending_bytes);
        let mut end = text.len().min(available.saturating_sub(overhead));
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        if end == 0 {
            self.truncated = true;
            return;
        }
        self.pending_bytes += overhead + end;
        match self.spans.last_mut() {
            Some(span) if span.style == style => {
                span.text.push_str(&text[..end]);
                // Equal adjacent anchors can have separate allocations. Keep
                // the current one to avoid repeatedly comparing URL contents.
                span.style.link = style.link;
            }
            _ => self.spans.push(Span {
                text: text[..end].to_owned(),
                style,
            }),
        }
        self.truncated |= end < text.len();
    }

    fn consume_pending_space(&mut self) {
        if self.pending_space && !self.spans.is_empty() {
            self.push_text(
                " ",
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
            self.pre_text.push_char('\n');
            self.truncated |= self.pre_text.truncated;
        } else if !self.spans.is_empty() {
            self.push_text("\n", Style::default());
        }
    }

    fn flush(&mut self) {
        self.pending_space = false;
        self.pending_bytes = 0;
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

        let rendered = render_spans(std::mem::take(&mut self.spans), self.output.remaining());
        self.truncated |= rendered.truncated;
        let text = rendered.text;
        let (text, list_item) = if let Some(level) = self.heading {
            (format!("{} {text}", "#".repeat(level)), false)
        } else if let Some(marker) = self.item_marker.as_mut() {
            let depth = self.lists.len().saturating_sub(1).min(MAX_RENDER_DEPTH - 1);
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
        let buffered = std::mem::take(&mut self.pre_text);
        self.truncated |= buffered.truncated;
        let text = buffered
            .text
            .strip_prefix('\n')
            .unwrap_or(&buffered.text)
            .trim_end_matches('\n');
        if !text.trim().is_empty() {
            let mut indented = BoundedText::new(self.output.remaining());
            for (index, line) in text.lines().enumerate() {
                if index > 0 {
                    indented.push("\n");
                }
                indented.push("    ");
                indented.push(line);
                if indented.truncated {
                    break;
                }
            }
            self.truncated |= indented.truncated;
            self.push_block(indented.text, false);
        }
    }

    fn push_block(&mut self, text: String, list_item: bool) {
        let prefix = "> ".repeat(self.quote_depth.min(MAX_RENDER_DEPTH));
        if !self.output.text.is_empty() {
            self.output.push(if self.prior_list_item && list_item {
                "\n"
            } else {
                "\n\n"
            });
        }
        for (index, line) in text.lines().enumerate() {
            if index > 0 {
                self.output.push("\n");
            }
            self.output.push(&prefix);
            self.output.push(line);
            if self.output.truncated {
                break;
            }
        }
        self.prior_list_item = list_item;
        self.truncated |= self.output.truncated;
    }

    fn start_list(&mut self, number: Option<u32>) {
        if self.lists.len() < MAX_RENDER_DEPTH {
            self.lists.push(number);
        } else {
            self.omitted_list_depth = self.omitted_list_depth.saturating_add(1);
        }
    }

    fn finish(mut self) -> String {
        self.flush();
        if self.pre_depth > 0 {
            self.pre_depth = 1;
            self.end_pre();
        }
        if self.truncated {
            self.output.text.push_str(TRUNCATION_NOTICE);
        }
        self.output.text
    }
}

fn render_spans(spans: Vec<Span>, limit: usize) -> BoundedText {
    let mut output = BoundedText::new(limit);
    let mut link: Option<Rc<str>> = None;
    for span in spans {
        if span.style.link != link {
            if let Some(url) = link.take() {
                output.push("](");
                output.push(&url);
                output.push(")");
            }
            if span.style.link.is_some() {
                output.push("[");
            }
        }
        // Refresh the representative even when equal adjacent anchors merge;
        // subsequent styled spans then compare a shared pointer.
        link = span.style.link.clone();
        render_span(span, &mut output);
        if output.truncated {
            break;
        }
    }
    if let Some(url) = link {
        output.push("](");
        output.push(&url);
        output.push(")");
    }
    output
}

fn render_span(span: Span, output: &mut BoundedText) {
    if span.style.bold {
        output.push("**");
    }
    if span.style.italic {
        output.push("*");
    }
    if span.style.code {
        output.push("`");
    }
    output.push(&span.text);
    if span.style.code {
        output.push("`");
    }
    if span.style.italic {
        output.push("*");
    }
    if span.style.bold {
        output.push("**");
    }
}

fn raw_content(name: &str) -> TokenSinkResult<()> {
    match name {
        "script" => TokenSinkResult::RawData(RawKind::ScriptData),
        "style" => TokenSinkResult::RawData(RawKind::Rawtext),
        "title" | "textarea" => TokenSinkResult::RawData(RawKind::Rcdata),
        _ => TokenSinkResult::Continue,
    }
}

fn link_target(tag: &Tag) -> Option<Rc<str>> {
    let href = tag
        .attrs
        .iter()
        .find(|attribute| &*attribute.name.local == "href")?
        .value
        .trim();
    if href.len() > MAX_CONTENT_BYTES {
        return None;
    }
    let url = url::Url::parse(href).ok()?;
    if !matches!(url.scheme(), "http" | "https" | "mailto") {
        return None;
    }
    let target = url.to_string().replace('(', "%28").replace(')', "%29");
    (target.len() <= MAX_CONTENT_BYTES).then(|| Rc::from(target))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_adjacent_links_refresh_the_shared_target_when_runs_merge() {
        let target: Rc<str> = Rc::from(format!("https://example.test/{}", "x".repeat(32 * 1024)));
        let next: Rc<str> = Rc::from(target.as_ref());
        assert!(!Rc::ptr_eq(&target, &next));
        let mut builder = Builder {
            link: Some(target),
            ..Builder::default()
        };
        builder.text("first");
        builder.link = Some(next.clone());
        builder.text("second");
        assert_eq!(builder.spans.len(), 1);
        assert!(Rc::ptr_eq(
            builder.spans[0].style.link.as_ref().unwrap(),
            &next
        ));
    }

    #[test]
    fn batched_runs_obey_the_staging_budget_at_utf8_boundaries() {
        let text = "é猫";
        let overhead = std::mem::size_of::<Span>();
        for (budget, expected) in [(overhead - 1, ""), (overhead + 1, ""), (overhead + 4, "é")] {
            let mut builder = Builder {
                output: BoundedText::new(budget),
                ..Builder::default()
            };
            builder.text(text);
            assert!(builder.truncated);
            assert!(builder.pending_bytes <= budget);
            let visible: String = builder
                .spans
                .iter()
                .map(|span| span.text.as_str())
                .collect();
            assert_eq!(visible, expected);
        }
    }

    #[test]
    fn text_runs_preserve_controls_whitespace_and_equal_link_grouping() {
        let html = "<p>\u{2003}<a href='https://example.test/'>α\t β</a>\
                    <a href='https://example.test/'><b>猫 &amp; <i>é</i></b>\u{1b}tail</a></p>";
        assert_eq!(
            to_markdown(html),
            "[α β**猫** **&** ***é***tail](https://example.test/)"
        );
    }

    #[test]
    #[ignore = "manual scaling benchmark with fictional long HTML links"]
    fn measure_long_link_conversion_scaling() {
        for size in [32 * 1024, 64 * 1024, 128 * 1024] {
            let target = format!("https://example.test/{}", "x".repeat(size));
            let label = "z".repeat(size);
            let html = format!("<a href='{target}'>x</a><a href='{target}'>{label}</a>");
            let sanitized = ruston_core::html::sanitize(&html);
            let start = std::time::Instant::now();
            let output = to_markdown(&sanitized);
            eprintln!("CLI: URL/label={size} bytes in {:?}", start.elapsed());
            assert_eq!(output, format!("[x{label}]({target})"));
        }
    }

    #[test]
    fn sanitized_deep_quotes_do_not_amplify_each_paragraph() {
        let mut previous_size = None;
        for depth in [1024, 2048, 4096] {
            let html = format!(
                "{}{}{}",
                "<blockquote>".repeat(depth),
                "<p>x</p>".repeat(depth),
                "</blockquote>".repeat(depth)
            );
            let sanitized = ruston_core::html::sanitize(&html);
            let output = to_markdown(&sanitized);
            eprintln!(
                "depth={depth}, input={} bytes, output={} bytes",
                sanitized.len(),
                output.len()
            );
            assert!(output.len() <= sanitized.len() * 4);
            if let Some(previous_size) = previous_size {
                assert!(output.len() <= previous_size * 2 + 32);
            }
            previous_size = Some(output.len());
        }
    }

    #[test]
    fn sanitized_deep_lists_have_bounded_indentation_and_linear_growth() {
        let mut previous_size = None;
        for depth in [512, 1024, 2048] {
            let html = format!(
                "{}{}{}",
                "<ul><li>".repeat(depth),
                "<p>x</p>".repeat(depth),
                "</li></ul>".repeat(depth)
            );
            let sanitized = ruston_core::html::sanitize(&html);
            let output = to_markdown(&sanitized);
            assert!(
                output
                    .lines()
                    .all(|line| line.chars().take_while(|c| *c == ' ').count()
                        <= 2 * (MAX_RENDER_DEPTH - 1) + 1)
            );
            assert!(output.len() <= sanitized.len() * 4);
            if let Some(previous_size) = previous_size {
                assert!(output.len() <= previous_size * 2 + 32);
            }
            previous_size = Some(output.len());
        }
    }

    #[test]
    fn flattened_nesting_unwinds_to_the_correct_outer_level() {
        let html = format!(
            "{}<p>deep</p>{}<p>outer</p></blockquote><p>outside</p>",
            "<blockquote>".repeat(128),
            "</blockquote>".repeat(127)
        );
        assert_eq!(
            to_markdown(&html),
            format!(
                "{}deep\n\n> outer\n\noutside",
                "> ".repeat(MAX_RENDER_DEPTH)
            )
        );

        let html = format!(
            "<ol><li>first</li><li>{}deep{}</li><li>last</li></ol>",
            "<ul><li>".repeat(128),
            "</li></ul>".repeat(128)
        );
        let output = to_markdown(&html);
        assert!(output.starts_with("1. first\n"));
        assert!(output.ends_with("3. last"));
        assert!(output.contains(&format!("{}- deep", "  ".repeat(MAX_RENDER_DEPTH - 1))));
    }

    #[test]
    fn output_limit_covers_large_blocks_preformatted_text_and_many_blocks() {
        for html in [
            format!("<p>{}</p><p>AFTER_LIMIT</p>", "é".repeat(MAX_OUTPUT_BYTES)),
            format!(
                "<pre>{}</pre><p>AFTER_LIMIT</p>",
                "é\n".repeat(MAX_OUTPUT_BYTES / 2)
            ),
            format!(
                "{}<p>AFTER_LIMIT</p>",
                "<blockquote><p>text</p></blockquote>".repeat(MAX_OUTPUT_BYTES / 4)
            ),
        ] {
            let output = to_markdown(&html);
            assert!(output.len() <= MAX_OUTPUT_BYTES);
            assert!(output.ends_with(TRUNCATION_NOTICE));
            assert_eq!(output.matches(TRUNCATION_NOTICE).count(), 1);
            assert!(!output.contains("AFTER_LIMIT"));
        }
    }

    #[test]
    fn temporary_buffers_and_list_stack_stop_growing_at_their_limits() {
        let mut builder = Builder::default();
        for index in 0..MAX_OUTPUT_BYTES {
            builder.bold = index % 2;
            builder.text("x");
            if builder.truncated {
                break;
            }
        }
        assert!(builder.truncated);
        assert!(builder.pending_bytes <= MAX_CONTENT_BYTES);
        assert!(builder.spans.len() * std::mem::size_of::<Span>() <= MAX_CONTENT_BYTES);
        assert!(builder.finish().ends_with(TRUNCATION_NOTICE));

        let mut builder = Builder {
            pre_depth: 1,
            ..Builder::default()
        };
        builder.text(&"x".repeat(MAX_OUTPUT_BYTES * 2));
        assert!(builder.truncated);
        assert!(builder.pre_text.text.len() <= MAX_CONTENT_BYTES);

        let mut builder = Builder::default();
        for _ in 0..1024 {
            builder.start_list(Some(1));
        }
        assert_eq!(builder.lists.len(), MAX_RENDER_DEPTH);
        assert_eq!(builder.omitted_list_depth, 1024 - MAX_RENDER_DEPTH);
    }

    #[test]
    fn bounded_text_preserves_utf8_and_link_targets_are_shared() {
        let mut text = BoundedText::new(4);
        text.push("é猫");
        assert_eq!(text.text, "é");
        assert!(text.truncated);

        let target: Rc<str> = Rc::from(format!("https://example.test/{}", "x".repeat(32 * 1024)));
        let mut builder = Builder {
            link: Some(target.clone()),
            ..Builder::default()
        };
        builder.text(&"a".repeat(64 * 1024));
        assert_eq!(builder.spans.len(), 1);
        assert!(Rc::ptr_eq(
            builder.spans[0].style.link.as_ref().unwrap(),
            &target
        ));
        let output = builder.finish();
        assert!(output.contains(target.as_ref()));
        assert!(!output.ends_with(TRUNCATION_NOTICE));
    }

    #[test]
    fn chunked_html_preserves_unicode_entities_and_links() {
        let text = "é".repeat(INPUT_CHUNK_BYTES / 2 - 2);
        let target = format!("https://example.test/{}", "x".repeat(INPUT_CHUNK_BYTES * 2));
        let html = format!("<p>{text}🐈 &amp; tail <a href='{target}'>link</a></p>");
        assert_eq!(
            to_markdown(&html),
            format!("{text}🐈 & tail [link]({target})")
        );

        let target = format!("https://example.test/{}", "x".repeat(MAX_OUTPUT_BYTES));
        assert_eq!(
            to_markdown(&format!("<a href='{target}'>visible label</a>")),
            "visible label"
        );
    }

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
