//! Offline demo mode providing a self-contained simulated mailbox.
//!
//! Enables testing, CI automation, and CLI demonstrations without contacting Proton
//! or requiring real user credentials. Activated via `--demo` or `RUSTON_DEMO=1`.

use crate::terminal;
use std::sync::Mutex;

use ruston_core::mail::organize::folder_id_among;
use ruston_core::mail::read::search_window;
use ruston_core::mail::send::{forward_subject, reply_subject};
use ruston_core::model::ConversationLabel;
use ruston_core::model::enums::{label_ids, label_type, message_flag};
use ruston_core::{
    Attachment, Conversation, ConversationMessage, Error, FullMessage, LabelCount, MessageMetadata,
    Quote, Recipient, RecipientProtection, Result, SearchOpts, Verdict,
};

use crate::cli::{Command, Ctx, unix_now};
use crate::mailbox::{Change, Outgoing, OutgoingKind, someone_matches, within};
use crate::render;

mod commands;
mod data;

#[cfg(test)]
pub(crate) use data::demo_counts;
use data::{DemoMailbox, fixtures};
pub(crate) use data::{demo_addresses, demo_labels};

/// Said on standard error before every demo command, so a fictional send,
/// move or deletion is not taken for a real one. `RUSTON_DEMO` left set in
/// the environment turns the demo on without `--demo`.
const NOTICE: &str =
    "note: offline demo (--demo or RUSTON_DEMO); nothing is sent to or changed on Proton";

/// Fictional user details.
pub(crate) const DEMO_EMAIL: &str = "demo@example.com";
const DEMO_NAME: &str = "Demo User";
const DEMO_ALIAS: &str = "volunteers@example.org";

#[derive(Clone)]
struct DemoMessage {
    meta: MessageMetadata,
    body: String,
    mime_type: String,
    attachments: Vec<Attachment>,
}

impl DemoMessage {
    fn to_full(&self) -> FullMessage {
        FullMessage {
            meta: self.meta.clone(),
            body: self.body.clone(),
            mime_type: self.mime_type.clone(),
            verdict: Verdict::Verified,
            attachments: self.attachments.clone(),
            reply_tos: Vec::new(),
        }
    }
}

/// A plain-text message with nothing attached, as most of the demo's are.
impl Default for DemoMessage {
    fn default() -> Self {
        Self {
            meta: MessageMetadata::default(),
            body: String::new(),
            mime_type: "text/plain".to_string(),
            attachments: Vec::new(),
        }
    }
}

/// Page `page` of `items`, `page_size` to a page: empty past the end, as a
/// real listing is, however far past it the page is asked for.
fn page_of<T>(items: Vec<T>, page: u32, page_size: u32) -> Vec<T> {
    let start = u64::from(page) * u64::from(page_size);
    items
        .into_iter()
        .skip(usize::try_from(start).unwrap_or(usize::MAX))
        .take(page_size as usize)
        .collect()
}

/// Page `page` of `conversations` in `label`, newest first, with how many
/// there are in all; `is_all` takes every one, and `unread` only the unread.
fn list_page(
    conversations: Vec<Conversation>,
    label: &str,
    is_all: bool,
    unread: bool,
    page: u32,
    page_size: u32,
) -> (u32, Vec<Conversation>) {
    let mut matching: Vec<Conversation> = conversations
        .into_iter()
        .filter(|c| {
            is_all
                || c.labels
                    .iter()
                    .any(|l| l.id == label && l.context_num_messages.unwrap_or(0) > 0)
        })
        .filter(|c| !unread || c.has_unread())
        .collect();
    matching.sort_by_key(|a| std::cmp::Reverse(a.time));
    let total = matching.len() as u32;
    (total, page_of(matching, page, page_size))
}

/// The conversation whose ID is or holds `id`, with its messages, all of
/// which open in the demo.
fn read_conversation(
    messages: Vec<DemoMessage>,
    conversations: Vec<Conversation>,
    id: &str,
) -> Result<(Conversation, Vec<ConversationMessage>)> {
    let conversation = conversations
        .into_iter()
        .find(|c| c.id == id || c.id.contains(id))
        .ok_or_else(|| Error::Other(format!("demo conversation not found for id '{id}'")))?;
    let messages = messages
        .into_iter()
        .filter(|m| m.meta.conversation_id == conversation.id)
        .map(|m| ConversationMessage::Opened(m.to_full()))
        .collect();
    Ok((conversation, messages))
}

/// How Proton would protect a message from the demo to `address`: end to
/// end for Proton's own domains, as the desktop's demo has it, and without
/// end-to-end encryption for any other.
pub(crate) fn protection(address: &str) -> RecipientProtection {
    let domain = address.rsplit('@').next().unwrap_or_default();
    let proton = ["proton.me", "protonmail.com", "pm.me"]
        .iter()
        .any(|known| domain.eq_ignore_ascii_case(known));
    if proton {
        RecipientProtection::EndToEnd
    } else {
        RecipientProtection::Unencrypted
    }
}

/// The demo's mail as the terminal interface changes it: the fixtures at
/// first, then each change it makes and each message it sends, for as long
/// as it runs. Its counts come from its conversations, so the two always
/// agree.
pub(crate) struct DemoSession {
    conversations: Mutex<Vec<Conversation>>,
    messages: Mutex<Vec<DemoMessage>>,
}

impl DemoSession {
    pub(crate) fn new() -> Self {
        let DemoMailbox {
            messages,
            conversations,
        } = fixtures();
        Self {
            conversations: Mutex::new(conversations),
            messages: Mutex::new(messages),
        }
    }

    /// The conversation whose ID is or holds `id`, with its messages, as
    /// `conversations read` shows it.
    pub(crate) fn conversation(
        &self,
        id: &str,
    ) -> Result<(Conversation, Vec<ConversationMessage>)> {
        let messages = self
            .messages
            .lock()
            .expect("demo mailbox lock poisoned")
            .clone();
        read_conversation(messages, self.snapshot(), id)
    }

    /// Keeps `outgoing` as a message the demo's user sent just now, as
    /// Proton would: a new one in a conversation of its own, an answer in
    /// the conversation of the message it answers, with the subject core
    /// gives it and, unless left out, that message quoted under its text as
    /// core quotes it in plain text. It carries the files attached, by their
    /// names and sizes, and a forward the original's but those left out.
    /// Sent lists either.
    pub(crate) fn send(&self, outgoing: &Outgoing) {
        let time = unix_now();
        let people = |addresses: &[String]| -> Vec<Recipient> {
            addresses.iter().map(Recipient::new).collect()
        };
        let sender = data::USER.recipient();
        let mut messages = self.messages.lock().expect("demo mailbox lock poisoned");
        let number = messages.len() + 1;
        let answered = match &outgoing.kind {
            OutgoingKind::New => None,
            OutgoingKind::Reply { message_id, .. } | OutgoingKind::Forward { message_id, .. } => {
                messages.iter().find(|m| m.meta.id == *message_id).cloned()
            }
        };
        let (subject, conversation_id) = match (&outgoing.kind, &answered) {
            (OutgoingKind::Reply { .. }, Some(original)) => (
                reply_subject(&original.meta.subject),
                original.meta.conversation_id.clone(),
            ),
            (OutgoingKind::Forward { .. }, Some(original)) => (
                forward_subject(&original.meta.subject),
                original.meta.conversation_id.clone(),
            ),
            _ => (outgoing.subject.clone(), format!("conv-demo-sent-{number}")),
        };
        let body = match &answered {
            Some(original) if outgoing.kind.quote() == Quote::Original => {
                format!("{}\n\n{}", outgoing.body, quoted(original))
            }
            _ => outgoing.body.clone(),
        };
        let forwarded = match (&outgoing.kind, &answered) {
            (OutgoingKind::Forward { leave_out, .. }, Some(original)) => original
                .attachments
                .iter()
                .filter(|attachment| !leave_out.contains(&attachment.id))
                .cloned()
                .collect(),
            _ => Vec::new(),
        };
        let attached = outgoing.attachments.iter().map(|path| Attachment {
            name: path
                .file_name()
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned()),
            size: std::fs::metadata(path).map_or(0, |metadata| metadata.len()),
            disposition: Some("attachment".to_string()),
            ..Attachment::default()
        });
        let attachments: Vec<Attachment> = forwarded
            .into_iter()
            .chain(attached)
            .enumerate()
            .map(|(index, attachment)| Attachment {
                id: format!("att-demo-sent-{number}-{}", index + 1),
                ..attachment
            })
            .collect();
        let meta = MessageMetadata {
            id: format!("msg-demo-sent-{number}"),
            order: i64::try_from(number).unwrap_or(i64::MAX),
            conversation_id: conversation_id.clone(),
            subject: subject.clone(),
            unread: 0,
            sender: sender.clone(),
            to_list: people(&outgoing.to),
            cc_list: people(&outgoing.cc),
            bcc_list: people(&outgoing.bcc),
            time,
            size: body.len() as u64,
            address_id: "addr-demo-01".to_string(),
            label_ids: vec![label_ids::SENT.to_string()],
            external_id: None,
            num_attachments: i64::try_from(attachments.len()).unwrap_or(i64::MAX),
            flags: message_flag::SENT,
            expiration_time: None,
        };
        let carries = meta.num_attachments;
        let recipients: Vec<Recipient> = meta
            .to_list
            .iter()
            .chain(&meta.cc_list)
            .chain(&meta.bcc_list)
            .cloned()
            .collect();
        messages.push(DemoMessage {
            meta,
            body,
            mime_type: "text/plain".to_string(),
            attachments,
        });
        drop(messages);
        let sent = |messages: i64| ConversationLabel {
            id: label_ids::SENT.to_string(),
            context_num_messages: Some(messages),
            context_num_unread: Some(0),
            context_time: Some(time),
        };
        let mut conversations = self
            .conversations
            .lock()
            .expect("demo mailbox lock poisoned");
        match conversations.iter_mut().find(|c| c.id == conversation_id) {
            Some(conversation) => {
                conversation.num_messages += 1;
                conversation.num_attachments += carries;
                conversation.time = time;
                match conversation
                    .labels
                    .iter_mut()
                    .find(|l| l.id == label_ids::SENT)
                {
                    Some(label) => {
                        label.context_num_messages =
                            Some(label.context_num_messages.unwrap_or(0) + 1);
                        label.context_time = Some(time);
                    }
                    None => conversation.labels.push(sent(1)),
                }
            }
            None => conversations.push(Conversation {
                id: conversation_id,
                subject,
                time,
                num_messages: 1,
                num_unread: 0,
                num_attachments: carries,
                senders: vec![sender],
                recipients,
                labels: vec![sent(1)],
                expiration_time: None,
            }),
        }
    }

    /// The attachment `attachment_id` of the message `message_id`: its name
    /// and the stand-in a download of it saves, as the demo's
    /// `attachments download` saves one.
    pub(crate) fn download_attachment(
        &self,
        message_id: &str,
        attachment_id: &str,
    ) -> Result<(String, Vec<u8>)> {
        let messages = self.messages.lock().expect("demo mailbox lock poisoned");
        let attachment = messages
            .iter()
            .find(|message| message.meta.id == message_id)
            .and_then(|message| message.attachments.iter().find(|a| a.id == attachment_id))
            .ok_or_else(|| Error::NotFound {
                kind: "attachment".into(),
            })?;
        Ok((
            attachment.name.clone(),
            simulated_attachment(&attachment.name),
        ))
    }

    /// The conversations as they stand now.
    fn snapshot(&self) -> Vec<Conversation> {
        self.conversations
            .lock()
            .expect("demo mailbox lock poisoned")
            .clone()
    }

    /// Page `page` of the conversations in the folder or label `label`, as
    /// `conversations list` would list them.
    pub(crate) fn conversations(
        &self,
        label: &str,
        page: u32,
        page_size: u32,
    ) -> (u32, Vec<Conversation>) {
        let is_all = label == label_ids::ALL_MAIL;
        list_page(self.snapshot(), label, is_all, false, page, page_size)
    }

    /// Page `page` of the conversations a search for `opts` finds, as
    /// `conversations search` finds them, its folder named by label ID,
    /// newest first, with how many there are in all.
    pub(crate) fn search(
        &self,
        opts: &SearchOpts,
        page: u32,
        page_size: u32,
    ) -> Result<(u32, Vec<Conversation>)> {
        let window = search_window(opts.after.as_deref(), opts.before.as_deref())?;
        let label = opts
            .folder
            .as_deref()
            .filter(|label| *label != label_ids::ALL_MAIL);
        let mut found: Vec<Conversation> = self
            .snapshot()
            .into_iter()
            .filter(|conversation| finds(conversation, opts, window, label))
            .collect();
        found.sort_by_key(|conversation| std::cmp::Reverse(conversation.time));
        let total = found.len() as u32;
        Ok((total, page_of(found, page, page_size)))
    }

    /// How many conversations each place holds, and how many are unread.
    pub(crate) fn counts(&self) -> Vec<LabelCount> {
        conversation_counts(&self.snapshot())
    }

    /// Makes `change` to the conversation `id`, listed in `place`, much as
    /// Proton would: a star labels it Starred, reading marks it and all its
    /// messages read wherever it is, and unread only where it is listed,
    /// with its newest message there when none was unread, a move takes it
    /// and its messages out of the folders they were in, keeping their
    /// labels, Sent and Drafts, and deleting it forever takes it away whole,
    /// as a move brings all of it to Trash or Spam.
    pub(crate) fn change(&self, id: &str, place: &str, change: &Change) {
        let mut conversations = self
            .conversations
            .lock()
            .expect("demo mailbox lock poisoned");
        let Some(conversation) = conversations.iter_mut().find(|c| c.id == id) else {
            return;
        };
        let whole = |id: &str, conversation: &Conversation| ConversationLabel {
            id: id.to_string(),
            context_num_messages: Some(conversation.num_messages),
            context_num_unread: Some(conversation.num_unread),
            context_time: Some(conversation.time),
        };
        match change {
            Change::Star(true) => {
                if !conversation
                    .labels
                    .iter()
                    .any(|l| l.id == label_ids::STARRED)
                {
                    let starred = whole(label_ids::STARRED, conversation);
                    conversation.labels.push(starred);
                }
            }
            Change::Star(false) => conversation.labels.retain(|l| l.id != label_ids::STARRED),
            Change::Read(true) => {
                conversation.num_unread = 0;
                for label in &mut conversation.labels {
                    label.context_num_unread = Some(0);
                }
                let mut messages = self.messages.lock().expect("demo mailbox lock poisoned");
                for message in messages.iter_mut().filter(|m| m.meta.conversation_id == id) {
                    message.meta.unread = 0;
                }
            }
            Change::Read(false) => {
                // Proton marks its newest message in the place unread. The
                // demo's messages keep the places they started in and none
                // is in All mail, so where none is, its newest is.
                if !conversation.has_unread() {
                    let mut messages = self.messages.lock().expect("demo mailbox lock poisoned");
                    let newest = |in_place: bool| {
                        let held = messages.iter().enumerate().filter(|(_, m)| {
                            m.meta.conversation_id == id
                                && (!in_place || m.meta.label_ids.iter().any(|l| l == place))
                        });
                        held.max_by_key(|(_, m)| m.meta.order).map(|(at, _)| at)
                    };
                    if let Some(at) = newest(true).or_else(|| newest(false)) {
                        messages[at].meta.unread = 1;
                    }
                }
                conversation.num_unread = conversation.num_unread.max(1);
                if let Some(label) = conversation.labels.iter_mut().find(|l| l.id == place) {
                    label.context_num_unread = Some(label.context_num_unread.unwrap_or(0).max(1));
                }
            }
            Change::Move(folder) => {
                let folders = demo_labels()
                    .into_iter()
                    .filter(|label| label.label_type == label_type::MESSAGE_FOLDER)
                    .map(|label| label.id);
                let left: Vec<String> = [
                    label_ids::INBOX,
                    label_ids::ARCHIVE,
                    label_ids::SPAM,
                    label_ids::TRASH,
                ]
                .map(String::from)
                .into_iter()
                .chain(folders)
                .collect();
                conversation.labels.retain(|l| !left.contains(&l.id));
                let moved = whole(folder, conversation);
                conversation.labels.push(moved);
                // Its messages go with it, keeping theirs likewise.
                let mut messages = self.messages.lock().expect("demo mailbox lock poisoned");
                for message in messages.iter_mut().filter(|m| m.meta.conversation_id == id) {
                    message.meta.label_ids.retain(|l| !left.contains(l));
                    message.meta.label_ids.push(folder.clone());
                }
            }
            Change::Label(label) => {
                if !conversation.labels.iter().any(|l| &l.id == label) {
                    let labelled = whole(label, conversation);
                    conversation.labels.push(labelled);
                }
            }
            Change::Unlabel(label) => conversation.labels.retain(|l| &l.id != label),
            Change::Delete => conversations.retain(|c| c.id != id),
        }
    }

    /// Deletes forever every conversation in the place `place`, as emptying
    /// it does.
    pub(crate) fn empty(&self, place: &str) {
        let (_, listed) = self.conversations(place, 0, u32::MAX);
        for conversation in listed {
            self.change(&conversation.id, place, &Change::Delete);
        }
    }
}

/// What the demo's attachments hold: a stand-in, as its downloads save it.
fn simulated_attachment(name: &str) -> Vec<u8> {
    format!("%PDF-1.4\n% Demo simulated attachment content for {name}\n").into_bytes()
}

/// `original` quoted under an answer's text as core quotes it in plain
/// text, from what the reader shows of its body.
fn quoted(original: &DemoMessage) -> String {
    let sender = &original.meta.sender;
    let who = if sender.name.is_empty() {
        sender.address.clone()
    } else {
        format!("{} <{}>", sender.name, sender.address)
    };
    let lines: String = render::thread_body(&original.to_full())
        .lines()
        .map(|line| format!("> {line}\n"))
        .collect();
    format!("------- Original Message -------\nOn {who} wrote:\n{lines}")
}

/// The first of `messages` that `reference` names, as a message reference
/// names one in the demo: its ID, part of it, or words of its subject.
fn message_named<'a>(messages: &'a [DemoMessage], reference: &str) -> Result<&'a DemoMessage> {
    messages
        .iter()
        .find(|m| {
            m.meta.id == reference
                || m.meta.id.contains(reference)
                || m.meta
                    .subject
                    .to_lowercase()
                    .contains(&reference.to_lowercase())
        })
        .ok_or_else(|| Error::Other(format!("demo message not found for ref '{reference}'")))
}

/// What tells `message` from the demo's other messages: its ID without the
/// `msg-demo-` all of them start with.
fn demo_number(message: &DemoMessage) -> &str {
    message.meta.id.trim_start_matches("msg-demo-")
}

/// The IDs of the messages `references` name, in their order, each found as
/// [`message_named`] finds it: what an action on them reports, as the real
/// commands report the IDs they resolve.
fn ids_named(messages: &[DemoMessage], references: &[String]) -> Result<Vec<String>> {
    references
        .iter()
        .map(|reference| Ok(message_named(messages, reference)?.meta.id.clone()))
        .collect()
}

/// Up to `limit` of the demo's messages whose subject or body has `query`,
/// whatever its case: the demo's local index, for `search` and the terminal
/// interface's `:local` alike.
pub(crate) fn search_local(query: &str, limit: u32) -> Vec<MessageMetadata> {
    let query = query.to_lowercase();
    let mut found: Vec<MessageMetadata> = fixtures()
        .messages
        .into_iter()
        .filter(|m| {
            m.meta.subject.to_lowercase().contains(&query) || m.body.to_lowercase().contains(&query)
        })
        .map(|m| m.meta)
        .collect();
    found.truncate(limit as usize);
    found
}

/// The place a demo search looks in, as the real search resolves it: none
/// for All mail, which holds every message.
fn search_label(folder: Option<&str>) -> Result<Option<String>> {
    let label = folder
        .map(|folder| folder_id_among(&demo_labels(), folder))
        .transpose()?;
    Ok(label.filter(|label| label != label_ids::ALL_MAIL))
}

/// Whether `conversation`'s subject has `words`, whatever their case: how
/// the demo finds conversations, in `conversations search` and the terminal
/// interface alike.
fn subject_has(conversation: &Conversation, words: &str) -> bool {
    conversation
        .subject
        .to_lowercase()
        .contains(&words.to_lowercase())
}

/// Whether a demo search for `opts` finds `conversation`, in
/// `conversations search` and the terminal interface alike: its subject
/// has the words and the subject asked for, one of its senders and one of
/// its recipients match those asked for, it is unread when asked, its last
/// message falls within `window`, and it is in the place `label`, if any.
fn finds(
    conversation: &Conversation,
    opts: &SearchOpts,
    window: (Option<i64>, Option<i64>),
    label: Option<&str>,
) -> bool {
    [&opts.keyword, &opts.subject]
        .into_iter()
        .flatten()
        .all(|words| subject_has(conversation, words))
        && opts
            .from
            .as_ref()
            .is_none_or(|from| someone_matches(&conversation.senders, from))
        && opts
            .to
            .as_ref()
            .is_none_or(|to| someone_matches(&conversation.recipients, to))
        && (!opts.unread || conversation.has_unread())
        && within(window, conversation.time)
        && label.is_none_or(|label| conversation.labels.iter().any(|on| on.id == label))
}

/// How many of `conversations` each place holds and how many of those are
/// unread there, as Proton counts them; All mail holds every one.
fn conversation_counts(conversations: &[Conversation]) -> Vec<LabelCount> {
    let unread = conversations.iter().filter(|c| c.has_unread()).count();
    tally(
        conversations.iter().flat_map(|conversation| {
            conversation
                .labels
                .iter()
                .map(|label| (label.id.as_str(), label.context_num_unread.unwrap_or(0) > 0))
        }),
        (conversations.len(), unread),
    )
}

/// How many of `messages` each place holds and how many of those are
/// unread, as Proton counts messages; All mail holds every one.
fn message_counts(messages: &[DemoMessage]) -> Vec<LabelCount> {
    let unread = messages.iter().filter(|m| m.meta.is_unread()).count();
    tally(
        messages.iter().flat_map(|message| {
            let unread = message.meta.is_unread();
            message
                .meta
                .label_ids
                .iter()
                .map(move |id| (id.as_str(), unread))
        }),
        (messages.len(), unread),
    )
}

/// The places each `(place, unread)` pair is in, in the order they first
/// come, each with how many pairs name it and how many of those are
/// unread, then All mail with `all`, its total and unread.
fn tally<'a>(
    placed: impl Iterator<Item = (&'a str, bool)>,
    all: (usize, usize),
) -> Vec<LabelCount> {
    let mut counts: Vec<LabelCount> = Vec::new();
    for (place, unread) in placed {
        let at = counts
            .iter()
            .position(|count| count.label_id == place)
            .unwrap_or_else(|| {
                counts.push(LabelCount {
                    label_id: place.to_owned(),
                    total: 0,
                    unread: 0,
                });
                counts.len() - 1
            });
        counts[at].total += 1;
        counts[at].unread += i64::from(unread);
    }
    counts.push(LabelCount {
        label_id: label_ids::ALL_MAIL.to_string(),
        total: all.0 as i64,
        unread: all.1 as i64,
    });
    counts
}

/// Dispatcher for CLI commands in offline demo mode.
pub async fn dispatch(ctx: &Ctx, cmd: Command) -> Result<()> {
    terminal::eprintln!("{NOTICE}");
    render::mark_demo();
    let mailbox = fixtures();

    match cmd {
        Command::Login(_) => commands::login(ctx),
        Command::Logout => commands::logout(ctx),
        Command::Whoami => commands::whoami(ctx),
        Command::Tui(args) => crate::tui::run(ctx, &args).await,
        Command::Messages { cmd } => commands::messages(ctx, cmd, mailbox.messages),
        Command::Conversations { cmd } => {
            commands::conversations(ctx, cmd, mailbox.messages, mailbox.conversations)
        }
        Command::Attachments { cmd } => commands::attachments(ctx, cmd, &mailbox.messages),
        Command::Drafts { cmd } => commands::drafts(ctx, cmd, mailbox.messages),
        Command::Filters { cmd } => commands::filters(ctx, cmd),
        Command::Contacts { cmd } => commands::contacts(ctx, cmd),
        Command::Addresses { cmd } => commands::addresses(ctx, cmd),
        Command::Settings { cmd } => commands::settings(ctx, cmd),
        Command::Counts { conversations } => commands::counts(ctx, conversations),
        Command::Labels { cmd } => commands::labels(ctx, cmd),
        Command::Export { folder, out, max } => {
            commands::export(ctx, folder, out, max, &mailbox.messages)
        }
        Command::Search { query, limit } => commands::search(ctx, query, limit),
        Command::Sync { backfill, .. } => commands::sync(ctx, backfill),
        Command::Index { .. } => commands::index(ctx, mailbox.messages.len()),
        Command::Watch { interval, folder } => commands::watch(ctx, interval, folder),
    }
}

#[cfg(test)]
mod tests;
