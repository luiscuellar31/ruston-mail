//! HTML sanitization for message bodies.

use std::collections::HashSet;
use std::sync::LazyLock;

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
        .add_clean_content_tags(&[
            "head", "title", "noscript", "template", "textarea", "iframe", "svg",
        ])
        // A link may lead to the web or to an address, and nowhere else. Every
        // output keeps this, EML exports and quoted replies included, not only
        // the readers that filter links themselves.
        .url_schemes(HashSet::from(["http", "https", "mailto"]))
        .url_relative(ammonia::UrlRelative::Deny);
    builder
});

/// Remove active and hidden content while retaining safe HTML formatting.
/// Image tags may remain; renderers decide whether to display or fetch them.
/// Links keep only http, https and mailto URLs.
pub fn sanitize(html: &str) -> String {
    SANITIZER.clean(html).to_string()
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
