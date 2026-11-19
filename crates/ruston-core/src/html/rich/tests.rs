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
                } => format!("li{depth} {} {}", marker.label(), text(spans)),
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
fn styled_runs_share_one_long_link_target() {
    let target = format!("https://example.test/{}", "x".repeat(32 * 1024));
    let body = spans(&format!(
        "<a href='{target}'>plain<b>bold</b><i>italic</i>tail</a>"
    ));
    assert_eq!(body.len(), 4);
    let first = body[0].link.as_deref().unwrap();
    assert_eq!(first, target);
    for span in &body {
        let link = span.link.as_deref().unwrap();
        assert!(std::ptr::eq(first, link));
    }
}

#[test]
fn equal_adjacent_links_refresh_the_shared_target_when_runs_merge() {
    let target: Arc<str> = Arc::from(format!("https://example.test/{}", "x".repeat(32 * 1024)));
    let next: Arc<str> = Arc::from(target.as_ref());
    assert!(!Arc::ptr_eq(&target, &next));
    let mut builder = Builder {
        link: Some(target),
        ..Builder::new(usize::MAX)
    };
    builder.text("first");
    builder.link = Some(next.clone());
    builder.text("second");
    assert_eq!(builder.spans.len(), 1);
    assert_eq!(builder.spans[0].text, "firstsecond");
    assert!(Arc::ptr_eq(builder.spans[0].link.as_ref().unwrap(), &next));
    let cloned = builder.spans[0].clone();
    assert!(Arc::ptr_eq(cloned.link.as_ref().unwrap(), &next));
}

#[test]
fn text_runs_keep_unicode_whitespace_entities_and_link_styles() {
    let body = spans(
        "<p>\u{2003}<a href='https://example.test/'>α\t β<br><b>猫 &amp; <i>é</i></b></a> tail</p>",
    );
    assert_eq!(
        body.iter()
            .map(|span| span.text.as_str())
            .collect::<String>(),
        "α β\n猫 & é tail"
    );
    let cat = body.iter().find(|span| span.text.contains('猫')).unwrap();
    assert!(cat.strong);
    let accented = body.iter().find(|span| span.text == "é").unwrap();
    assert!(accented.strong && accented.emphasis);
    assert!(Arc::ptr_eq(
        cat.link.as_ref().unwrap(),
        accented.link.as_ref().unwrap()
    ));
    assert!(body.last().unwrap().link.is_none());
    fn require_send_sync<T: Send + Sync>() {}
    require_send_sync::<RichBody>();
}

#[test]
fn a_control_character_separates_words_without_a_space() {
    // A terminal would obey it, and the desktop would draw it as a box.
    assert_eq!(
        outline("<p>Fish &amp; chips &#27; safe</p><p>é\u{1b}tail</p><pre>a\u{7}b\tc</pre>"),
        ["p Fish & chips safe", "p étail", "pre ab\tc"]
    );
}

#[test]
#[ignore = "manual scaling benchmark with fictional long HTML links"]
fn measure_long_link_conversion_scaling() {
    for size in [32 * 1024, 64 * 1024, 128 * 1024] {
        let target = format!("https://example.test/{}", "x".repeat(size));
        let label = "z".repeat(size);
        let html = format!("<a href='{target}'>x</a><a href='{target}'>{label}</a>");
        let sanitized = crate::html::sanitize(&html);
        let start = std::time::Instant::now();
        let body = parse(&sanitized);
        eprintln!("URL/label={size} bytes in {:?}", start.elapsed());
        assert_eq!(body.plain_text(), format!("x{label}\n"));
    }
}

#[test]
fn input_read_in_chunks_keeps_characters_entities_and_links_whole() {
    let text = "é".repeat(INPUT_CHUNK_BYTES / 2 - 2);
    let target = format!("https://example.test/{}", "x".repeat(INPUT_CHUNK_BYTES * 2));
    let html = format!("<p>{text}🐈 &amp; tail <a href='{target}'>link</a></p>");
    let body = parse_within(&html, 1024 * 1024);
    let [first, last] = &spans(&html)[..] else {
        panic!("expected two runs");
    };
    assert_eq!(first.text, format!("{text}🐈 & tail "));
    assert_eq!(last.text, "link");
    assert_eq!(last.link.as_deref(), Some(target.as_str()));
    assert!(!body.truncated);
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
fn an_item_goes_on_under_its_text_after_a_list_within_it() {
    let html = "<ol><li><p>One</p><p>more</p><ul><li><p>a</p><p>b</p></li></ul>after</li>\
                <li>Two</li></ol><ul><li><ul><li>inner</li></ul>outer</li></ul>";

    assert_eq!(
        outline(html),
        [
            "li1 1. One",
            "li1  more",
            "li2 • a",
            "li2  b",
            "li1  after",
            "li1 2. Two",
            // An item whose list came first keeps its marker for its text.
            "li2 • inner",
            "li1 • outer",
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
    let body = parse("<p>and a <s>withdrawn</s> point, plus <code>settings.json</code> here</p>");
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

#[test]
fn text_within_the_limit_stops_at_a_character() {
    let text = "é猫";
    // A run's storage, and room for the block it ends up in.
    let overhead = std::mem::size_of::<RichSpan>() + BLOCK_SIZE;
    for (limit, expected) in [(overhead - 1, ""), (overhead + 1, ""), (overhead + 4, "é")] {
        let mut builder = Builder::new(limit);
        builder.text(text);
        assert!(builder.truncated);
        assert!(builder.used <= limit);
        let kept: String = builder
            .spans
            .iter()
            .map(|span| span.text.as_str())
            .collect();
        assert_eq!(kept, expected);
    }
}

#[test]
fn a_body_past_its_limit_is_cut_short_and_says_so() {
    let limit = 64 * 1024;
    for html in [
        format!("<p>{}</p><p>AFTER_LIMIT</p>", "é".repeat(limit)),
        format!("<pre>{}</pre><p>AFTER_LIMIT</p>", "é\n".repeat(limit / 2)),
        format!(
            "{}<p>AFTER_LIMIT</p>",
            "<blockquote><p>text</p></blockquote>".repeat(limit / 4)
        ),
        // Many tiny style changes, or blocks with no text, take room too.
        format!("<p>{}</p><p>AFTER_LIMIT</p>", "<b>x</b>x".repeat(limit)),
        format!("{}<p>AFTER_LIMIT</p>", "<hr>".repeat(limit)),
    ] {
        let body = parse_within(&html, limit);
        assert!(body.truncated);
        assert!(!body.plain_text().contains("AFTER_LIMIT"));
        assert!(body.blocks.len() * std::mem::size_of::<RichBlock>() <= limit);
    }
    // What was read before the limit is kept, a long preformatted text too.
    let body = parse_within(&format!("<pre>{}</pre>", "x\n".repeat(limit)), limit);
    assert!(
        matches!(&body.blocks[..], [RichBlock { kind: BlockKind::Preformatted(text), .. }] if text.starts_with("x\nx"))
    );
    // Without a limit nothing is left out.
    let html = format!("<p>{}</p><p>AFTER_LIMIT</p>", "é".repeat(limit));
    let body = parse(&html);
    assert!(!body.truncated);
    assert!(body.plain_text().contains("AFTER_LIMIT"));
}

#[test]
fn the_list_stack_stops_growing_at_its_limit() {
    let mut builder = Builder::new(usize::MAX);
    for _ in 0..1024 {
        builder.start_list(Some(1));
    }
    assert_eq!(builder.lists.len(), MAX_LIST_DEPTH);
    assert_eq!(builder.omitted_lists, 1024 - MAX_LIST_DEPTH);

    // Deep lists unwind to the right outer level, numbered as they were.
    let html = format!(
        "<ol><li>first</li><li>{}deep{}</li><li>last</li></ol>",
        "<ul><li>".repeat(128),
        "</li></ul>".repeat(128)
    );
    let outline = outline(&html);
    assert_eq!(outline.first().map(String::as_str), Some("li1 1. first"));
    assert!(outline.contains(&format!("li{MAX_LIST_DEPTH} • deep")));
    assert_eq!(outline.last().map(String::as_str), Some("li1 3. last"));
}

fn plain(text: &str) -> Vec<RichSpan> {
    vec![RichSpan {
        text: text.to_owned(),
        ..RichSpan::default()
    }]
}

fn block(kind: BlockKind, quote_depth: u8) -> RichBlock {
    RichBlock { kind, quote_depth }
}

#[test]
fn plain_text_keeps_structure_one_block_per_line() {
    let body = RichBody {
        blocks: vec![
            block(
                BlockKind::Heading {
                    level: 1,
                    spans: plain("Title"),
                },
                0,
            ),
            block(BlockKind::Paragraph(plain("Hello")), 0),
            block(
                BlockKind::ListItem {
                    marker: ListMarker::Bullet,
                    depth: 1,
                    spans: plain("One"),
                },
                0,
            ),
            block(
                BlockKind::Image {
                    description: "Logo".to_owned(),
                },
                0,
            ),
            block(BlockKind::Rule, 0),
            block(BlockKind::Paragraph(plain("Quoted")), 1),
            block(
                BlockKind::Preformatted("let x = 1;\n  indented".to_owned()),
                2,
            ),
        ],
        truncated: false,
    };

    assert_eq!(
        body.plain_text(),
        "Title\nHello\n• One\n[Image: Logo]\n---\n> Quoted\n> > let x = 1;\n> >   indented\n"
    );
}
