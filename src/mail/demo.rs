//! Fictional in-memory mailbox with reserved `example.*` addresses.
//! It never touches Proton, the network, the keychain or disk.

use std::sync::{Arc, Mutex, MutexGuard};

use ruston_core::model::enums::message_flag;
use ruston_core::{AddressInfo, Quote};

use super::outgoing::{Outgoing, OutgoingKind, SendError};
use super::{
    ConversationDetail, ConversationPage, ConversationSummary, Folder, IncomingMail, ListFilter,
    MailAddress, MailFolder, MailMessage, MailboxCounts, MailboxError, MessageBody, SummaryKind,
};

mod fixtures;

use fixtures::fixtures;

/// Small enough that the demo Inbox needs several pages.
pub const PAGE_SIZE: u32 = 10;

/// How many messages the fictional mailbox accepts in one run.
const SEND_LIMIT: usize = 5;
const MINUTE: i64 = 60;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;

/// Time between consecutive messages in a generated thread.
const THREAD_GAP: i64 = 3 * HOUR + 17 * MINUTE;

const DEMO_USER_NAME: &str = "Demo User";
const DEMO_USER_ADDRESS: &str = "demo@example.com";

/// The fictional account's addresses, each with the ID Proton would give
/// it, in the order Proton lists them: new mail goes from the first unless
/// another is chosen. Like many Proton accounts, it has its name under a
/// second domain.
const DEMO_ADDRESSES: [(&str, &str); 2] = [
    ("demo-address-1", DEMO_USER_ADDRESS),
    ("demo-address-2", "demo@example.org"),
];

/// What joins a conversation's correspondents in [`Fixture::correspondents`].
const CORRESPONDENT_SEPARATOR: &str = ", ";

struct Fixture {
    folder: MailFolder,
    /// Whether the demo user sent the original conversation.
    outgoing: bool,
    /// Senders, or recipients in Sent and Drafts, joined by
    /// [`CORRESPONDENT_SEPARATOR`]. Empty means unknown.
    correspondents: String,
    subject: String,
    /// Seconds before now.
    age: i64,
    unread: bool,
    starred: bool,
    /// Message bodies, oldest first. The length is the conversation count.
    bodies: Vec<String>,
    /// Whether the bodies are HTML rather than plain text.
    html: bool,
    /// Replaces the generated recipients of every message.
    to: Vec<String>,
    /// Written in this run rather than shipped with the demo.
    composed: bool,
    /// Which of [`DEMO_ADDRESSES`] the demo user writes from and is
    /// written to at.
    address: usize,
}

/// Process-local mutable state for the offline mailbox.
#[derive(Clone)]
pub struct DemoMailbox {
    fixtures: Arc<Mutex<Vec<Fixture>>>,
}

impl Default for DemoMailbox {
    fn default() -> Self {
        Self::new()
    }
}

impl DemoMailbox {
    pub fn new() -> Self {
        Self {
            fixtures: Arc::new(Mutex::new(fixtures())),
        }
    }

    /// The fictional mail, held for one operation.
    fn lock(&self) -> MutexGuard<'_, Vec<Fixture>> {
        self.fixtures.lock().expect("demo mailbox lock poisoned")
    }

    /// The newest of the Inbox, as a check for new mail reads it.
    pub fn recent_inbox(&self, limit: u32, now: i64) -> Vec<IncomingMail> {
        list_from(&self.lock(), &Folder::INBOX, |_, _| true, 0, limit, now)
            .conversations
            .into_iter()
            .map(|row| IncomingMail {
                id: row.id,
                time: row.time.unwrap_or_default(),
                unread: row.unread,
                sender: row.correspondents.unwrap_or_default(),
                subject: row.subject.unwrap_or_default(),
            })
            .collect()
    }

    /// Every conversation of `folder`, a page at a time, as tests list them.
    #[cfg(test)]
    pub fn list_conversations(
        &self,
        folder: &Folder,
        page: u32,
        page_size: u32,
        now: i64,
    ) -> Result<ConversationPage, MailboxError> {
        self.list_filtered(folder, ListFilter::All, page, page_size, now)
    }

    /// One page of the conversations of `folder` that `filter` shows.
    pub fn list_filtered(
        &self,
        folder: &Folder,
        filter: ListFilter,
        page: u32,
        page_size: u32,
        now: i64,
    ) -> Result<ConversationPage, MailboxError> {
        let fixtures = self.lock();
        let shows = |_: usize, fixture: &Fixture| filter == ListFilter::All || fixture.unread;
        Ok(list_from(&fixtures, folder, shows, page, page_size, now))
    }

    /// The fictional account's addresses, in the order Proton lists them.
    pub fn addresses(&self) -> Vec<AddressInfo> {
        DEMO_ADDRESSES
            .iter()
            .map(|(id, email)| AddressInfo {
                id: (*id).to_owned(),
                email: (*email).to_owned(),
            })
            .collect()
    }

    /// Searches every folder, the way the server does for a real account.
    #[cfg(test)]
    pub fn search(
        &self,
        query: &str,
        limit: u32,
        now: i64,
    ) -> Result<ConversationPage, MailboxError> {
        self.search_page(query, 0, limit, now)
    }

    pub fn search_page(
        &self,
        query: &str,
        page: u32,
        page_size: u32,
        now: i64,
    ) -> Result<ConversationPage, MailboxError> {
        let fixtures = self.lock();
        Ok(search_from(&fixtures, query, page, page_size, now))
    }

    pub fn counts(&self) -> MailboxCounts {
        let fixtures = self.lock();
        counts_from(&fixtures)
    }

    pub fn conversation_detail(
        &self,
        id: &str,
        now: i64,
    ) -> Result<ConversationDetail, MailboxError> {
        let fixtures = self.lock();
        detail_from(&fixtures, id, now).ok_or(MailboxError::Unavailable)
    }

    /// The first `page_size` conversations of `folder` that `filter` shows,
    /// with the counts, after a change. A conversation `listed` already stays
    /// listed, as a Proton account's would until the list is loaded again.
    pub fn snapshot(
        &self,
        folder: &Folder,
        filter: ListFilter,
        listed: impl Fn(&str) -> bool,
        page_size: u32,
        now: i64,
    ) -> (ConversationPage, MailboxCounts) {
        let fixtures = self.lock();
        let shows = |index: usize, fixture: &Fixture| {
            filter == ListFilter::All || fixture.unread || listed(&conversation_id(index))
        };
        (
            list_from(&fixtures, folder, shows, 0, page_size, now),
            counts_from(&fixtures),
        )
    }

    /// Stores a sent message locally, subject to the demo cap.
    pub fn send(&self, outgoing: &Outgoing, now: i64) -> Result<(), SendError> {
        let mut fixtures = self.lock();
        if fixtures.iter().filter(|fixture| fixture.composed).count() >= SEND_LIMIT {
            return Err(SendError::DemoLimitReached);
        }

        // Mirror core's subject, quote and reply-recipient behavior: a reply
        // goes to the recipients typed, or to whom it asks when none were.
        let answered = answered_message(&fixtures, &outgoing.kind, now);
        let named_nobody =
            outgoing.to.is_empty() && outgoing.cc.is_empty() && outgoing.bcc.is_empty();
        let (prefix, back_to) = match (&outgoing.kind, &answered) {
            (OutgoingKind::Reply { .. }, Some(parent)) if named_nobody => {
                ("Re: ", parent.reply_to.clone())
            }
            (OutgoingKind::Reply { .. }, _) => ("Re: ", Vec::new()),
            (OutgoingKind::Forward { .. }, _) => ("Fw: ", Vec::new()),
            (OutgoingKind::New, _) => ("", Vec::new()),
        };

        let mut everyone: Vec<String> = Vec::new();
        for address in back_to
            .into_iter()
            .chain(outgoing.to.iter().cloned())
            .chain(outgoing.cc.iter().cloned())
            .chain(outgoing.bcc.iter().cloned())
        {
            if !everyone
                .iter()
                .any(|known| known.eq_ignore_ascii_case(&address))
            {
                everyone.push(address);
            }
        }
        fixtures.push(Fixture {
            folder: MailFolder::Sent,
            outgoing: true,
            correspondents: everyone.join(CORRESPONDENT_SEPARATOR),
            subject: answered.as_ref().map_or_else(
                || outgoing.subject.clone(),
                |parent| with_prefix(prefix, &parent.subject),
            ),
            age: 0,
            unread: false,
            starred: false,
            // What would have gone on the wire, so choosing HTML shows in the
            // demo what choosing HTML does.
            bodies: vec![match (&answered, outgoing.kind.quote()) {
                (Some(parent), Quote::Original) => {
                    format!("{}\n\n{}", outgoing.wire_body(), parent.quoted)
                }
                _ => outgoing.wire_body(),
            }],
            html: outgoing.is_html(),
            to: everyone,
            composed: true,
            // Core sends from the first address unless another is chosen.
            address: DEMO_ADDRESSES
                .iter()
                .position(|(_, email)| outgoing.from.as_deref() == Some(*email))
                .unwrap_or_default(),
        });

        Ok(())
    }

    /// [`Self::send`] at the fictional mailbox's own clock.
    #[cfg(test)]
    pub fn send_now(&self, outgoing: &Outgoing) -> Result<(), SendError> {
        self.send(outgoing, tests::NOW)
    }

    pub fn set_unread(&self, id: &str, unread: bool) -> bool {
        self.update(id, |fixture| fixture.unread = unread)
    }

    pub fn set_starred(&self, id: &str, starred: bool) -> bool {
        self.update(id, |fixture| fixture.starred = starred)
    }

    /// Moves a row between Proton's own folders. The fictional mailbox has no
    /// folders of its own, so a move into one changes nothing.
    pub fn move_to(&self, id: &str, folder: &Folder) -> bool {
        let Some(folder) = folder.system() else {
            return false;
        };

        self.update(id, |fixture| fixture.folder = folder)
    }

    fn update(&self, id: &str, update: impl FnOnce(&mut Fixture)) -> bool {
        let Some(index) = fixture_index(id) else {
            return false;
        };
        let mut fixtures = self.lock();
        let Some(fixture) = fixtures.get_mut(index) else {
            return false;
        };

        update(fixture);
        true
    }
}

impl Fixture {
    fn message_count(&self) -> usize {
        self.bodies.len()
    }
}

/// Plain content used by previews and search, never HTML tags.
fn body_text(fixture: &Fixture, body: &str) -> String {
    if !fixture.html {
        return body.to_owned();
    }
    ruston_core::html::rich::parse(body).plain_text()
}

/// Matching rows across the whole demo mailbox, newest first.
fn search_from(
    fixtures: &[Fixture],
    query: &str,
    page: u32,
    page_size: u32,
    now: i64,
) -> ConversationPage {
    let query = query.trim().to_lowercase();
    let matching = fixtures
        .iter()
        .enumerate()
        .filter(|(_, fixture)| matches_query(fixture, &query))
        .collect();
    page_of(matching, page, page_size, now)
}

/// The same places the mailbox search looks: who wrote it, what it is about,
/// and what it says.
fn matches_query(fixture: &Fixture, query: &str) -> bool {
    if query.is_empty() {
        return false;
    }

    [
        fixture.subject.to_owned(),
        fixture.correspondents.to_owned(),
    ]
    .into_iter()
    .chain(fixture.bodies.iter().map(|body| body_text(fixture, body)))
    .any(|value| value.to_lowercase().contains(query))
}

/// The fictional mailbox has only Proton's own folders, so a folder the
/// account made never holds any of it.
fn in_folder(fixture: &Fixture, folder: &Folder) -> bool {
    let Some(folder) = folder.system() else {
        return false;
    };

    fixture.folder == folder || (folder == MailFolder::Starred && fixture.starred)
}

/// Page `page` of the conversations of `folder` that `shows` lets through,
/// given each one's place in the mailbox.
fn list_from(
    fixtures: &[Fixture],
    folder: &Folder,
    shows: impl Fn(usize, &Fixture) -> bool,
    page: u32,
    page_size: u32,
    now: i64,
) -> ConversationPage {
    let matching = fixtures
        .iter()
        .enumerate()
        .filter(|(index, fixture)| in_folder(fixture, folder) && shows(*index, fixture))
        .collect();
    page_of(matching, page, page_size, now)
}

/// Page `page` of the `matching` conversations, by their places in the
/// mailbox, newest first.
fn page_of(
    mut matching: Vec<(usize, &Fixture)>,
    page: u32,
    page_size: u32,
    now: i64,
) -> ConversationPage {
    matching.sort_by_key(|(_, fixture)| fixture.age);
    let total = matching.len() as u32;
    let conversations = matching
        .into_iter()
        .skip((page as usize).saturating_mul(page_size as usize))
        .take(page_size as usize)
        .map(|(index, fixture)| summary(index, fixture, now))
        .collect();

    ConversationPage {
        conversations,
        total,
        inspect_candidates: Vec::new(),
    }
}

fn counts_from(fixtures: &[Fixture]) -> MailboxCounts {
    MailFolder::ALL
        .into_iter()
        .map(Folder::System)
        .map(|folder| {
            let unread = fixtures
                .iter()
                .filter(|fixture| fixture.unread && in_folder(fixture, &folder))
                .count();
            (folder, unread as u32)
        })
        .collect()
}

fn detail_from(fixtures: &[Fixture], id: &str, now: i64) -> Option<ConversationDetail> {
    detail_at(fixtures, fixture_index(id)?, now)
}

/// The conversation at `index` in the mailbox, with its messages.
fn detail_at(fixtures: &[Fixture], index: usize, now: i64) -> Option<ConversationDetail> {
    let fixture = fixtures.get(index)?;
    let participants: Vec<MailAddress> = correspondent_addresses(fixture).collect();
    let count = fixture.message_count();
    let messages = (0..count)
        .map(|position| message(fixture, &participants, index, position, count, now))
        .collect();

    Some(ConversationDetail {
        id: conversation_id(index),
        subject: non_empty(&fixture.subject),
        messages,
        // The demo account owns no labels, so nothing carries one.
        labels: Vec::new(),
    })
}

/// The message an answer is answering, as the fictional mailbox sees it.
struct Answered {
    /// Where core would send a reply.
    reply_to: Vec<String>,
    subject: String,
    quoted: String,
}

/// Finds the message being answered and works out how it is quoted.
fn answered_message(fixtures: &[Fixture], kind: &OutgoingKind, now: i64) -> Option<Answered> {
    let message_id = match kind {
        OutgoingKind::New => return None,
        OutgoingKind::Reply { message_id, .. } | OutgoingKind::Forward { message_id, .. } => {
            message_id
        }
    };
    let (index, position) = message_place(message_id)?;
    let detail = detail_at(fixtures, index, now)?;
    let message = detail.messages.get(position)?;

    Some(Answered {
        subject: detail.subject.clone().unwrap_or_default(),
        quoted: message
            .body
            .plain_text()
            .lines()
            .map(|line| format!("> {line}"))
            .collect::<Vec<_>>()
            .join("\n"),
        reply_to: message.reply_to.clone(),
    })
}

/// The ID of the conversation at `index` in the mailbox.
fn conversation_id(index: usize) -> String {
    format!("demo-{index}")
}

/// The place in the mailbox that [`conversation_id`] names.
fn fixture_index(id: &str) -> Option<usize> {
    id.strip_prefix("demo-")?.parse().ok()
}

/// The ID of the message at `position` in the conversation at `index`.
fn message_id(index: usize, position: usize) -> String {
    format!("demo-{index}-{position}")
}

/// The conversation and the place in it that [`message_id`] names.
fn message_place(id: &str) -> Option<(usize, usize)> {
    let rest = id.strip_prefix("demo-")?;
    let (index, position) = rest.split_once('-')?;

    Some((index.parse().ok()?, position.parse().ok()?))
}

/// Adds the prefix unless the subject already carries it.
fn with_prefix(prefix: &str, subject: &str) -> String {
    if prefix.is_empty() || subject.starts_with(prefix) {
        return subject.to_owned();
    }

    format!("{prefix}{subject}")
}

/// Fictional protection: the demo treats Proton's own domains as encrypted
/// and every other address as unencrypted, so both states can be seen.
pub fn protection(address: &str) -> super::Protection {
    let domain = address.rsplit('@').next().unwrap_or_default();
    let proton = ["proton.me", "protonmail.com", "pm.me"]
        .iter()
        .any(|known| domain.eq_ignore_ascii_case(known));
    if proton {
        super::Protection::EndToEnd
    } else {
        super::Protection::Unencrypted
    }
}

#[cfg(not(test))]
pub fn now() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        })
}

/// Tests read the fictional mailbox at one fixed instant, so every listing of
/// a row, and the reader stamp taken from it, agrees however long they run.
#[cfg(test)]
pub fn now() -> i64 {
    tests::NOW
}

fn summary(index: usize, fixture: &Fixture, now: i64) -> ConversationSummary {
    ConversationSummary {
        id: conversation_id(index),
        kind: SummaryKind::Conversation,
        subject: non_empty(&fixture.subject),
        correspondents: non_empty(&fixture.correspondents),
        participants: fixture_participants(fixture),
        preview: fixture
            .bodies
            .last()
            .and_then(|body| non_empty(&preview(&body_text(fixture, body)))),
        time: Some(now - fixture.age),
        unread: fixture.unread,
        starred: fixture.starred,
        message_count: u32::try_from(fixture.message_count()).unwrap_or(u32::MAX),
        // The fictional mailbox carries no files.
        has_attachments: false,
    }
}

/// The addresses of `fixture`'s correspondents.
fn correspondent_addresses(fixture: &Fixture) -> impl Iterator<Item = MailAddress> + '_ {
    fixture
        .correspondents
        .split(CORRESPONDENT_SEPARATOR)
        .filter(|participant| !participant.is_empty())
        .map(participant_address)
}

fn fixture_participants(fixture: &Fixture) -> Vec<MailAddress> {
    std::iter::once(demo_user(fixture))
        .chain(correspondent_addresses(fixture))
        .chain(
            fixture
                .to
                .iter()
                .map(|address| participant_address(address)),
        )
        .collect()
}

fn preview(body: &str) -> String {
    body.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(200)
        .collect()
}

fn message(
    fixture: &Fixture,
    participants: &[MailAddress],
    index: usize,
    position: usize,
    count: usize,
    now: i64,
) -> MailMessage {
    let newer = count - 1 - position;
    // Messages alternate between the demo user and the other participants;
    // the newest one is the demo user's only in Sent and Drafts.
    let from_demo_user = newer.is_multiple_of(2) == fixture.outgoing;

    let (sender, mut recipients) = if from_demo_user {
        (demo_user(fixture), participants.to_vec())
    } else {
        let sender = if participants.is_empty() {
            MailAddress::default()
        } else {
            participants[(position / 2) % participants.len()].clone()
        };
        let recipients = std::iter::once(demo_user(fixture))
            .chain(
                participants
                    .iter()
                    .filter(|participant| **participant != sender)
                    .cloned(),
            )
            .collect();
        (sender, recipients)
    };
    if !fixture.to.is_empty() {
        recipients = fixture
            .to
            .iter()
            .map(|address| participant_address(address))
            .collect();
    }

    // The fictional mail sets no Reply-To, and answers come from core's rule
    // as the Proton adapter's do, told by the flags Proton would set: the
    // demo user sent it, received it, or only wrote it, as a draft.
    let flags = if !from_demo_user {
        message_flag::RECEIVED
    } else if fixture.folder == MailFolder::Drafts && newer == 0 {
        0
    } else {
        message_flag::SENT
    };
    let core_recipient = |person: &MailAddress| ruston_core::Recipient {
        address: person.address.clone(),
        ..ruston_core::Recipient::default()
    };
    let meta = ruston_core::MessageMetadata {
        flags,
        sender: core_recipient(&sender),
        to_list: recipients.iter().map(core_recipient).collect(),
        ..ruston_core::MessageMetadata::default()
    };
    let own: Vec<String> = DEMO_ADDRESSES
        .iter()
        .map(|(_, email)| (*email).to_owned())
        .collect();
    let reply = ruston_core::reply_recipients(&meta, &[], &own);
    MailMessage {
        id: message_id(index, position),
        address_id: DEMO_ADDRESSES[fixture.address].0.to_owned(),
        verdict: super::Verdict::Unverified,
        reply_to: reply.to,
        reply_all_cc: reply.cc,
        reply_redirected: false,
        sender,
        // The fictional mail goes to its people directly, never as a copy.
        to: recipients,
        cc: Vec::new(),
        bcc: Vec::new(),
        time: Some(now - fixture.age - newer as i64 * THREAD_GAP),
        // An unread conversation's newest message is the one not read yet.
        unread: fixture.unread && newer == 0 && !from_demo_user,
        body: if fixture.html {
            MessageBody::Rich(ruston_core::html::rich::parse(&fixture.bodies[position]))
        } else {
            MessageBody::PlainText(fixture.bodies[position].to_owned())
        },
        attachments: Vec::new(),
    }
}

/// The demo user, at the address `fixture` involves.
fn demo_user(fixture: &Fixture) -> MailAddress {
    MailAddress {
        name: Some(DEMO_USER_NAME.to_owned()),
        address: DEMO_ADDRESSES[fixture.address].1.to_owned(),
    }
}

/// A bare address has no display name; a name gets an invented
/// `first.last@example.com` address.
fn participant_address(participant: &str) -> MailAddress {
    if participant.contains('@') {
        return MailAddress {
            name: None,
            address: participant.to_owned(),
        };
    }

    let local_part = participant
        .split_whitespace()
        .map(|word| {
            word.chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
                .collect::<String>()
                .to_ascii_lowercase()
        })
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(".");

    MailAddress {
        name: Some(participant.to_owned()),
        address: format!("{local_part}@example.com"),
    }
}

fn non_empty(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_owned())
}

#[cfg(test)]
mod tests;
