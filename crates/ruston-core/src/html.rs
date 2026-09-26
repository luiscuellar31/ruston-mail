//! HTML sanitization for message bodies.

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

/// Remove active and hidden content while retaining safe HTML formatting.
/// Image tags may remain; renderers decide whether to display or fetch them.
pub fn sanitize(html: &str) -> String {
    ammonia::Builder::new()
        .add_clean_content_tags(&[
            "head", "title", "noscript", "template", "textarea", "iframe", "svg",
        ])
        .clean(html)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

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
