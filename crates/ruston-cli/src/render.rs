//! Output rendering: human-readable text by default, JSON with `--json`.

mod html;

use crate::terminal::{self, BodyFormat};
use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::cli::{EmptyFolder, ReadFormat};
use ruston_core::model::enums::label_ids;
use ruston_core::{
    AddressInfo, Attachment, Contact, ContactEmail, Conversation, ConversationMessage, Filter,
    FullMessage, Label, LabelCount, MessageMetadata, Recipient, Verdict,
};
use serde_json::json;

/// Render per-label counts.
pub fn counts(json_mode: bool, counts: &[LabelCount]) {
    if json_mode {
        json_out(&json!({ "counts": counts }));
        return;
    }
    for c in counts {
        terminal::println!(
            "  {:<14} total {:>6}  unread {:>6}",
            label_name(&c.label_id),
            c.total,
            c.unread
        );
    }
}

fn label_name(id: &str) -> &str {
    label_ids::name(id).unwrap_or(id)
}

/// Render a contacts list.
pub fn contacts_list(json_mode: bool, total: u32, contacts: &[Contact]) {
    if json_mode {
        json_out(&json!({ "total": total, "contacts": contacts }));
        return;
    }
    terminal::println!("{} contact(s) (total {total})", contacts.len());
    for c in contacts {
        let emails: Vec<&str> = c.emails.iter().map(|e| e.email.as_str()).collect();
        terminal::println!("  {}  {}  [{}]", c.id, c.name, emails.join(", "));
    }
}

/// Render contact email entries.
pub fn contact_emails(json_mode: bool, emails: &[ContactEmail]) {
    if json_mode {
        json_out(&json!({ "emails": emails }));
        return;
    }
    terminal::println!("{} email(s)", emails.len());
    for e in emails {
        terminal::println!("  {}  {}", e.email, e.name);
    }
}

/// Render the account's sending addresses.
pub fn addresses_list(json_mode: bool, addrs: &[AddressInfo]) {
    if json_mode {
        json_out(&json!({ "addresses": addrs }));
        return;
    }
    terminal::println!("{} address(es)", addrs.len());
    for a in addrs {
        terminal::println!("  {}  {}", a.email, a.id);
    }
}

/// Render a filter list.
pub fn filters_list(json_mode: bool, filters: &[Filter]) {
    if json_mode {
        let filters: Vec<_> = filters.iter().map(filter_json).collect();
        json_out(&json!({ "filters": filters }));
        return;
    }
    terminal::println!("{} filter(s)", filters.len());
    for f in filters {
        let state = if f.is_enabled() {
            "enabled"
        } else {
            "disabled"
        };
        terminal::println!("  [{state}] {}  {}", f.id, f.name);
    }
}

/// Render a newly created filter.
pub fn filter_created(json_mode: bool, f: &Filter) {
    if json_mode {
        json_out(&filter_json(f));
    } else {
        terminal::println!("Filter created: {} ({})", f.name, f.id);
    }
}

fn filter_json(f: &Filter) -> serde_json::Value {
    json!({ "ID": f.id, "Name": f.name, "Status": f.status, "Version": f.version })
}

/// Set for the offline demo, whose JSON says `"demo": true`, so a script
/// cannot take its fictional mail for real.
static DEMO: AtomicBool = AtomicBool::new(false);

/// Marks this run's JSON as the demo's.
pub fn mark_demo() {
    DEMO.store(true, Ordering::Relaxed);
}

/// Serialize a value as pretty JSON to stdout; in the demo, an object also
/// says `"demo": true`.
pub fn json_out<T: serde::Serialize + ?Sized>(value: &T) {
    if !DEMO.load(Ordering::Relaxed) {
        terminal::write_json(value);
        return;
    }
    // Demo data is small, so building it once more to carry the mark costs
    // nothing; real output is written as it is serialized.
    match serde_json::to_value(value) {
        Ok(mut marked) => {
            if let Some(object) = marked.as_object_mut() {
                object.insert("demo".into(), true.into());
            }
            terminal::write_json(&marked);
        }
        Err(_) => terminal::write_json(value),
    }
}

fn fmt_addr(r: &Recipient) -> String {
    if r.name.is_empty() || r.name == r.address {
        r.address.clone()
    } else {
        format!("{} <{}>", r.name, r.address)
    }
}

fn fmt_addrs(list: &[Recipient]) -> String {
    if list.is_empty() {
        "-".to_string()
    } else {
        list.iter().map(fmt_addr).collect::<Vec<_>>().join(", ")
    }
}

/// Format a Unix timestamp as `YYYY-MM-DD HH:MM:SS UTC` (no external deps).
pub fn fmt_time(ts: i64) -> String {
    if ts <= 0 {
        return "-".to_string();
    }
    let days = ts.div_euclid(86_400);
    let secs = ts.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    let (hh, mm, ss) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02}:{ss:02} UTC")
}

// Howard Hinnant's days-from-civil inverse (proleptic Gregorian).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

pub(crate) fn subject_or_empty(s: &str) -> &str {
    if s.is_empty() { "(no subject)" } else { s }
}

pub fn messages_list(json: bool, total: u32, msgs: &[MessageMetadata]) {
    if json {
        json_out(&json!({ "total": total, "messages": msgs }));
        return;
    }
    terminal::println!("{} message(s) (total {total})", msgs.len());
    for m in msgs {
        let mark = if m.unread != 0 { "*" } else { " " };
        terminal::println!();
        terminal::println!("{mark} {}", subject_or_empty(&m.subject));
        terminal::println!("    from: {}", fmt_addr(&m.sender));
        terminal::println!("    date: {}", fmt_time(m.time));
        if m.num_attachments > 0 {
            terminal::println!("    attachments: {}", m.num_attachments);
        }
        terminal::println!("    id:   {}", m.id);
    }
}

pub fn conversations_list(json: bool, total: u32, convs: &[Conversation]) {
    if json {
        json_out(&json!({ "total": total, "conversations": convs }));
        return;
    }
    terminal::println!("{} conversation(s) (total {total})", convs.len());
    for c in convs {
        let mark = if c.num_unread != 0 { "*" } else { " " };
        terminal::println!();
        terminal::println!("{mark} {}", subject_or_empty(&c.subject));
        terminal::println!("    from:     {}", fmt_addrs(&c.senders));
        terminal::println!("    messages: {} ({} unread)", c.num_messages, c.num_unread);
        if c.num_attachments > 0 {
            terminal::println!("    attachments: {}", c.num_attachments);
        }
        terminal::println!("    id:       {}", c.id);
    }
}

/// A verdict's name, as `--json` writes it.
fn verdict_name(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Verified => "verified",
        Verdict::Unsigned => "unsigned",
        Verdict::Unverified => "unverified",
        Verdict::Invalid => "invalid",
    }
}

fn print_message_header(m: &MessageMetadata, verdict: &str, mime_type: &str) {
    terminal::println!("Subject: {}", subject_or_empty(&m.subject));
    terminal::println!("From:    {}", fmt_addr(&m.sender));
    terminal::println!("To:      {}", fmt_addrs(&m.to_list));
    if !m.cc_list.is_empty() {
        terminal::println!("Cc:      {}", fmt_addrs(&m.cc_list));
    }
    terminal::println!("Date:    {}", fmt_time(m.time));
    terminal::println!("Verdict: {verdict}");
    terminal::println!("Type:    {mime_type}");
    terminal::println!("Id:      {}", m.id);
}

/// How much of a message [`full_message`] writes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MessageView {
    /// Its headers, attachments and body.
    Whole,
    /// Only its body.
    BodyOnly,
}

impl MessageView {
    /// The view the `--body-only` flag asks for.
    pub fn chosen(body_only: bool) -> Self {
        if body_only {
            Self::BodyOnly
        } else {
            Self::Whole
        }
    }
}

/// Which of the account's labels a listing holds.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LabelKind {
    /// Labels, which a message can carry several of.
    Labels,
    /// Folders, which a message is in one at a time.
    Folders,
}

impl LabelKind {
    /// The kind the `--folders` flag asks for.
    pub fn chosen(folders: bool) -> Self {
        if folders { Self::Folders } else { Self::Labels }
    }
}

pub fn full_message(json: bool, msg: &FullMessage, format: ReadFormat, view: MessageView) {
    if json {
        if view == MessageView::BodyOnly {
            // Borrowed, not copied into a `json!` value: bodies can be large.
            #[derive(serde::Serialize)]
            struct Body<'a> {
                body: &'a str,
            }
            json_out(&Body { body: &msg.body });
        } else {
            json_out(msg);
        }
        return;
    }
    let body = displayed_body(&msg.body, &msg.mime_type, format);
    let body_format = if matches!(format, ReadFormat::Raw | ReadFormat::Html) {
        BodyFormat::Named
    } else {
        BodyFormat::Readable
    };
    if view == MessageView::BodyOnly || matches!(format, ReadFormat::Raw) {
        terminal::print_body(&body, body_format);
        return;
    }
    print_message_header(&msg.meta, verdict_name(msg.verdict), &msg.mime_type);
    if !msg.attachments.is_empty() {
        terminal::println!("Attachments:");
        for a in &msg.attachments {
            terminal::println!("  - {} ({} bytes, id {})", a.name, a.size, a.id);
        }
    }
    terminal::println!();
    terminal::print_body(&body, body_format);
}

fn displayed_body<'a>(body: &'a str, mime_type: &str, format: ReadFormat) -> Cow<'a, str> {
    if ruston_core::html::is_html_mime(mime_type) && matches!(format, ReadFormat::Text) {
        Cow::Owned(html::to_markdown(body))
    } else {
        Cow::Borrowed(body)
    }
}

/// A conversation as `--json` writes it. Borrowed, not copied into a `json!`
/// value: a thread holds every message's body.
#[derive(serde::Serialize)]
struct Thread<'a> {
    conversation: &'a Conversation,
    messages: Vec<ThreadMessage<'a>>,
}

/// An opened message is written as its `FullMessage` serializes; one that
/// could not be opened keeps its headers and says why, with no body.
#[derive(serde::Serialize)]
#[serde(untagged)]
enum ThreadMessage<'a> {
    Opened(&'a FullMessage),
    Unreadable {
        meta: &'a MessageMetadata,
        reply_tos: &'a [Recipient],
        error: String,
    },
}

impl<'a> Thread<'a> {
    fn new(conversation: &'a Conversation, messages: &'a [ConversationMessage]) -> Self {
        let messages = messages
            .iter()
            .map(|message| match message {
                ConversationMessage::Opened(message) => ThreadMessage::Opened(message),
                ConversationMessage::Unreadable(unreadable) => ThreadMessage::Unreadable {
                    meta: &unreadable.meta,
                    reply_tos: &unreadable.reply_tos,
                    error: unreadable.error.to_string(),
                },
            })
            .collect();
        Self {
            conversation,
            messages,
        }
    }
}

pub fn conversation_read(json: bool, conv: &Conversation, msgs: &[ConversationMessage]) {
    if json {
        json_out(&Thread::new(conv, msgs));
        return;
    }
    terminal::println!("Conversation: {}", subject_or_empty(&conv.subject));
    terminal::println!(
        "Messages: {} ({} unread)",
        conv.num_messages,
        conv.num_unread
    );
    terminal::println!("Id: {}", conv.id);
    for (i, m) in msgs.iter().enumerate() {
        terminal::println!("\n--- message {}/{} ---", i + 1, msgs.len());
        let m = match m {
            ConversationMessage::Opened(m) => m,
            ConversationMessage::Unreadable(unreadable) => {
                print_message_header(&unreadable.meta, "not decrypted", "-");
                terminal::println!();
                terminal::println!(
                    "(This message could not be decrypted: {})",
                    unreadable.error
                );
                continue;
            }
        };
        print_message_header(&m.meta, verdict_name(m.verdict), &m.mime_type);
        terminal::println!();
        terminal::print_body(&thread_body(m), BodyFormat::Readable);
    }
}

/// What a thread shows of an opened message: what `messages read` shows by
/// default, HTML as text and a huge body cut short.
fn thread_body(message: &FullMessage) -> Cow<'_, str> {
    displayed_body(&message.body, &message.mime_type, ReadFormat::Text)
}

pub fn labels_list(json: bool, labels: &[Label], kind: LabelKind) {
    let noun = match kind {
        LabelKind::Labels => "label",
        LabelKind::Folders => "folder",
    };
    if json {
        json_out(&json!({ format!("{noun}s"): labels }));
        return;
    }
    terminal::println!("{} {noun}(s)", labels.len());
    for l in labels {
        let parent = l
            .parent_id
            .as_deref()
            .map(|p| format!("  parent={p}"))
            .unwrap_or_default();
        terminal::println!("  {} {}  ({}){}", l.id, l.name, l.color, parent);
    }
}

pub fn label_created(json: bool, label: &Label) {
    if json {
        json_out(label);
        return;
    }
    terminal::println!("Created '{}' (id {})", label.name, label.id);
}

pub fn attachments_list(json: bool, atts: &[Attachment]) {
    if json {
        json_out(&json!({ "attachments": atts }));
        return;
    }
    terminal::println!("{} attachment(s)", atts.len());
    for a in atts {
        let mime = a.mime_type.as_deref().unwrap_or("application/octet-stream");
        let disp = a.disposition.as_deref().unwrap_or("attachment");
        terminal::println!("  {} {}  ({} bytes, {mime}, {disp})", a.id, a.name, a.size);
    }
}

/// Report the attachment files a download wrote, where each one landed.
pub fn downloaded(json: bool, files: &[String]) {
    if json {
        json_out(&json!({ "status": "ok", "count": files.len(), "files": files }));
        return;
    }
    terminal::println!("Downloaded {} file(s):", files.len());
    for file in files {
        terminal::println!("  {file}");
    }
}

/// Report what a command did to the items `ids` names; `action` is the
/// command's own name, as typed (`move`, `undelete`, `enable`...).
pub fn action_result(json: bool, action: &str, ids: &[String]) {
    if json {
        json_out(&json!({ "status": "ok", "action": action, "count": ids.len(), "ids": ids }));
        return;
    }
    terminal::println!("{action}: {} item(s)", ids.len());
}

/// Report a setting turned on or off: `setting` names it in JSON, and
/// `label` in text.
pub fn setting_changed(json: bool, setting: &str, label: &str, on: bool) {
    if json {
        json_out(&json!({ "status": "ok", "setting": setting, "value": on }));
        return;
    }
    terminal::println!("{label}: {}", if on { "on" } else { "off" });
}

/// Report a folder whose messages were all deleted for good.
pub fn folder_emptied(json: bool, folder: EmptyFolder) {
    if json {
        json_out(&json!({ "status": "ok", "action": "empty", "folder": folder.as_str() }));
        return;
    }
    terminal::println!("Emptied {}.", folder.name());
}

/// Report a message body written to a file instead of shown.
pub fn body_written(json: bool, bytes: usize, path: &std::path::Path) {
    if json {
        json_out(&json!({ "status": "ok", "bytes": bytes, "path": path.display().to_string() }));
        return;
    }
    terminal::println!("Wrote {bytes} bytes to {}", path.display());
}

/// Report a successful send, with the sent message's ID.
pub fn sent(json: bool, id: &str) {
    if json {
        json_out(&json!({ "status": "ok", "id": id }));
        return;
    }
    terminal::println!("Sent (id {id})");
}

/// Report a message Proton holds until `at` (Unix time): it has not left yet.
pub fn scheduled(json: bool, id: &str, at: i64) {
    if json {
        json_out(&json!({ "status": "ok", "id": id, "scheduled_at": at }));
        return;
    }
    terminal::println!("{}", scheduled_text(id, at));
}

fn scheduled_text(id: &str, at: i64) -> String {
    format!("Scheduled (id {id}) to be sent at {}", fmt_time(at))
}

/// Report a saved draft, which was not sent, with its message ID.
pub fn draft_saved(json: bool, id: &str) {
    if json {
        json_out(&json!({ "status": "ok", "id": id }));
        return;
    }
    terminal::println!("Draft saved (id {id})");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_verdict_is_named_as_json_writes_it() {
        for verdict in [
            Verdict::Verified,
            Verdict::Unsigned,
            Verdict::Unverified,
            Verdict::Invalid,
        ] {
            assert_eq!(
                serde_json::to_value(verdict).unwrap(),
                verdict_name(verdict)
            );
        }
    }

    #[test]
    fn text_format_converts_html_without_changing_explicit_formats() {
        let html = "<p>Hello <b>team</b></p>";
        assert_eq!(
            displayed_body(html, "Text/HTML; charset=UTF-8", ReadFormat::Text),
            "Hello **team**"
        );
        assert_eq!(displayed_body(html, "text/html", ReadFormat::Html), html);
        assert_eq!(displayed_body(html, "text/html", ReadFormat::Raw), html);
        assert_eq!(
            displayed_body("Hello team", "text/plain", ReadFormat::Text),
            "Hello team"
        );
    }

    #[test]
    fn large_html_is_bounded_only_in_the_converted_text_view() {
        let html = format!("<p>{}</p>", "x".repeat(2 * 1024 * 1024));
        let text = displayed_body(&html, "text/html", ReadFormat::Text);
        assert!(text.len() < html.len());
        assert!(text.contains("[Message truncated."));
        for format in [ReadFormat::Html, ReadFormat::Raw] {
            assert_eq!(displayed_body(&html, "text/html", format), html);
        }
    }

    #[test]
    fn fmt_time_handles_epoch_and_known_values() {
        // Non-positive timestamps render as a placeholder.
        assert_eq!(fmt_time(0), "-");
        assert_eq!(fmt_time(-5), "-");
        // 1 second past the Unix epoch.
        assert_eq!(fmt_time(1), "1970-01-01 00:00:01 UTC");
        // A well-known timestamp: 1_700_000_000 = 2023-11-14 22:13:20 UTC.
        assert_eq!(fmt_time(1_700_000_000), "2023-11-14 22:13:20 UTC");
        // And the first second of 2021.
        assert_eq!(fmt_time(1_609_459_200), "2021-01-01 00:00:00 UTC");
    }

    #[test]
    fn a_scheduled_message_says_when_it_leaves_not_that_it_was_sent() {
        assert_eq!(
            scheduled_text("m1", 1_900_000_000),
            "Scheduled (id m1) to be sent at 2030-03-17 17:46:40 UTC"
        );
    }

    #[test]
    fn civil_from_days_handles_leap_day() {
        // 2024-02-29 00:00:00 UTC = day 19782 since the epoch; the hand-rolled
        // proleptic-Gregorian inverse must land on the leap day exactly.
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
    }

    #[test]
    fn label_name_maps_system_ids_and_passes_through() {
        assert_eq!(label_name("0"), "inbox");
        assert_eq!(label_name("3"), "trash");
        assert_eq!(label_name("10"), "starred");
        assert_eq!(label_name("9"), "outbox");
        assert_eq!(label_name("12"), "scheduled");
        assert_eq!(label_name("15"), "almost-all");
        assert_eq!(label_name("16"), "snoozed");
        // Unknown / custom label ids pass through unchanged.
        assert_eq!(label_name("custom-xyz"), "custom-xyz");
    }

    #[test]
    fn a_thread_shows_html_as_text_and_cuts_a_huge_body_short() {
        let message = |body: String| FullMessage {
            meta: MessageMetadata::default(),
            body,
            mime_type: "text/html".into(),
            verdict: ruston_core::Verdict::Verified,
            attachments: Vec::new(),
            reply_tos: Vec::new(),
        };
        let read = thread_body(&message("<p>Hi <b>team</b></p>".into())).into_owned();
        assert!(read.contains("Hi"), "{read}");
        assert!(!read.contains("<p>"), "{read}");

        let huge = format!("<p>{}</p>", "x".repeat(2 * 1024 * 1024));
        let huge_message = message(huge.clone());
        let read = thread_body(&huge_message);
        assert!(read.len() < huge.len());
        assert!(read.contains("[Message truncated."));
    }

    #[test]
    fn an_unreadable_thread_message_says_why_and_has_no_body() {
        let opened = FullMessage {
            meta: MessageMetadata {
                id: "m0".into(),
                ..Default::default()
            },
            body: "Hello".into(),
            mime_type: "text/plain".into(),
            verdict: ruston_core::Verdict::Verified,
            attachments: Vec::new(),
            reply_tos: Vec::new(),
        };
        let conversation = Conversation::default();
        let messages = [
            ConversationMessage::Opened(opened.clone()),
            ConversationMessage::Unreadable(ruston_core::UnreadableMessage {
                meta: MessageMetadata {
                    id: "m1".into(),
                    ..Default::default()
                },
                reply_tos: Vec::new(),
                error: ruston_core::Error::Crypto("body decrypt failed".into()),
            }),
        ];

        let json = serde_json::to_value(Thread::new(&conversation, &messages)).unwrap();

        // An opened message is written exactly as its `FullMessage` serializes.
        assert_eq!(json["messages"][0], serde_json::to_value(&opened).unwrap());
        let unreadable = &json["messages"][1];
        assert_eq!(unreadable["meta"]["ID"], "m1");
        assert!(
            unreadable["error"]
                .as_str()
                .is_some_and(|error| error.contains("body decrypt failed"))
        );
        assert!(unreadable.get("body").is_none());
    }
}
