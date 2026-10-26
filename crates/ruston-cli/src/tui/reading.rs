//! What the reader shows of a conversation: each message's headers, its
//! signature's verdict and its body as `conversations read` shows it, with
//! every line safe to draw, where its quoted passages are and the web
//! addresses it links to. It is built once, in the task that loads the
//! conversation, since turning a large HTML body into text takes a while.

use crate::render;
use crate::terminal::{screen_line, screen_text};
use ruston_core::{Conversation, ConversationMessage, MessageMetadata};
use std::ops::Range;

/// What stands between two messages.
const SEPARATOR: &str = "────────────────────────";

/// How many lines a quoted passage needs before the reader folds it; a
/// shorter one would take about as many rows folded.
const FOLDED_MIN_LINES: usize = 3;

/// How many links of a conversation the reader lists at most.
const MAX_LINKS: usize = 100;

/// A conversation as the reader shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Reading {
    /// The conversation's ID.
    pub(crate) id: String,
    pub(crate) subject: String,
    /// Its lines, before the pane wraps them to its width.
    pub(crate) lines: Vec<String>,
    /// The ranges of `lines` that quote another message, in order, which
    /// the reader folds into a line each until asked to show them.
    pub(crate) quotes: Vec<Range<usize>>,
    /// The web addresses its bodies link to, each once, in order: what `o`
    /// lists to open one.
    pub(crate) links: Vec<String>,
}

impl Reading {
    pub(crate) fn new(conversation: &Conversation, messages: &[ConversationMessage]) -> Self {
        let mut lines = Vec::new();
        let mut quotes = Vec::new();
        let mut links: Vec<String> = Vec::new();
        for (index, message) in messages.iter().enumerate() {
            if index > 0 {
                lines.extend([String::new(), SEPARATOR.to_owned(), String::new()]);
            }
            match message {
                ConversationMessage::Opened(message) => {
                    headers(
                        &mut lines,
                        &message.meta,
                        render::verdict_name(message.verdict),
                    );
                    lines.push(String::new());
                    let body = screen_text(&render::thread_body(message));
                    let start = lines.len();
                    lines.extend(body.lines().map(str::to_owned));
                    quotes.extend(
                        quoted_passages(&lines[start..])
                            .into_iter()
                            .map(|quote| start + quote.start..start + quote.end),
                    );
                    for link in web_links(&body) {
                        if links.len() < MAX_LINKS && !links.contains(&link) {
                            links.push(link);
                        }
                    }
                }
                ConversationMessage::Unreadable(message) => {
                    headers(&mut lines, &message.meta, "not decrypted");
                    lines.push(String::new());
                    lines.push(screen_line(&format!(
                        "(This message could not be decrypted: {})",
                        message.error
                    )));
                }
            }
        }
        Self {
            id: conversation.id.clone(),
            subject: screen_line(render::subject_or_empty(&conversation.subject)),
            lines,
            quotes,
            links,
        }
    }
}

/// The web addresses in `text`, written out or left by the HTML renderer as
/// `[words](address)`: each from `http://` or `https://` to the first space,
/// quote or square bracket, less the punctuation that ends a sentence around
/// it, as `url` reads it, so the address shown is the one that opens, its
/// host in ASCII. Words that are an address themselves count as one too.
fn web_links(text: &str) -> Vec<String> {
    let mut links = Vec::new();
    let mut from = 0;
    while let Some(found) = text[from..].find("http") {
        let start = from + found;
        let rest = &text[start..];
        let end = rest
            .find(|c: char| {
                c.is_whitespace() || matches!(c, '<' | '>' | '"' | '\'' | '`' | '[' | ']')
            })
            .unwrap_or(rest.len());
        from = start + end.max(1);
        let mut address = &rest[..end];
        while let Some(last) = address.chars().last() {
            // A closing bracket ends the address unless it closes one of its own.
            let unopened =
                last == ')' && address.matches('(').count() < address.matches(')').count();
            if !(unopened || matches!(last, '.' | ',' | ';' | ':' | '!' | '?' | ']' | '}' | '*')) {
                break;
            }
            address = &address[..address.len() - last.len_utf8()];
        }
        if !(address.starts_with("http://") || address.starts_with("https://")) {
            continue;
        }
        if let Ok(url) = url::Url::parse(address)
            && matches!(url.scheme(), "http" | "https")
        {
            links.push(url.to_string());
        }
    }
    links
}

/// The passages of a message's `body` that quote another message: runs of
/// lines that start with `>`, with the blank lines between them, at least
/// [`FOLDED_MIN_LINES`] long. A body that is all quotes has none, as the
/// desktop leaves it open: folded, nothing of it would show.
fn quoted_passages(body: &[String]) -> Vec<Range<usize>> {
    let quoted = |line: &String| line.starts_with('>');
    if body
        .iter()
        .all(|line| quoted(line) || line.trim().is_empty())
    {
        return Vec::new();
    }
    let mut passages = Vec::new();
    let mut start = 0;
    while let Some(first) = body[start..].iter().position(quoted) {
        let first = start + first;
        // It ends at its last quoted line before any other text.
        let mut end = first + 1;
        for (index, line) in body.iter().enumerate().skip(end) {
            if quoted(line) {
                end = index + 1;
            } else if !line.trim().is_empty() {
                break;
            }
        }
        if end - first >= FOLDED_MIN_LINES {
            passages.push(first..end);
        }
        start = end;
    }
    passages
}

/// The headers of a message, as `conversations read` names them.
fn headers(lines: &mut Vec<String>, meta: &MessageMetadata, verdict: &str) {
    lines.push(screen_line(&format!(
        "From:    {}",
        render::fmt_addr(&meta.sender)
    )));
    lines.push(screen_line(&format!(
        "To:      {}",
        render::fmt_addrs(&meta.to_list)
    )));
    if !meta.cc_list.is_empty() {
        lines.push(screen_line(&format!(
            "Cc:      {}",
            render::fmt_addrs(&meta.cc_list)
        )));
    }
    lines.push(format!("Date:    {}", render::fmt_time(meta.time)));
    lines.push(format!("Verdict: {verdict}"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use ruston_core::{Error, FullMessage, Recipient, UnreadableMessage, Verdict};

    fn opened(sender: &str, body: &str) -> ConversationMessage {
        ConversationMessage::Opened(FullMessage {
            meta: meta(sender),
            body: body.into(),
            mime_type: "text/plain".into(),
            verdict: Verdict::Verified,
            attachments: Vec::new(),
            reply_tos: Vec::new(),
        })
    }

    fn meta(sender: &str) -> MessageMetadata {
        MessageMetadata {
            sender: Recipient::new(sender),
            to_list: vec![Recipient::new("me@example.test")],
            time: 1_714_000_000,
            ..MessageMetadata::default()
        }
    }

    #[test]
    fn each_message_shows_its_headers_verdict_and_body_safely() {
        let conversation = Conversation {
            subject: "Plans\x1b[2J\nfor May".into(),
            ..Conversation::default()
        };
        let messages = [
            ConversationMessage::Opened(FullMessage {
                meta: meta("alex@example.test"),
                body: "Hi,\r\n\tsee you\x1b]52;c;payload\x07 there".into(),
                mime_type: "text/plain".into(),
                verdict: Verdict::Verified,
                attachments: Vec::new(),
                reply_tos: Vec::new(),
            }),
            ConversationMessage::Unreadable(UnreadableMessage {
                meta: meta("sam@example.test"),
                reply_tos: Vec::new(),
                error: Error::Other("no key opens it".into()),
            }),
        ];

        let reading = Reading::new(&conversation, &messages);
        assert_eq!(reading.subject, "Plans[2J for May");
        let text = reading.lines.join("\n");
        assert!(
            text.chars().all(|c| !c.is_control() || c == '\n'),
            "{text:?}"
        );
        for expected in [
            "From:    alex@example.test",
            "To:      me@example.test",
            "Date:    2024-04-24 23:06:40 UTC",
            "Verdict: verified",
            "Hi,\n    see you",
            SEPARATOR,
            "From:    sam@example.test",
            "Verdict: not decrypted",
            "(This message could not be decrypted: no key opens it)",
        ] {
            assert!(text.contains(expected), "{expected:?} missing:\n{text}");
        }
        assert!(reading.quotes.is_empty());
    }

    #[test]
    fn quoted_passages_of_three_lines_or_more_fold_unless_all_is_quoted() {
        let messages = [
            opened(
                "alex@example.test",
                "Thanks!\n\nOn Monday, Sam wrote:\n> Shall we meet?\n>\n\n> Tuesday works.\n\n\
                 Below:\n> one\n> two\n\nAnd:\n> x\n> y\n> z\n\nBye",
            ),
            opened("sam@example.test", "> all\n> of it\n\n> quoted"),
        ];
        let reading = Reading::new(&Conversation::default(), &messages);
        assert!(reading.links.is_empty());
        let at = |text: &str| reading.lines.iter().position(|line| line == text).unwrap();
        let (first, second) = (at("> Shall we meet?"), at("> x"));
        // The blank line between two quoted ones belongs to the passage; the
        // two lines quoted below it are too few to fold.
        assert_eq!(reading.quotes, [first..first + 4, second..second + 3]);
        assert_eq!(reading.lines[first + 3], "> Tuesday works.");
        assert_eq!(reading.lines[first + 4], "");
    }

    #[test]
    fn links_are_the_web_addresses_of_the_bodies_as_they_open() {
        let text = "See https://example.test/a, the [report](https://example.test/r%281%29) \
                    and (https://example.test/b). Also http://xn--bcher-kva.example/ and \
                    https://bücher.example/x, mailto:me@example.test, javascript:alert(1), \
                    httpd, **https://example.test/c**, \
                    [https://example.test/d](https://example.test/d) and \
                    https://en.wikipedia.org/wiki/Rust_(programming_language) too.";
        assert_eq!(
            web_links(text),
            [
                "https://example.test/a",
                "https://example.test/r%281%29",
                "https://example.test/b",
                "http://xn--bcher-kva.example/",
                "https://xn--bcher-kva.example/x",
                "https://example.test/c",
                "https://example.test/d",
                "https://example.test/d",
                "https://en.wikipedia.org/wiki/Rust_(programming_language)",
            ]
        );
    }

    #[test]
    fn an_html_link_lists_where_it_goes_as_well_as_what_it_says() {
        let html = "<p>Pay at <a href=\"https://real.example/pay\">https://bank.example</a></p>";
        let mut message = opened("alex@example.test", html);
        if let ConversationMessage::Opened(full) = &mut message {
            full.mime_type = "text/html".into();
        }
        let reading = Reading::new(&Conversation::default(), &[message]);
        let text = reading.lines.join(
            "
",
        );
        assert!(
            text.contains("[https://bank.example](https://real.example/pay)"),
            "{text}"
        );
        assert_eq!(
            reading.links,
            ["https://bank.example/", "https://real.example/pay"]
        );
    }

    #[test]
    fn a_conversation_lists_each_link_of_its_bodies_once() {
        let messages = [
            opened("alex@example.test", "Plans: https://example.test/plan"),
            opened(
                "sam@example.test",
                "> Plans: https://example.test/plan\nNotes: https://example.test/notes",
            ),
        ];
        let reading = Reading::new(&Conversation::default(), &messages);
        assert_eq!(
            reading.links,
            ["https://example.test/plan", "https://example.test/notes"]
        );
    }
}
