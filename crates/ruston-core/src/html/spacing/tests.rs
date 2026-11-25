use super::*;

#[test]
fn margins_and_padding_become_room_or_none_above_and_below() {
    for (style, normalized) in [
        ("margin:0", "margin-top:0;margin-bottom:0"),
        ("margin: 0px auto", "margin-top:0;margin-bottom:0"),
        (
            "margin:0;text-align:left;margin-bottom:0px !important",
            "margin-top:0;margin-bottom:0;text-align:left",
        ),
        ("margin:16px 0 0 0", "margin-top:1em;margin-bottom:0"),
        ("margin: 0 0 8px", "margin-top:0;margin-bottom:1em"),
        ("MARGIN-TOP: 1.5EM", "margin-top:1em"),
        ("padding-bottom:8px !important", "margin-bottom:1em"),
        // Padding makes room where a margin of none does not take it away.
        (
            "margin:0; padding:12px 0",
            "margin-top:1em;margin-bottom:1em",
        ),
        // A negative margin makes none, and pulls nothing anywhere.
        ("margin-top:-500px", "margin-top:0"),
        // What is not a length says nothing, and goes.
        ("margin: calc(1em + 2px)", ""),
        ("margin-left:20px; margin-right:auto", ""),
        ("padding:0", ""),
        ("color:red", "color:red"),
    ] {
        assert_eq!(normalize(style), normalized, "{style}");
    }
}

#[test]
fn a_line_across_a_block_stays_and_a_box_does_not() {
    for (style, normalized) in [
        ("border-top:1px solid #dddddd", "border-top:1px solid"),
        ("border-bottom: 2px dashed red", "border-bottom:1px solid"),
        // A style alone draws a medium line.
        ("border-top-style: solid", "border-top:1px solid"),
        (
            "border-style: solid none; border-width: 1px 0",
            "border-top:1px solid;border-bottom:1px solid",
        ),
        // A width alone draws nothing, and neither does none or zero.
        ("border-top: 1px", ""),
        ("border-top: none", ""),
        ("border-bottom: 0 solid black", ""),
        ("border-top:1px solid; border-top-width:0", ""),
        // A box, as a button or a card is drawn, is no line across.
        ("border:1px solid #333", ""),
        ("border-top:1px solid; border-right:1px solid", ""),
        // A column's line down one side is not one across either.
        ("border-right:1px solid #ddd", ""),
        ("border-radius:4px; border-collapse:collapse", ""),
    ] {
        assert_eq!(normalize(style), normalized, "{style}");
    }
}

/// The margin, padding and border declarations a reader of `style` finds,
/// reading it as ammonia does, each with its value as written.
fn spacing_declarations(style: &str) -> Vec<(String, String)> {
    let mut input = ParserInput::new(style);
    let mut parser = Parser::new(&mut input);
    let mut found = Vec::new();
    while !parser.is_exhausted() {
        let declaration = parser.parse_until_after(Delimiter::Semicolon, |parser| {
            let name = parser.expect_ident()?.to_ascii_lowercase();
            parser.expect_colon()?;
            let start = parser.position();
            while parser.next().is_ok() {}
            let value = parser.slice_from(start).trim().to_owned();
            Ok::<_, ParseError<'_, ()>>((name, value))
        });
        if let Ok((name, value)) = declaration
            && ["margin", "padding", "border"]
                .iter()
                .any(|kind| name.starts_with(kind))
        {
            found.push((name, value));
        }
    }
    found
}

#[test]
fn nothing_the_sender_writes_reaches_a_margin_or_a_border_unread() {
    // ammonia reads names through comments and escapes, and so does this.
    for style in [
        "/* a */margin-top:-50px",
        "mar\\67 in-top:-50px",
        "margin\\-top:-50px",
        "x:\\\"; margin-top:-50px; y:\\\"",
        "border-top:1px solid; MARGIN-TOP :-9px",
        "margin-top:-50px; margin-top:-60px !important",
        "padding: 0 -10px; border: 3px double; border-left: none",
        "border-bottom: 1px solid; margin-left: -9999px",
    ] {
        let normalized = normalize(style);
        for (name, value) in spacing_declarations(&normalized) {
            assert!(
                matches!(
                    (name.as_str(), value.as_str()),
                    ("margin-top" | "margin-bottom", "0" | "1em")
                        | ("border-top" | "border-bottom", "1px solid")
                ),
                "{style} let {name}: {value} through: {normalized}"
            );
        }
    }
    // A string left open at the end cannot take in what comes first.
    assert_eq!(
        normalize("margin:0; font-family: \"Unclosed"),
        "margin-top:0;margin-bottom:0;font-family: \"Unclosed"
    );
    // Other declarations come through as written, for the sanitizer to sift.
    assert_eq!(
        normalize("display:none; font-family: \"A;B\"; border-top: 1px solid"),
        "border-top:1px solid;display:none;font-family: \"A;B\""
    );
}
