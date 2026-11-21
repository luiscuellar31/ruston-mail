use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use ruston_core::Quote;
use ruston_core::mail::send::PROTECTION_LOOKUP_LIMIT;

use crate::mail::{
    BodyFormat, Domains, InternationalDomain, MailAttachment, MailMessage, MailboxError,
    MessageBody, Outgoing, OutgoingKind, Protection, ProtectionCheck, Recipients, SendError,
    recipients,
};

use super::{App, Effects, Message, ReaderState, UiEffect};

pub type ComposeId = u64;

/// One of the fields being typed into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ComposeField {
    To,
    Cc,
    Bcc,
    Subject,
    Body,
}

/// Why a message cannot leave yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotReady {
    /// Nobody to send it to.
    NoRecipients,
    /// Pieces of a recipient field that do not look like an address. Sending
    /// anyway would reach fewer people than the sender believes.
    BadAddresses(Vec<String>),
    /// Addresses refused only for a domain written in another script, which
    /// Settings do not accept.
    InternationalDomains(Vec<String>),
}

impl NotReady {
    pub fn message(&self) -> String {
        match self {
            Self::NoRecipients => "Add someone to send this to.".to_owned(),
            Self::BadAddresses(pieces) => {
                format!("This does not look like an address: {}", pieces.join(", "))
            }
            Self::InternationalDomains(addresses) => format!(
                "Domains in another script are not accepted: {}. Type the domain in its xn-- form, or turn on Accept international domain names in the Advanced section of Settings.",
                addresses.join(", ")
            ),
        }
    }
}

/// What is known about how one recipient's copy will be protected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Checked {
    /// Asked, no answer yet.
    Checking,
    Known(Protection),
    /// The lookup failed; nothing is claimed either way.
    Unknown,
}

/// What the composer says about protection before a new message is sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtectionNotice {
    Checking,
    /// Every recipient has a key the message will be encrypted to.
    Encrypted,
    /// Some recipients will not get an end-to-end encrypted copy, or could
    /// not be checked. Both are named, so an unchecked address never reads
    /// as encrypted.
    Warning {
        unencrypted: Vec<String>,
        unknown: Vec<String>,
        /// How many recipients past [`PROTECTION_LOOKUP_LIMIT`] were not
        /// looked up, counted rather than named.
        unchecked: usize,
    },
}

impl ProtectionNotice {
    pub fn is_warning(&self) -> bool {
        matches!(self, Self::Warning { .. })
    }

    pub fn message(&self) -> String {
        match self {
            Self::Checking => "Checking how this message will be protected…".to_owned(),
            Self::Encrypted => {
                "Proton will end-to-end encrypt this message for every recipient.".to_owned()
            }
            Self::Warning {
                unencrypted,
                unknown,
                unchecked,
            } => {
                let mut lines = Vec::new();
                if !unencrypted.is_empty() {
                    lines.push(format!(
                        "Proton will not end-to-end encrypt this message for: {}",
                        unencrypted.join(", ")
                    ));
                }
                if !unknown.is_empty() {
                    lines.push(format!(
                        "Could not check how this message will be protected for: {}",
                        unknown.join(", ")
                    ));
                }
                if *unchecked > 0 {
                    lines.push(unchecked_line(*unchecked));
                }
                lines.join("\n")
            }
        }
    }
}

/// The warning's line for the `count` recipients past
/// [`PROTECTION_LOOKUP_LIMIT`], which were not looked up.
fn unchecked_line(count: usize) -> String {
    let recipients = if count == 1 {
        "recipient"
    } else {
        "recipients"
    };
    format!("Not checked: {count} more {recipients}, past the first {PROTECTION_LOOKUP_LIMIT}.")
}

/// How far along a message is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Sending {
    /// Still being written.
    #[default]
    Writing,
    /// Handed over, waiting to hear back.
    InFlight,
    /// The send failed or its outcome could not be confirmed.
    Failed(SendError),
}

/// How a message is answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnswerKind {
    /// A reply to whoever the message asks replies to go to.
    Reply,
    /// A reply that also copies in everyone else the message went to.
    ReplyAll,
    /// The message passed on to recipients the user types.
    Forward,
}

impl AnswerKind {
    /// What core sends to answer the message `message_id` this way.
    fn outgoing(self, message_id: &str) -> OutgoingKind {
        let message_id = message_id.to_owned();
        match self {
            AnswerKind::Reply => OutgoingKind::Reply {
                message_id,
                everyone: false,
                quote: Quote::Original,
            },
            AnswerKind::ReplyAll => OutgoingKind::Reply {
                message_id,
                everyone: true,
                quote: Quote::Original,
            },
            AnswerKind::Forward => OutgoingKind::Forward {
                message_id,
                quote: Quote::Original,
                leave_out: Vec::new(),
            },
        }
    }
}

/// The message being answered. Core writes the subject; a reply's recipient
/// fields open with whom core would answer, and the writer may change them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answering {
    pub sender: String,
    pub subject: String,
    pub kind: AnswerKind,
    /// Whom a reply opens addressed to; empty for a forward, whose
    /// recipients are typed.
    pub to: Vec<String>,
    /// Whom Reply all opens copying in.
    pub cc: Vec<String>,
    /// The sender's address when the message's Reply-To sends the reply
    /// somewhere else, so the composer can say so.
    pub redirected_from: Option<String>,
    /// Whether core quotes the original under the text, which it does when
    /// the original opens, as it did in the reader.
    pub quotes_original: bool,
    /// The files a forward carries from the original, as the reader lists
    /// them; a reply carries none.
    pub forwarded: Vec<MailAttachment>,
}

/// A message being written.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Compose {
    id: ComposeId,
    kind: OutgoingKind,
    answering: Option<Answering>,
    to: String,
    cc: String,
    bcc: String,
    /// What To and Cc opened with: for a reply, whom core would answer.
    /// Leaving them as they were is not writing.
    opened_to: String,
    opened_cc: String,
    subject: String,
    body: String,
    attachments: Vec<PathBuf>,
    /// The size of each attached file once read on the blocking pool; `None`
    /// for a file that could not be read.
    sizes: HashMap<PathBuf, Option<u64>>,
    /// Which domains the recipient fields take, from Settings.
    domains: Domains,
    /// The recipient fields as last read under `domains`.
    typed: Typed,
    /// Protection looked up for typed recipients, by address as sent. An
    /// address leaves this map as soon as it leaves every recipient field.
    protection: HashMap<String, Checked>,
    /// Whether the copy fields are on show. They start hidden: most mail goes
    /// to one person.
    copies_shown: bool,
    state: Sending,
    confirming_discard: bool,
    /// Another message was asked for while this one is open, so the
    /// composer says why none opened, until this one is edited.
    asked_again: bool,
    /// Folded into a strip over the reader, which shows the conversation
    /// chosen while writing; only a draft in the reading pane folds.
    folded: bool,
}

/// What the recipient fields came to. They are read again only when a field
/// or the rule for domains changes, so drawing a frame reads and copies
/// nothing. Empty fields read as the default.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Typed {
    to: Recipients,
    cc: Recipients,
    bcc: Recipients,
    /// Every accepted address once, in field order, as first written.
    addresses: Vec<String>,
    /// Whether every refused piece would be taken if Settings accepted domains
    /// in other scripts, so the refusal can say how to allow them.
    script_only: bool,
}

impl Typed {
    fn read(to: &str, cc: &str, bcc: &str, domains: Domains) -> Self {
        let (to, cc, bcc) = (
            recipients(to, domains),
            recipients(cc, domains),
            recipients(bcc, domains),
        );
        let mut seen = HashSet::new();
        let addresses = [&to, &cc, &bcc]
            .into_iter()
            .flat_map(|field| &field.accepted)
            .filter(|address| seen.insert(address.to_ascii_lowercase()))
            .cloned()
            .collect();
        let mut refused = to
            .rejected
            .iter()
            .chain(&cc.rejected)
            .chain(&bcc.rejected)
            .peekable();
        let script_only = domains == Domains::Ascii
            && refused.peek().is_some()
            && refused.all(|piece| {
                recipients(piece, Domains::International)
                    .rejected
                    .is_empty()
            });
        Self {
            to,
            cc,
            bcc,
            addresses,
            script_only,
        }
    }

    /// The pieces that do not look like an address, To before Cc before Bcc.
    fn rejected(&self) -> Vec<String> {
        self.to
            .rejected
            .iter()
            .chain(&self.cc.rejected)
            .chain(&self.bcc.rejected)
            .cloned()
            .collect()
    }
}

impl Compose {
    /// Which draft this is; the editors keep their memory per draft.
    pub fn id(&self) -> ComposeId {
        self.id
    }

    pub fn field(&self, field: ComposeField) -> &str {
        match field {
            ComposeField::To => &self.to,
            ComposeField::Cc => &self.cc,
            ComposeField::Bcc => &self.bcc,
            ComposeField::Subject => &self.subject,
            ComposeField::Body => &self.body,
        }
    }

    fn set(&mut self, field: ComposeField, value: String) {
        let slot = match field {
            ComposeField::To => &mut self.to,
            ComposeField::Cc => &mut self.cc,
            ComposeField::Bcc => &mut self.bcc,
            ComposeField::Subject => &mut self.subject,
            ComposeField::Body => &mut self.body,
        };
        *slot = value;
        if matches!(
            field,
            ComposeField::To | ComposeField::Cc | ComposeField::Bcc
        ) {
            self.reread_recipients();
        }
        self.edited();
    }

    /// The draft changed: a pending discard question goes, and so does a
    /// failed send, but not a send whose outcome is unconfirmed. That warning
    /// stays until the user knows whether the message went.
    fn edited(&mut self) {
        self.confirming_discard = false;
        self.asked_again = false;
        if matches!(&self.state, Sending::Failed(error) if *error != SendError::Unconfirmed) {
            self.state = Sending::Writing;
        }
    }

    /// Reads the recipient fields under another rule for domains. An address
    /// that changes form or stops being taken loses what was looked up.
    fn set_domains(&mut self, domains: Domains) {
        self.domains = domains;
        self.reread_recipients();
    }

    /// Reads the recipient fields again, after one of them or the rule for
    /// domains changed. What was looked up for an address that left them goes.
    fn reread_recipients(&mut self) {
        self.typed = Typed::read(&self.to, &self.cc, &self.bcc, self.domains);
        let typed: HashSet<&str> = self.typed.addresses.iter().map(String::as_str).collect();
        self.protection
            .retain(|address, _| typed.contains(address.as_str()));
    }

    pub fn showing_copies(&self) -> bool {
        self.copies_shown
    }

    pub fn confirming_discard(&self) -> bool {
        self.confirming_discard
    }

    /// Whether another message was asked for while this one is open.
    pub fn asked_again(&self) -> bool {
        self.asked_again
    }

    /// Whether the draft is folded into a strip over the reader.
    pub fn folded(&self) -> bool {
        self.folded
    }

    /// What to say about protection, once every typed recipient was looked
    /// up. Silent while an address is still unchecked, so a partial answer
    /// never reads as the whole one.
    pub fn protection_notice(&self) -> Option<ProtectionNotice> {
        let typed = &self.typed.addresses;
        let looked_up = typed.len().min(PROTECTION_LOOKUP_LIMIT);
        let mut checked = Vec::with_capacity(looked_up);
        for address in &typed[..looked_up] {
            checked.push((address, *self.protection.get(address)?));
        }
        let unchecked = typed.len() - looked_up;
        let with = |wanted: fn(Checked) -> bool| -> Vec<String> {
            checked
                .iter()
                .filter(|(_, state)| wanted(*state))
                .map(|(address, _)| (*address).clone())
                .collect()
        };
        let unencrypted = with(|state| state == Checked::Known(Protection::Unencrypted));
        let unknown = with(|state| state == Checked::Unknown);
        if checked.is_empty() {
            None
        } else if !unencrypted.is_empty() || !unknown.is_empty() || unchecked > 0 {
            Some(ProtectionNotice::Warning {
                unencrypted,
                unknown,
                unchecked,
            })
        } else if checked.iter().any(|(_, state)| *state == Checked::Checking) {
            Some(ProtectionNotice::Checking)
        } else {
            Some(ProtectionNotice::Encrypted)
        }
    }

    /// Each domain typed in another script beside the ASCII form the message
    /// goes to, so a look-alike of a familiar domain shows for what it is.
    pub fn international_notice(&self) -> Option<String> {
        let mut domains: Vec<&InternationalDomain> = Vec::new();
        for field in [&self.typed.to, &self.typed.cc, &self.typed.bcc] {
            for domain in &field.international {
                if !domains.contains(&domain) {
                    domains.push(domain);
                }
            }
        }
        if domains.is_empty() {
            return None;
        }
        let mut lines = vec![
            "Check that each domain is the one you mean: letters from another script can imitate a familiar one."
                .to_owned(),
        ];
        lines.extend(
            domains
                .iter()
                .map(|domain| format!("{} is sent as {}", domain.typed, domain.ascii)),
        );
        Some(lines.join("\n"))
    }

    pub fn attachments(&self) -> &[PathBuf] {
        &self.attachments
    }

    /// The size of an attached file, once it has been read.
    pub fn attachment_size(&self, path: &Path) -> Option<u64> {
        self.sizes.get(path).copied().flatten()
    }

    pub fn add_attachments(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        for path in paths {
            if !self.attachments.contains(&path) {
                self.attachments.push(path);
            }
        }
        self.edited();
    }

    pub fn remove_attachment(&mut self, index: usize) {
        if index < self.attachments.len() {
            let path = self.attachments.remove(index);
            self.sizes.remove(&path);
            self.edited();
        }
    }

    /// What is being answered, when something is.
    pub fn answering(&self) -> Option<&Answering> {
        self.answering.as_ref()
    }

    /// For an answer, whether the original goes under the text.
    pub fn quote(&self) -> Option<Quote> {
        match &self.kind {
            OutgoingKind::New => None,
            OutgoingKind::Reply { quote, .. } | OutgoingKind::Forward { quote, .. } => Some(*quote),
        }
    }

    /// The files of the original that go with a forward: those the reader
    /// listed, less the ones left out.
    pub fn forwarded(&self) -> impl Iterator<Item = &MailAttachment> {
        let leave_out = self.kind.leave_out();
        self.answering
            .iter()
            .flat_map(|answering| &answering.forwarded)
            .filter(move |file| !leave_out.contains(&file.id))
    }

    fn leave_out_forwarded(&mut self, id: &str) {
        if let OutgoingKind::Forward { leave_out, .. } = &mut self.kind
            && !leave_out.iter().any(|known| known == id)
        {
            leave_out.push(id.to_owned());
        }
    }

    fn toggle_quote(&mut self) {
        if let OutgoingKind::Reply { quote, .. } | OutgoingKind::Forward { quote, .. } =
            &mut self.kind
        {
            *quote = match quote {
                Quote::Original => Quote::Omitted,
                Quote::Omitted => Quote::Original,
            };
        }
    }

    /// Whether the window asks for a subject. Core writes one for an answer.
    pub fn asks_for_subject(&self) -> bool {
        matches!(self.kind, OutgoingKind::New)
    }

    pub fn sending(&self) -> Sending {
        self.state.clone()
    }

    pub fn in_flight(&self) -> bool {
        matches!(self.state, Sending::InFlight)
    }

    /// Whether anything has been typed. Closing an untouched message needs no
    /// second thought; closing a written one does. A reply still addressed
    /// as it opened is untouched.
    pub fn is_untouched(&self) -> bool {
        self.to.trim() == self.opened_to
            && self.cc.trim() == self.opened_cc
            && self.bcc.trim().is_empty()
            && self.subject.trim().is_empty()
            && self.body.trim().is_empty()
            && self.attachments.is_empty()
    }

    /// Why the message cannot be sent, if it cannot. The window asks on every
    /// frame, so this only looks at the fields as they were last read.
    pub fn not_ready(&self) -> Option<NotReady> {
        self.check().err()
    }

    /// Whether the message may leave, without building it: nothing is copied
    /// unless something is wrong. Subjects are optional.
    fn check(&self) -> Result<(), NotReady> {
        let rejected = self.typed.rejected();
        if !rejected.is_empty() {
            // When the only trouble is a domain's script, say how to allow it.
            return Err(if self.typed.script_only {
                NotReady::InternationalDomains(rejected)
            } else {
                NotReady::BadAddresses(rejected)
            });
        }
        if self.typed.addresses.is_empty() {
            return Err(NotReady::NoRecipients);
        }
        Ok(())
    }

    /// The message to send, or why it cannot be sent.
    fn ready(&self, format: BodyFormat) -> Result<Outgoing, NotReady> {
        self.check()?;
        Ok(Outgoing {
            kind: self.kind.clone(),
            to: self.typed.to.accepted.clone(),
            cc: self.typed.cc.accepted.clone(),
            bcc: self.typed.bcc.accepted.clone(),
            subject: self.subject.trim().to_owned(),
            body: self.body.clone(),
            format,
            attachments: self.attachments.clone(),
        })
    }
}

/// What the composer says about the message it answers this way.
fn answering_for(kind: AnswerKind, message: &MailMessage, subject: Option<&str>) -> Answering {
    let (to, cc) = match kind {
        AnswerKind::Reply => (message.reply_to.clone(), Vec::new()),
        AnswerKind::ReplyAll => (message.reply_to.clone(), message.reply_all_cc.clone()),
        AnswerKind::Forward => (Vec::new(), Vec::new()),
    };
    // Core says when Reply-To moved the answer: a reply to mail you sent
    // also goes to someone other than its sender, without any redirect.
    let sender = message.sender.address.trim();
    let redirected_from = (!to.is_empty() && message.reply_redirected && !sender.is_empty())
        .then(|| sender.to_owned());
    let forwarded = match kind {
        AnswerKind::Forward => message.attachments.clone(),
        AnswerKind::Reply | AnswerKind::ReplyAll => Vec::new(),
    };
    Answering {
        sender: message
            .sender
            .display_name()
            .unwrap_or("(unknown sender)")
            .to_owned(),
        subject: subject.unwrap_or("(No subject)").to_owned(),
        kind,
        to,
        cc,
        redirected_from,
        quotes_original: message.body != MessageBody::Unreadable,
        forwarded,
    }
}

/// A message being written when Proton ended the session. It stays in memory,
/// never on disk, for its own account to pick up by signing in again.
pub(super) struct ParkedDraft {
    /// The Proton account it was written in.
    account: String,
    /// That account's address, to say who to sign in as.
    address: Option<String>,
    compose: Compose,
}

impl App {
    pub fn compose(&self) -> Option<&Compose> {
        self.compose.as_ref()
    }

    /// Whether a message waits for its account to sign in again.
    pub fn has_parked_draft(&self) -> bool {
        self.parked_draft.is_some()
    }

    /// The address of the account a waiting message belongs to, when known.
    pub fn parked_draft_address(&self) -> Option<&str> {
        self.parked_draft.as_ref()?.address.as_deref()
    }

    /// Takes the draft out of a session that is closing. Given the `account`
    /// whose session Proton ended, a written or sending draft waits in memory
    /// for it; otherwise it goes, so it never reaches another account. A send
    /// on its way may or may not have gone, so it comes back asking to check
    /// Sent; any other failure was the old session's.
    pub(super) fn park_draft(&mut self, ended: Option<String>) {
        let address = match &self.auth_state {
            super::AuthState::Authenticated { email } => email.clone(),
            _ => None,
        };
        let (Some(mut compose), Some(account)) = (self.compose.take(), ended) else {
            return;
        };
        if compose.is_untouched() && !compose.in_flight() {
            return;
        }
        compose.state = match std::mem::take(&mut compose.state) {
            Sending::InFlight | Sending::Failed(SendError::Unconfirmed) => {
                Sending::Failed(SendError::Unconfirmed)
            }
            Sending::Writing | Sending::Failed(_) => Sending::Writing,
        };
        compose.confirming_discard = false;
        self.parked_draft = Some(ParkedDraft {
            account,
            address,
            compose,
        });
    }

    /// Gives a waiting draft back when its own account has just signed in,
    /// under a new ID so nothing from the ended session reaches it, and asks
    /// again about recipients whose lookup that session left unanswered. Any
    /// other account, or the demo, drops it.
    pub(super) fn restore_parked_draft(&mut self) -> Effects {
        let Some(parked) = self.parked_draft.take() else {
            return Effects::none();
        };
        if self.account.as_deref() != Some(parked.account.as_str()) {
            return Effects::none();
        }
        let mut compose = parked.compose;
        compose.id = self.next_compose_id();
        compose.folded = false;
        compose
            .protection
            .retain(|_, checked| *checked != Checked::Checking);
        self.compose = Some(compose);
        self.check_compose_protection()
    }

    /// Folds a draft that covers the reader, so a conversation chosen while
    /// writing shows; the draft waits in a strip above it.
    pub(super) fn fold_compose(&mut self) {
        if self.draft_covers_reader()
            && let Some(compose) = self.compose.as_mut()
        {
            compose.folded = true;
        }
    }

    /// Brings a folded draft back over the reader.
    pub(super) fn continue_writing(&mut self) {
        if let Some(compose) = self.compose.as_mut() {
            compose.folded = false;
        }
    }

    pub(super) fn open_compose(&mut self) -> Effects {
        if self.compose.is_some() {
            return self.ask_for_another_message();
        }
        let id = self.next_compose_id();
        self.compose = Some(Compose {
            id,
            domains: self.settings.recipient_domains(),
            ..Compose::default()
        });
        Effects::none()
    }

    /// One message is written at a time: asking for another leaves the open
    /// one as it is, says why, and brings it back into view, unfolded or with
    /// its window forward.
    fn ask_for_another_message(&mut self) -> Effects {
        let in_window = self.draft_in_window();
        let Some(compose) = self.compose.as_mut() else {
            return Effects::none();
        };
        compose.asked_again = true;
        compose.folded = false;
        if in_window {
            Effects::ui(UiEffect::RaiseComposer)
        } else {
            Effects::none()
        }
    }

    /// Answers the message with the given id in the open conversation.
    pub(super) fn answer(&mut self, message_id: &str, kind: AnswerKind) -> Effects {
        let Some(ReaderState::Loaded(reader)) = self.mailbox().map(super::Mailbox::reader_state)
        else {
            return Effects::none();
        };
        let Some(message) = reader
            .detail()
            .messages
            .iter()
            .find(|message| message.id == message_id)
        else {
            return Effects::none();
        };
        // Read what the composer shows while the reader still holds the
        // message, rather than copying its decrypted body and attachments.
        let answering = answering_for(kind, message, reader.detail().subject.as_deref());
        self.open_answer(kind.outgoing(message_id), answering)
    }

    /// Answers or forwards a message. A reply opens addressed to whom core
    /// would answer, in fields the writer may change, Cc on show when Reply
    /// all copies anyone in.
    fn open_answer(&mut self, kind: OutgoingKind, answering: Answering) -> Effects {
        if self.compose.is_some() {
            return self.ask_for_another_message();
        }
        let id = self.next_compose_id();
        let (to, cc) = (answering.to.join(", "), answering.cc.join(", "));
        let mut compose = Compose {
            id,
            kind,
            answering: Some(answering),
            copies_shown: !cc.is_empty(),
            opened_to: to.clone(),
            opened_cc: cc.clone(),
            to,
            cc,
            domains: self.settings.recipient_domains(),
            ..Compose::default()
        };
        compose.reread_recipients();
        self.compose = Some(compose);
        // Its recipients come from a conversation Proton already holds, so
        // they are looked up at once, unlike a mailto: link's.
        self.check_compose_protection()
    }

    /// Puts an unsent message away. Untouched messages close at once;
    /// written ones ask for confirmation first.
    pub(super) fn close_compose(&mut self) {
        let Some(compose) = self.compose.as_mut() else {
            return;
        };
        if compose.in_flight() {
            return;
        }
        if compose.is_untouched() {
            self.compose = None;
        } else {
            compose.confirming_discard = true;
        }
    }

    /// Discards an unsent message even if it carries text, and one waiting
    /// for its account to sign in again.
    pub(super) fn discard_compose(&mut self) {
        if !self.compose.as_ref().is_some_and(Compose::in_flight) {
            self.compose = None;
        }
        self.parked_draft = None;
    }

    /// Cancels discard confirmation and keeps the message.
    pub(super) fn cancel_discard(&mut self) {
        if let Some(compose) = self.compose.as_mut() {
            compose.confirming_discard = false;
        }
    }

    pub(super) fn change_compose(&mut self, field: ComposeField, value: String) {
        if let Some(compose) = self.compose.as_mut().filter(|c| !c.in_flight()) {
            compose.set(field, value);
        }
    }

    /// Leaves one of the original's files out of the forward being written.
    pub(super) fn leave_out_forwarded(&mut self, id: &str) {
        if let Some(compose) = self.compose.as_mut().filter(|c| !c.in_flight()) {
            compose.leave_out_forwarded(id);
        }
    }

    /// Puts the original under the text of the answer being written, or
    /// takes it away.
    pub(super) fn toggle_compose_quote(&mut self) {
        if let Some(compose) = self.compose.as_mut().filter(|c| !c.in_flight()) {
            compose.toggle_quote();
        }
    }

    pub(super) fn toggle_compose_copies(&mut self) {
        if let Some(compose) = self.compose.as_mut() {
            compose.copies_shown = !compose.copies_shown;
        }
    }

    pub(super) fn pick_compose_attachments(&self) -> Effects {
        if let Some(compose) = self.compose.as_ref().filter(|c| !c.in_flight()) {
            Effects::ui(UiEffect::PickComposeAttachments(compose.id))
        } else {
            Effects::none()
        }
    }

    /// Attaches files to the draft `id`, and reads their sizes on the blocking
    /// pool: asking the file system can hang on a share that went away.
    pub(super) fn add_compose_attachments(
        &mut self,
        id: ComposeId,
        paths: Vec<PathBuf>,
    ) -> Effects {
        let Some(compose) = self
            .compose
            .as_mut()
            .filter(|c| c.id == id && !c.in_flight())
        else {
            return Effects::none();
        };
        compose.add_attachments(paths.iter().cloned());
        if paths.is_empty() {
            return Effects::none();
        }
        Effects::perform(
            async move {
                tokio::task::spawn_blocking(move || file_sizes(paths))
                    .await
                    .unwrap_or_default()
            },
            move |sizes| Message::AttachmentSizesRead(id, sizes),
        )
    }

    /// Records sizes read for the draft `id`, if it is still the one open.
    pub(super) fn finish_attachment_sizes(
        &mut self,
        id: ComposeId,
        sizes: HashMap<PathBuf, Option<u64>>,
    ) {
        if let Some(compose) = self.compose.as_mut().filter(|c| c.id == id) {
            compose.sizes.extend(sizes);
        }
    }

    pub(super) fn remove_compose_attachment(&mut self, index: usize) {
        if let Some(compose) = self.compose.as_mut().filter(|c| !c.in_flight()) {
            compose.remove_attachment(index);
        }
    }

    /// Turns lookups while writing on or off. Turning them off also forgets
    /// what the open draft looked up, so its notice goes with them and a
    /// late answer finds nothing waiting for it.
    pub(super) fn set_check_recipient_protection(&mut self, on: bool) {
        self.remember(|settings| settings.check_recipient_protection = on);
        if !on && let Some(compose) = self.compose.as_mut() {
            compose.protection.clear();
        }
    }

    /// Takes domains in other scripts, or stops taking them. The open draft
    /// follows at once, so what Send would do matches what Settings say.
    pub(super) fn set_accept_international_domains(&mut self, on: bool) {
        self.remember(|settings| settings.accept_international_domains = on);
        let domains = self.settings.recipient_domains();
        if let Some(compose) = self.compose.as_mut() {
            compose.set_domains(domains);
        }
    }

    /// Looks up protection for typed recipients that were not looked up yet.
    /// Runs when a recipient field loses focus, not on every keystroke, so a
    /// half-typed address is never sent to Proton. Settings can turn it off.
    pub(super) fn check_compose_protection(&mut self) -> Effects {
        if !self.settings.check_recipient_protection {
            return Effects::none();
        }
        let epoch = self.session_epoch;
        let Some(compose) = self.compose.as_mut().filter(|c| !c.in_flight()) else {
            return Effects::none();
        };
        let unchecked: Vec<String> = compose
            .typed
            .addresses
            .iter()
            // One request each; the warning counts the rest.
            .take(PROTECTION_LOOKUP_LIMIT)
            .filter(|address| !compose.protection.contains_key(address.as_str()))
            .cloned()
            .collect();
        if unchecked.is_empty() {
            return Effects::none();
        }
        let Some(backend) = self.backend.clone() else {
            return Effects::none();
        };
        for address in &unchecked {
            compose
                .protection
                .insert(address.clone(), Checked::Checking);
        }
        let id = compose.id;
        Effects::perform(
            async move { backend.protection(&unchecked).await },
            move |checks| Message::ComposeProtectionChecked(epoch, id, checks),
        )
    }

    /// Records answers for addresses that are still typed and still waiting.
    pub(super) fn finish_protection_check(
        &mut self,
        epoch: super::SessionEpoch,
        id: ComposeId,
        checks: Vec<ProtectionCheck>,
    ) {
        if !self.is_current_session(epoch) {
            return;
        }
        let Some(compose) = self.compose.as_mut().filter(|c| c.id == id) else {
            return;
        };
        for (address, result) in checks {
            if let Some(state) = compose.protection.get_mut(&address)
                && *state == Checked::Checking
            {
                *state = result.map_or(Checked::Unknown, Checked::Known);
            }
        }
    }

    /// Hands the message over, if it is ready and no other is already gone.
    pub(super) fn send_compose(&mut self) -> Effects {
        let epoch = self.session_epoch;
        let format = self.settings.compose_format;
        let Some(compose) = self.compose.as_mut().filter(|c| !c.in_flight()) else {
            return Effects::none();
        };
        let Ok(outgoing) = compose.ready(format) else {
            // Keep the existing validation error visible.
            return Effects::none();
        };
        compose.state = Sending::InFlight;

        let Some(backend) = self.backend.clone() else {
            return Effects::none();
        };
        Effects::perform(
            async move { backend.send(&outgoing).await },
            move |result| Message::Sent(epoch, result),
        )
    }

    pub(super) fn finish_send(
        &mut self,
        epoch: super::SessionEpoch,
        result: Result<(), SendError>,
    ) -> Effects {
        if !self.is_current_session(epoch) {
            return Effects::none();
        }
        let Some(compose) = self.compose.as_mut() else {
            return Effects::none();
        };
        match result {
            // Gone. The window closes, the mailbox says so, and the folder it
            // landed in catches up.
            Ok(()) => {
                self.compose = None;
                self.notice = Some(super::Notice::Sent);
                if let Some(mailbox) = &mut self.mailbox {
                    mailbox.invalidate_listings();
                    mailbox.invalidate_reader_cache();
                }
                self.reload_counts()
            }
            Err(error) => {
                let expired = error == SendError::Mailbox(MailboxError::SessionExpired);
                compose.state = Sending::Failed(error);
                // Signing in again is the way on, and the draft waits for it.
                self.end_session_if_expired(expired.then_some(MailboxError::SessionExpired));
                Effects::none()
            }
        }
    }
}

/// The size of each file, or `None` for one that cannot be read.
fn file_sizes(paths: Vec<PathBuf>) -> HashMap<PathBuf, Option<u64>> {
    paths
        .into_iter()
        .map(|path| {
            let size = std::fs::metadata(&path).ok().map(|metadata| metadata.len());
            (path, size)
        })
        .collect()
}

#[cfg(test)]
mod tests;
