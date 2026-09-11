//! Plain-text rendering of sanitized HTML message bodies, used until Ruston
//! can render HTML. The output is only ever shown as text, so nothing in it
//! runs or loads remote content.

/// Elements whose text is never shown.
const HIDDEN: [&str; 4] = ["head", "script", "style", "title"];

/// Elements that begin or end a line.
const BLOCKS: [&str; 21] = [
    "address",
    "article",
    "blockquote",
    "br",
    "div",
    "footer",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hr",
    "ol",
    "p",
    "pre",
    "section",
    "table",
    "tr",
    "ul",
];

pub(super) fn to_plain_text(html: &str) -> String {
    let mut text = String::with_capacity(html.len());
    let mut hidden: Option<String> = None;
    let mut rest = html;

    while let Some(start) = rest.find('<') {
        if hidden.is_none() {
            push_text(&mut text, &rest[..start]);
        }
        // An unterminated tag ends the body.
        let Some(length) = rest[start..].find('>') else {
            rest = "";
            break;
        };
        let tag = &rest[start + 1..start + length];
        rest = &rest[start + length + 1..];

        let closing = tag.starts_with('/');
        let name = tag
            .trim_start_matches('/')
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .collect::<String>()
            .to_ascii_lowercase();

        match &hidden {
            Some(element) => {
                if closing && *element == name {
                    hidden = None;
                }
            }
            None if !closing && HIDDEN.contains(&name.as_str()) => hidden = Some(name),
            None if !closing && name == "li" => text.push_str("\n• "),
            None if BLOCKS.contains(&name.as_str()) => text.push('\n'),
            None => {}
        }
    }
    if hidden.is_none() {
        push_text(&mut text, rest);
    }

    tidy(&text)
}

/// Appends decoded text, collapsing whitespace runs like a browser does.
fn push_text(text: &mut String, fragment: &str) {
    for c in decode_entities(fragment).chars() {
        if c.is_whitespace() {
            if !text.ends_with([' ', '\n']) {
                text.push(' ');
            }
        } else {
            text.push(c);
        }
    }
}

/// Trims every line and keeps at most one blank line between paragraphs.
fn tidy(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank = false;

    for line in text.lines().map(str::trim) {
        if line.is_empty() {
            blank = true;
            continue;
        }
        if !out.is_empty() {
            out.push_str(if blank { "\n\n" } else { "\n" });
        }
        out.push_str(line);
        blank = false;
    }
    out
}

fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;

    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let decoded = rest[1..]
            .find(';')
            .filter(|&end| end <= 10)
            .and_then(|end| Some((entity(&rest[1..1 + end])?, end + 2)));
        match decoded {
            Some((c, length)) => {
                out.push(c);
                rest = &rest[length..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn entity(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some(' '),
        _ => {
            let code = name.strip_prefix('#')?;
            let value = match code.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => code.parse().ok()?,
            };
            char::from_u32(value)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_become_lines_and_paragraphs() {
        let html = "<div><p>Hello <b>Alex</b>,</p><p>Line one<br>Line two</p></div>";

        assert_eq!(to_plain_text(html), "Hello Alex,\n\nLine one\nLine two");
    }

    #[test]
    fn entities_are_decoded_and_unknown_ones_kept() {
        let html = "Fish &amp; chips &lt;3 &#39;ok&#39; &#x263A; &copy; a&b";

        assert_eq!(to_plain_text(html), "Fish & chips <3 'ok' ☺ &copy; a&b");
    }

    #[test]
    fn hidden_elements_are_dropped() {
        let html = "<head><title>Subject</title><style>p { color: red }</style></head>\
                    <body><p>Visible</p><script>alert(1)</script></body>";

        assert_eq!(to_plain_text(html), "Visible");
    }

    #[test]
    fn lists_get_bullets_and_whitespace_collapses() {
        let html = "<ul>\n  <li>First   item</li>\n  <li>Second\n item</li>\n</ul>";

        assert_eq!(to_plain_text(html), "• First item\n• Second item");
    }

    #[test]
    fn malformed_input_is_handled() {
        assert_eq!(to_plain_text("Text <b unterminated"), "Text");
        assert_eq!(to_plain_text(""), "");
        assert_eq!(to_plain_text("Plain text only"), "Plain text only");
    }
}
