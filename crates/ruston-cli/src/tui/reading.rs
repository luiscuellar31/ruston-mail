//! What the reader shows of a conversation: each message's headers, its
//! signature's verdict and its body as `conversations read` shows it, with
//! every line safe to draw, where its quoted passages are and the web
//! addresses it links to. It is built once, in the task that loads the
//! conversation, since turning a large HTML body into text takes a while.

use crate::render;
use crate::terminal::{screen_line, screen_text};
use ruston_core::model::enums::label_ids;
use ruston_core::{
    AddressInfo, Conversation, ConversationMessage, MessageMetadata, Recipient, reply_recipients,
    reply_redirected,
};
use std::ops::Range;

/// What stands between two messages.
const SEPARATOR: &str = "────────────────────────";

/// How many lines a quoted passage needs before the reader folds it; a
/// shorter one would take about as many rows folded.
const FOLDED_MIN_LINES: usize = 3;

/// How many links of a conversation the reader lists at most.
const MAX_LINKS: usize = 100;

/// How many of its first words a folded message's line keeps, as the
/// desktop's folded header previews.
const PREVIEW_WORDS: usize = 40;

/// One of the conversation's messages, which the reader can fold into one
/// line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReadingMessage {
    /// The range of the reading's lines it takes, from its headers to the
    /// end of its body, without the separator before it.
    pub(crate) lines: Range<usize>,
    /// The line that stands for it folded, safe to draw: when it came, who
    /// sent it, its signature's verdict and its first words.
    pub(crate) line: String,
    /// Whether it is not read yet.
    pub(crate) unread: bool,
}

/// An attachment of the conversation, which `v` lists to save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReadingAttachment {
    pub(crate) message_id: String,
    pub(crate) id: String,
    /// Its name, safe to draw.
    pub(crate) name: String,
    pub(crate) size: u64,
    /// Whether it belongs within the body rather than beside it, as a
    /// picture some apps place within the text does.
    pub(crate) inline: bool,
}

/// The message an answer to the open conversation answers, its last one
/// that is not a draft, with what writing the answer shows of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Answerable {
    pub(crate) message_id: String,
    /// The account's address it reached, which core answers from.
    pub(crate) from: Option<String>,
    /// Its subject and who sent it, safe to draw.
    pub(crate) subject: String,
    pub(crate) sender: String,
    /// Whom a reply goes to, as core would send it, and whom Reply all
    /// copies in as well.
    pub(crate) to: Vec<String>,
    pub(crate) cc: Vec<String>,
    /// Its sender, when the message asks for replies to go elsewhere.
    pub(crate) redirected_from: Option<String>,
    /// Whether it opened: only then can an answer quote it.
    pub(crate) opened: bool,
    /// The files a forward carries, the parts of the body included, unless
    /// left out.
    pub(crate) files: Vec<ReadingAttachment>,
}

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
    /// Its messages, oldest first.
    pub(crate) messages: Vec<ReadingMessage>,
    /// The web addresses its bodies link to, each once, in order: what `b`
    /// lists to open one.
    pub(crate) links: Vec<String>,
    /// The attachments of its opened messages, in order, inline ones aside.
    pub(crate) attachments: Vec<ReadingAttachment>,
    /// What an answer to it answers, when it has a message that is not a
    /// draft; boxed, as the reading travels in a message to the loop.
    pub(crate) answer: Option<Box<Answerable>>,
    /// The rows its lines wrap to, kept by the pane that draws it.
    pub(crate) wrapped: WrappedRows,
}

/// The rows each line of a reading wraps to at one width, counted the first
/// time it is drawn and kept from one frame to the next: wrapping a whole
/// conversation again at every key is what made a long one slow to move in.
/// It is no part of what the conversation says, so it compares equal to any
/// other and a copy starts without it.
#[derive(Debug, Default)]
pub(crate) struct WrappedRows(std::cell::RefCell<(u16, Vec<u32>)>);

impl WrappedRows {
    /// The rows line `at` takes `width` columns wide, counted with `wrap`
    /// unless it was already.
    pub(crate) fn rows(&self, at: usize, width: u16, wrap: impl FnOnce() -> usize) -> usize {
        let mut kept = self.0.borrow_mut();
        if kept.0 != width {
            *kept = (width, Vec::new());
        }
        if kept.1.len() <= at {
            kept.1.resize(at + 1, 0);
        }
        // Kept as one more than the rows, so that zero says not counted yet.
        if kept.1[at] == 0 {
            kept.1[at] = u32::try_from(wrap()).unwrap_or(u32::MAX - 1) + 1;
        }
        (kept.1[at] - 1) as usize
    }
}

impl Clone for WrappedRows {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl PartialEq for WrappedRows {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for WrappedRows {}

impl Reading {
    /// The reader's view of `conversation` and its `messages`, oldest first;
    /// `own` are the account's addresses, which a reply leaves out.
    pub(crate) fn new(
        conversation: &Conversation,
        messages: &[ConversationMessage],
        own: &[AddressInfo],
    ) -> Self {
        let mut lines = Vec::new();
        let mut quotes = Vec::new();
        let mut message_lines = Vec::new();
        let mut links: Vec<String> = Vec::new();
        let mut attachments = Vec::new();
        for (index, message) in messages.iter().enumerate() {
            if index > 0 {
                lines.extend([String::new(), SEPARATOR.to_owned(), String::new()]);
            }
            let start = lines.len();
            let (meta, verdict, words) = match message {
                ConversationMessage::Opened(message) => {
                    headers(
                        &mut lines,
                        &message.meta,
                        render::verdict_name(message.verdict),
                    );
                    // The parts of the body come after the files attached.
                    let (inline, attached): (Vec<_>, Vec<_>) =
                        message.attachments.iter().partition(|a| a.is_inline());
                    for attachment in attached.into_iter().chain(inline) {
                        let name = screen_line(&attachment.name);
                        let kind = if attachment.is_inline() {
                            "In the message"
                        } else {
                            "Attached"
                        };
                        lines.push(format!("{kind}: {name} ({})", size_label(attachment.size)));
                        attachments.push(ReadingAttachment {
                            message_id: message.meta.id.clone(),
                            id: attachment.id.clone(),
                            name,
                            size: attachment.size,
                            inline: attachment.is_inline(),
                        });
                    }
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
                    let words: Vec<&str> = body.split_whitespace().take(PREVIEW_WORDS).collect();
                    (
                        &message.meta,
                        render::verdict_name(message.verdict),
                        words.join(" "),
                    )
                }
                ConversationMessage::Unreadable(message) => {
                    headers(&mut lines, &message.meta, "not decrypted");
                    lines.push(String::new());
                    lines.push(screen_line(&format!(
                        "(This message could not be decrypted: {})",
                        message.error
                    )));
                    (&message.meta, "not decrypted", String::new())
                }
            };
            message_lines.push(ReadingMessage {
                lines: start..lines.len(),
                line: folded_line(meta, verdict, &words),
                unread: meta.is_unread(),
            });
        }
        Self {
            id: conversation.id.clone(),
            subject: screen_line(render::subject_or_empty(&conversation.subject)),
            lines,
            quotes,
            messages: message_lines,
            links,
            attachments,
            answer: answerable(messages, own).map(Box::new),
            wrapped: WrappedRows::default(),
        }
    }

    /// Whether the message at `index` is one the desktop's reader folds,
    /// which opens only the newest and those not read yet.
    pub(crate) fn folds(&self, index: usize) -> bool {
        index + 1 < self.messages.len() && !self.messages[index].unread
    }

    /// The message to bring into view as it opens, as the desktop's reader
    /// does: the first one not read yet, else the newest; none for a single
    /// message, which is in view already.
    pub(crate) fn landing(&self) -> Option<usize> {
        if self.messages.len() < 2 {
            return None;
        }
        let first_unread = self.messages.iter().position(|message| message.unread);
        first_unread.or(Some(self.messages.len() - 1))
    }
}

/// What an answer to a conversation of `messages`, oldest first, answers:
/// its last message that is not a draft, addressed as core would address a
/// reply, leaving out the account's `own` addresses, and from the one of
/// them it reached when mail can go out from it, as core sends it.
fn answerable(messages: &[ConversationMessage], own: &[AddressInfo]) -> Option<Answerable> {
    let draft = |meta: &MessageMetadata| {
        meta.label_ids
            .iter()
            .any(|label| label == label_ids::DRAFTS || label == label_ids::ALL_DRAFTS)
    };
    let (meta, reply_tos, files, opened): (
        &MessageMetadata,
        &[Recipient],
        Vec<ReadingAttachment>,
        bool,
    ) = messages
        .iter()
        .rev()
        .map(|message| match message {
            ConversationMessage::Opened(message) => (
                &message.meta,
                message.reply_tos.as_slice(),
                // A forward carries the parts of the body too.
                message
                    .attachments
                    .iter()
                    .map(|attachment| ReadingAttachment {
                        message_id: message.meta.id.clone(),
                        id: attachment.id.clone(),
                        name: screen_line(&attachment.name),
                        size: attachment.size,
                        inline: attachment.is_inline(),
                    })
                    .collect(),
                true,
            ),
            ConversationMessage::Unreadable(message) => (
                &message.meta,
                message.reply_tos.as_slice(),
                Vec::new(),
                false,
            ),
        })
        .find(|(meta, ..)| !draft(meta))?;
    let emails: Vec<String> = own.iter().map(|address| address.email.clone()).collect();
    let recipients = reply_recipients(meta, reply_tos, &emails);
    let from = own
        .iter()
        .find(|address| address.id == meta.address_id && address.can_send)
        .map(|address| screen_line(&address.email));
    let sender = meta.sender.address.trim();
    let redirected_from =
        (reply_redirected(meta, reply_tos) && !recipients.to.is_empty() && !sender.is_empty())
            .then(|| screen_line(sender));
    let named = if meta.sender.name.is_empty() {
        &meta.sender.address
    } else {
        &meta.sender.name
    };
    Some(Answerable {
        message_id: meta.id.clone(),
        from,
        subject: screen_line(render::subject_or_empty(&meta.subject)),
        sender: screen_line(named),
        to: recipients.to,
        cc: recipients.cc,
        redirected_from,
        opened,
        files,
    })
}

/// `bytes` as the desktop shows an attachment's size: in bytes under a
/// kilobyte, else KB, MB or GB, with one decimal below ten.
pub(crate) fn size_label(bytes: u64) -> String {
    const UNIT: f64 = 1024.0;
    // Under a kilobyte, a tenth of one would round a small file to nothing.
    if bytes < 1024 {
        return if bytes == 1 {
            "1 byte".to_owned()
        } else {
            format!("{bytes} bytes")
        };
    }
    let bytes = bytes as f64;
    for (limit, suffix) in [
        (UNIT, "KB"),
        (UNIT * UNIT, "MB"),
        (UNIT * UNIT * UNIT, "GB"),
    ] {
        if bytes < limit * UNIT {
            let value = bytes / limit;
            return if value < 10.0 {
                format!("{value:.1} {suffix}")
            } else {
                format!("{value:.0} {suffix}")
            };
        }
    }
    format!("{:.0} GB", bytes / (UNIT * UNIT * UNIT))
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
        // Counted once: counting again at each bracket taken off would cost
        // the square of a run of them, which a sender can make as long as
        // the body.
        let opening = address.matches('(').count();
        let mut closing = address.matches(')').count();
        while let Some(last) = address.chars().last() {
            // A closing bracket ends the address unless it closes one of its own.
            let unopened = last == ')' && opening < closing;
            if !(unopened || matches!(last, '.' | ',' | ';' | ':' | '!' | '?' | ']' | '}' | '*')) {
                break;
            }
            closing -= usize::from(last == ')');
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

/// Whether a line of a body quotes another message, as mail marks it.
pub(crate) fn is_quoted(line: &str) -> bool {
    line.starts_with('>')
}

/// The passages of a message's `body` that quote another message: runs of
/// lines that start with `>`, with the blank lines between them, at least
/// [`FOLDED_MIN_LINES`] long. A body that is all quotes has none, as the
/// desktop leaves it open: folded, nothing of it would show.
fn quoted_passages(body: &[String]) -> Vec<Range<usize>> {
    let quoted = |line: &String| is_quoted(line);
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

/// The line that stands for a folded message of `meta`, whose signature's
/// verdict is `verdict`: when it came, to the minute, who sent it, the
/// address beside the name as on the desktop, since a signature vouches for
/// the address alone, the verdict, and its first `words`.
fn folded_line(meta: &MessageMetadata, verdict: &str, words: &str) -> String {
    let time = render::fmt_time(meta.time);
    let when = time.get(..16).unwrap_or(&time);
    let mut line = format!("▸ {when} · {} · {verdict}", render::fmt_addr(&meta.sender));
    if !words.is_empty() {
        line = format!("{line} · {words}");
    }
    screen_line(&line)
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
mod tests;
