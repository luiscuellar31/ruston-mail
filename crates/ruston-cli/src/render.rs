//! What commands print: text for people, or JSON for scripts with `--json`.

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

/// Show each folder's and label's total and unread count.
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

/// Show a page of contacts, each with its addresses, and how many there are
/// in all.
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

/// Show the email addresses contacts hold, each with its contact's name.
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

/// Show each address the account sends from, with its ID.
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

/// Show the Sieve filters, each with whether it is enabled.
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

/// Report the filter just created, by its name and ID.
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

pub(crate) fn fmt_addr(r: &Recipient) -> String {
    if r.name.is_empty() || r.name == r.address {
        r.address.clone()
    } else {
        format!("{} <{}>", r.name, r.address)
    }
}

/// The recipients on one line, separated by commas, or `-` for none.
pub(crate) fn fmt_addrs(list: &[Recipient]) -> String {
    match list {
        [] => "-".to_owned(),
        recipients => recipients
            .iter()
            .map(fmt_addr)
            .collect::<Vec<_>>()
            .join(", "),
    }
}

/// Seconds in a day.
const SECONDS_PER_DAY: i64 = 86_400;
/// Days in 400 years of the Gregorian calendar, after which it repeats.
const DAYS_PER_400_YEARS: i64 = 146_097;

/// A Unix time as `YYYY-MM-DD HH:MM:SS UTC`, or `-` for a time not set.
pub fn fmt_time(ts: i64) -> String {
    if ts <= 0 {
        return "-".to_string();
    }
    let date = Date::after_epoch(ts / SECONDS_PER_DAY);
    let second = ts % SECONDS_PER_DAY;
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
        date.year,
        date.month,
        date.day,
        second / 3600,
        second / 60 % 60,
        second % 60
    )
}

/// A day of the proleptic Gregorian calendar.
#[derive(Debug, PartialEq, Eq)]
struct Date {
    year: i64,
    month: u32,
    day: u32,
}

impl Date {
    /// The day `days` days after 1970-01-01, or before it when negative.
    fn after_epoch(days: i64) -> Self {
        // Whole 400-year cycles first, since the calendar repeats after each;
        // the rest is walked a year, and then a month, at a time.
        let mut year = 1970 + days.div_euclid(DAYS_PER_400_YEARS) * 400;
        let mut day = days.rem_euclid(DAYS_PER_400_YEARS);
        while day >= days_in_year(year) {
            day -= days_in_year(year);
            year += 1;
        }
        let mut month = 1;
        while day >= days_in_month(year, month) {
            day -= days_in_month(year, month);
            month += 1;
        }
        Date {
            year,
            month,
            day: day as u32 + 1,
        }
    }
}

fn is_leap_year(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn days_in_year(year: i64) -> i64 {
    if is_leap_year(year) { 366 } else { 365 }
}

fn days_in_month(year: i64, month: u32) -> i64 {
    match month {
        2 if is_leap_year(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
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
pub(crate) fn verdict_name(verdict: Verdict) -> &'static str {
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
pub(crate) fn thread_body(message: &FullMessage) -> Cow<'_, str> {
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
/// command's own name, as typed (`move`, `delete`, `enable`...).
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
    fn a_time_shows_in_utc_to_the_second_or_as_a_dash_when_not_set() {
        // Zero is a time not set; earlier ones show the same way.
        for unset in [0, -86_400] {
            assert_eq!(fmt_time(unset), "-", "{unset}");
        }
        for (time, shown) in [
            (59, "1970-01-01 00:00:59 UTC"),
            (86_399, "1970-01-01 23:59:59 UTC"),
            (951_782_400, "2000-02-29 00:00:00 UTC"),
            (1_234_567_890, "2009-02-13 23:31:30 UTC"),
        ] {
            assert_eq!(fmt_time(time), shown, "{time}");
        }
    }

    #[test]
    fn a_scheduled_message_says_when_it_leaves_not_that_it_was_sent() {
        assert_eq!(
            scheduled_text("m1", 1_900_000_000),
            "Scheduled (id m1) to be sent at 2030-03-17 17:46:40 UTC"
        );
    }

    #[test]
    fn dates_follow_the_gregorian_calendar() {
        let date = |year, month, day| Date { year, month, day };
        for (days, expected) in [
            (0, date(1970, 1, 1)),
            (-1, date(1969, 12, 31)),
            // A leap day every fourth year...
            (19_782, date(2024, 2, 29)),
            // ...but of the century years only every fourth.
            (11_016, date(2000, 2, 29)),
            (-25_509, date(1900, 2, 28)),
            (-25_508, date(1900, 3, 1)),
            (2_932_896, date(9999, 12, 31)),
        ] {
            assert_eq!(Date::after_epoch(days), expected, "{days}");
        }
        // The latest time a timestamp can hold is still shown, and exactly.
        assert_eq!(fmt_time(i64::MAX), "292277026596-12-04 15:30:07 UTC");
    }

    #[test]
    fn system_labels_show_by_the_name_their_options_take() {
        use ruston_core::model::enums::resolve_folder;

        for (id, name) in [
            ("0", "inbox"),
            ("3", "trash"),
            ("9", "outbox"),
            ("10", "starred"),
            ("12", "scheduled"),
            ("15", "almost-all"),
            ("16", "snoozed"),
        ] {
            assert_eq!(label_name(id), name);
            assert_eq!(resolve_folder(name), id, "{name}");
        }
        // A label of the account's own is shown by its ID.
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
