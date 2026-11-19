//! Readable terminal text from a decrypted HTML message: the blocks core
//! reads from it, written as Markdown.

use ruston_core::html::rich::{self, BlockKind, ListMarker, RichBlock, RichSpan};

/// Quotes and lists shown within one another; deeper ones keep the last
/// prefix and indent, so text never leaves the screen.
const MAX_RENDER_DEPTH: usize = 8;
const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
const TRUNCATION_NOTICE: &str =
    "\n\n[Message truncated. Use --format raw or --output to read the full body.]";
const MAX_CONTENT_BYTES: usize = MAX_OUTPUT_BYTES - TRUNCATION_NOTICE.len();

pub(super) fn to_markdown(html: &str) -> String {
    // Within the output's own bound, so a body cut short is not read whole.
    let body = rich::parse_within(html, MAX_CONTENT_BYTES);
    let mut writer = Writer::default();
    for block in &body.blocks {
        writer.block(block);
        if writer.output.truncated {
            break;
        }
    }
    // The terminal shows no image, described or not, so it says how many.
    let images = body.undescribed_images
        + body
            .blocks
            .iter()
            .filter(|block| !block.hidden && matches!(block.kind, BlockKind::Image { .. }))
            .count();
    if images > 0 && !writer.output.truncated {
        let note = if images == 1 {
            "[1 image not shown]".to_owned()
        } else {
            format!("[{images} images not shown]")
        };
        writer.push_block(&note, 0, false);
    }
    let mut text = writer.output.text;
    if body.truncated || writer.output.truncated {
        text.push_str(TRUNCATION_NOTICE);
    }
    text
}

/// Keep output within the same byte budget, including UTF-8.
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

    fn push(&mut self, text: &str) {
        if self.truncated {
            return;
        }
        let end = text.floor_char_boundary(self.limit.saturating_sub(self.text.len()));
        self.text.push_str(&text[..end]);
        self.truncated = end != text.len();
    }
}

/// One message's text, written block by block.
#[derive(Default)]
struct Writer {
    /// The text written so far, within `MAX_CONTENT_BYTES`.
    output: BoundedText,
    /// Whether the last block written began a list item: the next item's
    /// first block then follows it on the next line, not after a blank one.
    prior_list_item: bool,
    /// The columns the marker each list level wrote last takes with the space
    /// after it, which the later blocks of its item line up under.
    widths: [usize; MAX_RENDER_DEPTH],
}

impl Writer {
    fn block(&mut self, block: &RichBlock) {
        // What the sender hid stays hidden, as it would in a browser.
        if block.hidden {
            return;
        }
        let (text, list_item) = match &block.kind {
            BlockKind::Paragraph(spans) => (markdown(spans), false),
            BlockKind::Heading { level, spans } => (
                format!("{} {}", "#".repeat(usize::from(*level)), markdown(spans)),
                false,
            ),
            BlockKind::ListItem {
                marker,
                depth,
                spans,
            } => self.list_item(*marker, *depth, &markdown(spans)),
            BlockKind::Preformatted(text) => (
                text.lines()
                    .map(|line| format!("    {line}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
                false,
            ),
            // The terminal shows no images, nor says what they were.
            BlockKind::Image { .. } => return,
            BlockKind::Rule => ("---".to_owned(), false),
        };
        self.push_block(&text, usize::from(block.quote_depth), list_item);
    }

    /// A list item's block, its marker first when it starts the item, and
    /// whether it does. Each line of the item starts under the text of its
    /// first.
    fn list_item(&mut self, marker: ListMarker, depth: u8, text: &str) -> (String, bool) {
        let level = usize::from(depth).clamp(1, MAX_RENDER_DEPTH) - 1;
        let indent = "  ".repeat(level);
        let first = match marker {
            ListMarker::Bullet => Some("-".to_owned()),
            ListMarker::Number(number) => Some(format!("{number}.")),
            ListMarker::Continuation => None,
        };
        if let Some(first) = &first {
            self.widths[level] = first.len() + 1;
        }
        let under = format!("{indent}{}", " ".repeat(self.widths[level]));
        let mut item = String::new();
        for (index, line) in text.lines().enumerate() {
            match &first {
                Some(first) if index == 0 => {
                    item.push_str(&indent);
                    item.push_str(first);
                    item.push(' ');
                }
                _ => {
                    if index > 0 {
                        item.push('\n');
                    }
                    item.push_str(&under);
                }
            }
            item.push_str(line);
        }
        (item, first.is_some())
    }

    fn push_block(&mut self, text: &str, quote_depth: usize, list_item: bool) {
        let prefix = "> ".repeat(quote_depth.min(MAX_RENDER_DEPTH));
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
    }
}

/// `spans` as Markdown: each word within its style's markers, and each run of
/// one link within its brackets, line by line, with the spaces around it left
/// outside.
fn markdown(spans: &[RichSpan]) -> String {
    let shown: Vec<RichSpan> = spans.iter().filter(|span| !span.hidden).cloned().collect();
    let mut out = String::new();
    let mut rest = &shown[..];
    while let Some(first) = rest.first() {
        let run = rest
            .iter()
            .take_while(|span| span.link == first.link)
            .count();
        let text = styled(&rest[..run]);
        // A target too long for the output reads as the text alone.
        match first
            .link
            .as_deref()
            .map(|link| link.replace('(', "%28").replace(')', "%29"))
            .filter(|target| target.len() <= MAX_CONTENT_BYTES)
        {
            Some(target) => {
                for (index, line) in text.split('\n').enumerate() {
                    if index > 0 {
                        out.push('\n');
                    }
                    let words = line.trim();
                    if words.is_empty() {
                        out.push_str(line);
                        continue;
                    }
                    let start = line.len() - line.trim_start().len();
                    out.push_str(&line[..start]);
                    out.push('[');
                    out.push_str(words);
                    out.push_str("](");
                    out.push_str(&target);
                    out.push(')');
                    out.push_str(&line[start + words.len()..]);
                }
            }
            None => out.push_str(&text),
        }
        rest = &rest[run..];
    }
    // Hidden text at the end may leave the space before it.
    out.truncate(out.trim_end().len());
    out
}

/// The text of `spans` with each word, never a space, within the markers of
/// its style, so every one reads as Markdown.
fn styled(spans: &[RichSpan]) -> String {
    let mut out = String::new();
    for span in spans {
        // What the sender's style makes a run counts as its tags do.
        let markers: Vec<&str> = [
            (span.struck || span.styled.struck, "~~"),
            (span.strong || span.styled.strong, "**"),
            (span.emphasis || span.styled.emphasis, "*"),
            (span.code, "`"),
        ]
        .into_iter()
        .filter_map(|(on, marker)| on.then_some(marker))
        .collect();
        for piece in pieces(&span.text) {
            if piece.starts_with(char::is_whitespace) {
                out.push_str(piece);
                continue;
            }
            for marker in &markers {
                out.push_str(marker);
            }
            out.push_str(piece);
            for marker in markers.iter().rev() {
                out.push_str(marker);
            }
        }
    }
    out
}

/// `text` in runs of whitespace and of the words between them, in order.
fn pieces(text: &str) -> impl Iterator<Item = &str> {
    let mut rest = text;
    std::iter::from_fn(move || {
        let first = rest.chars().next()?;
        let space = first.is_whitespace();
        let end = rest
            .find(|c: char| c.is_whitespace() != space)
            .unwrap_or(rest.len());
        let (piece, after) = rest.split_at(end);
        rest = after;
        Some(piece)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_start_where_the_desktop_reader_starts_them() {
        assert_eq!(
            to_markdown("<dl><dt>Term</dt><dd>One</dd><dd>Two</dd></dl><nav>Menu</nav>After"),
            "Term\n\nOne\n\nTwo\n\nMenu\n\nAfter"
        );
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
                        <= 2 * (MAX_RENDER_DEPTH - 1) + 2)
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
    fn bounded_text_stops_at_a_character() {
        let mut text = BoundedText::new(4);
        text.push("é猫");
        assert_eq!(text.text, "é");
        assert!(text.truncated);
    }

    #[test]
    fn a_link_longer_than_the_output_reads_as_its_text() {
        let target = format!("https://example.test/{}", "x".repeat(32 * 1024));
        assert_eq!(
            to_markdown(&format!("<p>tail <a href='{target}'>link</a></p>")),
            format!("tail [link]({target})")
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
            "- First\n- Second\n  line"
        );
    }

    #[test]
    fn only_the_first_paragraph_of_a_list_item_carries_its_marker() {
        // The others line up under its text, after a list within the item too.
        assert_eq!(
            to_markdown("<ul><li><p>One</p><p>more</p></li><li>Two</li></ul>"),
            "- One\n\n  more\n\n- Two"
        );
        assert_eq!(
            to_markdown(
                "<ol><li><p>One</p><p>more</p><ul><li><p>a</p><p>b</p></li></ul>after</li>\
                 <li>Two</li></ol>"
            ),
            "1. One\n\n   more\n\n  - a\n\n    b\n\n   after\n\n2. Two"
        );
    }

    #[test]
    fn inline_styles_last_until_their_outermost_element_closes() {
        let html = "<p><b>one <strong>two</strong> three</b> four <i>five <em>six</em> seven</i> \
                    <code>x <kbd>y</kbd> z</code> <samp>w</samp></p>";
        assert_eq!(
            to_markdown(html),
            "**one** **two** **three** four *five* *six* *seven* `x` `y` `z` `w`"
        );
    }

    #[test]
    fn every_style_tag_the_desktop_reads_counts() {
        assert_eq!(
            to_markdown("<p><cite>The Book</cite> and <tt>cargo test</tt></p>"),
            "*The* *Book* and `cargo` `test`"
        );
    }

    #[test]
    fn a_struck_price_reads_as_struck() {
        assert_eq!(
            to_markdown("<p>Was <s>$20</s>, now <del>$18</del> <strike>$16</strike> $15</p>"),
            "Was ~~$20~~, now ~~$18~~ ~~$16~~ $15"
        );
    }

    #[test]
    fn images_say_how_many_are_not_shown_but_pixels_do_not_count() {
        assert_eq!(
            to_markdown(
                "<p>Sale</p><img src='cid:banner'><img src='cid:logo' alt='Logo'>\
                 <img src='https://t.example/o.gif' width='1' height='1'>"
            ),
            "Sale\n\n[2 images not shown]"
        );
        assert_eq!(to_markdown("<img src='cid:only'>"), "[1 image not shown]");
    }

    #[test]
    fn the_senders_style_reads_as_its_tags_would() {
        assert_eq!(
            to_markdown(
                "<p style='text-align:center'><span style='font-weight:bold'>Sale</span> ends \
                 <span style='text-decoration:line-through'>today</span>, \
                 <span style='font-style:italic'>maybe</span></p>"
            ),
            "**Sale** ends ~~today~~, *maybe*"
        );
    }

    #[test]
    fn text_the_sender_hid_is_left_out() {
        assert_eq!(
            to_markdown(
                "<div style='display:none'>Preview text</div>\
                 <p>Pay<span style='display:none'>x</span>Pal, <b>today</b> <span hidden>only</span></p>"
            ),
            "PayPal, **today**"
        );
    }

    #[test]
    fn a_line_break_in_a_link_starts_the_link_again_on_the_next_line() {
        assert_eq!(
            to_markdown("<p><a href='https://example.test/'>one<br><b>two</b> </a>three</p>"),
            "[one](https://example.test/)\n[**two**](https://example.test/) three"
        );
    }
}
