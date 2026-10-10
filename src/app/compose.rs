use std::path::PathBuf;

use crate::mail::{
    BodyFormat, Kind, MailMessage, Outgoing, Protection, ProtectionCheck, SendError, recipients,
};

use super::{App, Effects, Message, ReaderState, UiEffect};

pub type ComposeId = u64;

/// One of the fields being typed into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
}

impl NotReady {
    pub fn message(&self) -> String {
        match self {
            Self::NoRecipients => "Add someone to send this to.".to_owned(),
            Self::BadAddresses(pieces) => {
                format!("This does not look like an address: {}", pieces.join(", "))
            }
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
    /// These recipients will receive the message unencrypted.
    Unencrypted(Vec<String>),
    /// These recipients could not be checked.
    Unknown(Vec<String>),
}

impl ProtectionNotice {
    pub fn message(&self) -> String {
        match self {
            Self::Checking => "Checking how this message will be protected…".to_owned(),
            Self::Encrypted => "Proton will encrypt this message for every recipient.".to_owned(),
            Self::Unencrypted(addresses) => format!(
                "Proton will deliver this message unencrypted to: {}",
                addresses.join(", ")
            ),
            Self::Unknown(addresses) => format!(
                "Could not check how this message will be protected for: {}",
                addresses.join(", ")
            ),
        }
    }
}

/// How far along a message is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sending {
    /// Still being written.
    Writing,
    /// Handed over, waiting to hear back.
    InFlight,
    /// The send failed or its outcome could not be confirmed.
    Failed(SendError),
}

/// The message being answered. Proton determines recipients and subject.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answering {
    pub sender: String,
    pub subject: String,
    pub everyone: bool,
}

/// A message being written.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Compose {
    id: ComposeId,
    kind: Kind,
    answering: Option<Answering>,
    to: String,
    cc: String,
    bcc: String,
    subject: String,
    body: String,
    attachments: Vec<PathBuf>,
    /// Protection looked up for typed recipients, by address as typed. An
    /// address leaves this list as soon as it leaves every recipient field.
    protection: Vec<(String, Checked)>,
    /// Whether the copy fields are on show. They start hidden: most mail goes
    /// to one person.
    more: bool,
    state: State,
    confirming_discard: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum State {
    #[default]
    Writing,
    InFlight,
    Failed(SendError),
}

impl Compose {
    #[cfg(test)]
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
            let typed = self.typed_recipients();
            self.protection
                .retain(|(address, _)| typed.iter().any(|other| other == address));
        }
        self.confirming_discard = false;
        // Keep an unconfirmed-send warning visible even if the draft changes.
        if matches!(self.state, State::Failed(ref error) if *error != SendError::Unconfirmed) {
            self.state = State::Writing;
        }
    }

    pub fn showing_more(&self) -> bool {
        self.more
    }

    pub fn confirming_discard(&self) -> bool {
        self.confirming_discard
    }

    /// Every well-formed address in the recipient fields, once each.
    fn typed_recipients(&self) -> Vec<String> {
        let mut typed: Vec<String> = Vec::new();
        for field in [&self.to, &self.cc, &self.bcc] {
            for address in recipients(field).accepted {
                if !typed
                    .iter()
                    .any(|other| other.eq_ignore_ascii_case(&address))
                {
                    typed.push(address);
                }
            }
        }
        typed
    }

    /// What to say about protection, once every typed recipient was looked
    /// up. Silent while an address is still unchecked, so a partial answer
    /// never reads as the whole one. Replies stay silent: Proton chooses
    /// their recipients when it sends them.
    pub fn protection_notice(&self) -> Option<ProtectionNotice> {
        if !self.asks_for_recipients() {
            return None;
        }
        let typed = self.typed_recipients();
        let mut checked = Vec::with_capacity(typed.len());
        for address in &typed {
            let (_, state) = self.protection.iter().find(|(other, _)| other == address)?;
            checked.push((address, *state));
        }
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
        } else if !unencrypted.is_empty() {
            Some(ProtectionNotice::Unencrypted(unencrypted))
        } else if !unknown.is_empty() {
            Some(ProtectionNotice::Unknown(unknown))
        } else if checked.iter().any(|(_, state)| *state == Checked::Checking) {
            Some(ProtectionNotice::Checking)
        } else {
            Some(ProtectionNotice::Encrypted)
        }
    }

    pub fn attachments(&self) -> &[PathBuf] {
        &self.attachments
    }

    pub fn add_attachments(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        for path in paths {
            if !self.attachments.contains(&path) {
                self.attachments.push(path);
            }
        }
        self.confirming_discard = false;
        if matches!(self.state, State::Failed(ref error) if *error != SendError::Unconfirmed) {
            self.state = State::Writing;
        }
    }

    pub fn remove_attachment(&mut self, index: usize) {
        if index < self.attachments.len() {
            self.attachments.remove(index);
            self.confirming_discard = false;
            if matches!(self.state, State::Failed(ref error) if *error != SendError::Unconfirmed) {
                self.state = State::Writing;
            }
        }
    }

    /// What is being answered, when something is.
    pub fn answering(&self) -> Option<&Answering> {
        self.answering.as_ref()
    }

    /// Whether the window asks who the message goes to.
    pub fn asks_for_recipients(&self) -> bool {
        Outgoing::needs_recipients(&self.kind)
    }

    /// Whether the window asks for a subject. Proton writes one for an answer.
    pub fn asks_for_subject(&self) -> bool {
        matches!(self.kind, Kind::New)
    }

    pub fn sending(&self) -> Sending {
        match &self.state {
            State::Writing => Sending::Writing,
            State::InFlight => Sending::InFlight,
            State::Failed(error) => Sending::Failed(error.clone()),
        }
    }

    pub fn in_flight(&self) -> bool {
        matches!(self.state, State::InFlight)
    }

    /// Whether anything has been typed. Closing an untouched message needs no
    /// second thought; closing a written one does.
    pub fn is_untouched(&self) -> bool {
        self.to.trim().is_empty()
            && self.cc.trim().is_empty()
            && self.bcc.trim().is_empty()
            && self.subject.trim().is_empty()
            && self.body.trim().is_empty()
            && self.attachments.is_empty()
    }

    /// Why the message cannot be sent, if it cannot.
    pub fn not_ready(&self) -> Option<NotReady> {
        self.ready(BodyFormat::PlainText).err()
    }

    /// The message to send, or its validation error. Subjects are optional.
    fn ready(&self, format: BodyFormat) -> Result<Outgoing, NotReady> {
        let (to, cc, bcc) = (
            recipients(&self.to),
            recipients(&self.cc),
            recipients(&self.bcc),
        );

        let rejected: Vec<String> = to
            .rejected
            .iter()
            .chain(&cc.rejected)
            .chain(&bcc.rejected)
            .cloned()
            .collect();
        if !rejected.is_empty() {
            return Err(NotReady::BadAddresses(rejected));
        }
        if Outgoing::needs_recipients(&self.kind)
            && to.accepted.is_empty()
            && cc.accepted.is_empty()
            && bcc.accepted.is_empty()
        {
            return Err(NotReady::NoRecipients);
        }

        Ok(Outgoing {
            kind: self.kind.clone(),
            to: to.accepted,
            cc: cc.accepted,
            bcc: bcc.accepted,
            subject: self.subject.trim().to_owned(),
            body: self.body.clone(),
            format,
            attachments: self.attachments.clone(),
        })
    }
}

impl App {
    pub fn compose(&self) -> Option<&Compose> {
        self.compose.as_ref()
    }

    pub(super) fn open_compose(&mut self) {
        if self.compose.is_none() {
            let id = self.next_compose_id();
            self.compose = Some(Compose {
                id,
                ..Compose::default()
            });
        }
    }

    /// Answers the message with the given id in the open conversation.
    pub(super) fn answer(&mut self, message_id: &str, forward: bool, everyone: bool) {
        let Some(ReaderState::Loaded(reader)) = self.mailbox().map(super::Mailbox::reader_state)
        else {
            return;
        };
        let Some(message) = reader
            .detail()
            .messages
            .iter()
            .find(|message| message.id == message_id)
        else {
            return;
        };
        let kind = if forward {
            Kind::Forward {
                message_id: message_id.to_owned(),
            }
        } else {
            Kind::Reply {
                message_id: message_id.to_owned(),
                everyone,
            }
        };
        let subject = reader.detail().subject.clone();
        let message = message.clone();
        self.open_answer(kind, &message, subject.as_deref());
    }

    /// Answers or forwards a message. Proton addresses replies itself.
    pub(super) fn open_answer(&mut self, kind: Kind, message: &MailMessage, subject: Option<&str>) {
        if self.compose.is_some() {
            return;
        }
        let id = self.next_compose_id();
        let everyone = matches!(kind, Kind::Reply { everyone: true, .. });
        self.compose = Some(Compose {
            id,
            kind,
            answering: Some(Answering {
                sender: message
                    .sender
                    .display_name()
                    .unwrap_or("(unknown sender)")
                    .to_owned(),
                subject: subject.unwrap_or("(No subject)").to_owned(),
                everyone,
            }),
            ..Compose::default()
        });
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

    /// Discards an unsent message even if it carries text.
    pub(super) fn discard_compose(&mut self) {
        if !self.compose.as_ref().is_some_and(Compose::in_flight) {
            self.compose = None;
        }
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

    pub(super) fn toggle_compose_copies(&mut self) {
        if let Some(compose) = self.compose.as_mut() {
            compose.more = !compose.more;
        }
    }

    pub(super) fn pick_compose_attachments(&self) -> Effects {
        if let Some(compose) = self.compose.as_ref().filter(|c| !c.in_flight()) {
            Effects::ui(UiEffect::PickComposeAttachments(compose.id))
        } else {
            Effects::none()
        }
    }

    pub(super) fn add_compose_attachments(&mut self, id: ComposeId, paths: Vec<PathBuf>) {
        if let Some(compose) = self
            .compose
            .as_mut()
            .filter(|c| c.id == id && !c.in_flight())
        {
            compose.add_attachments(paths);
        }
    }

    pub(super) fn remove_compose_attachment(&mut self, index: usize) {
        if let Some(compose) = self.compose.as_mut().filter(|c| !c.in_flight()) {
            compose.remove_attachment(index);
        }
    }

    /// Looks up protection for typed recipients that were not looked up yet.
    /// Runs when a recipient field loses focus, not on every keystroke, so a
    /// half-typed address is never sent to Proton.
    pub(super) fn check_compose_protection(&mut self) -> Effects {
        let epoch = self.session_epoch;
        let Some(compose) = self
            .compose
            .as_mut()
            .filter(|c| !c.in_flight() && c.asks_for_recipients())
        else {
            return Effects::none();
        };
        let unchecked: Vec<String> = compose
            .typed_recipients()
            .into_iter()
            .filter(|address| !compose.protection.iter().any(|(other, _)| other == address))
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
                .push((address.clone(), Checked::Checking));
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
            if let Some((_, state)) = compose
                .protection
                .iter_mut()
                .find(|(other, state)| *other == address && *state == Checked::Checking)
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
        compose.state = State::InFlight;

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
            // Gone. The window closes, and the folder it landed in catches up.
            Ok(()) => {
                self.compose = None;
                if let Some(mailbox) = &mut self.mailbox {
                    mailbox.invalidate_listings();
                    mailbox.invalidate_reader_cache();
                }
                self.reload_counts()
            }
            Err(error) => {
                compose.state = State::Failed(error);
                Effects::none()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail::MailboxError;

    fn written(to: &str) -> Compose {
        let mut compose = Compose::default();
        compose.set(ComposeField::To, to.to_owned());
        compose.set(ComposeField::Subject, "  Thursday  ".to_owned());
        compose.set(ComposeField::Body, "See you then.".to_owned());
        compose
    }

    fn composing(to: &str) -> App {
        let (mut app, _) = App::boot(true, crate::settings::Settings::default());
        let _ = app.update(Message::OpenCompose);
        let _ = app.update(Message::ComposeChanged(ComposeField::To, to.to_owned()));
        app
    }

    fn checked(app: &mut App, checks: Vec<ProtectionCheck>) {
        let epoch = app.session_epoch;
        let id = app.compose().unwrap().id();
        let _ = app.update(Message::ComposeProtectionChecked(epoch, id, checks));
    }

    fn notice(app: &App) -> Option<ProtectionNotice> {
        app.compose().unwrap().protection_notice()
    }

    #[test]
    fn protection_is_looked_up_when_a_recipient_field_is_left() {
        let mut app = composing("ada@proton.me, half-typed");
        assert_eq!(notice(&app), None);

        let effects = app.update(Message::ComposeRecipientsLeft);
        assert_eq!(effects.units(), 1);
        assert_eq!(notice(&app), Some(ProtectionNotice::Checking));

        // Nothing new to ask about: the same address is not looked up twice.
        assert_eq!(app.update(Message::ComposeRecipientsLeft).units(), 0);

        checked(
            &mut app,
            vec![("ada@proton.me".into(), Ok(Protection::EndToEnd))],
        );
        assert_eq!(notice(&app), Some(ProtectionNotice::Encrypted));
    }

    #[test]
    fn an_unencrypted_recipient_is_named() {
        let mut app = composing("ada@proton.me; key@example.org, plain@example.com");
        let _ = app.update(Message::ComposeRecipientsLeft);
        checked(
            &mut app,
            vec![
                ("ada@proton.me".into(), Ok(Protection::EndToEnd)),
                ("key@example.org".into(), Ok(Protection::Pgp)),
                ("plain@example.com".into(), Ok(Protection::Unencrypted)),
            ],
        );

        assert_eq!(
            notice(&app),
            Some(ProtectionNotice::Unencrypted(vec![
                "plain@example.com".into()
            ]))
        );
    }

    #[test]
    fn a_failed_lookup_claims_nothing() {
        let mut app = composing("ada@proton.me");
        let _ = app.update(Message::ComposeRecipientsLeft);
        checked(
            &mut app,
            vec![("ada@proton.me".into(), Err(MailboxError::Connection))],
        );

        assert_eq!(
            notice(&app),
            Some(ProtectionNotice::Unknown(vec!["ada@proton.me".into()]))
        );
    }

    #[test]
    fn a_new_address_silences_the_notice_until_it_is_checked() {
        let mut app = composing("ada@proton.me");
        let _ = app.update(Message::ComposeRecipientsLeft);
        checked(
            &mut app,
            vec![("ada@proton.me".into(), Ok(Protection::EndToEnd))],
        );

        let _ = app.update(Message::ComposeChanged(
            ComposeField::To,
            "ada@proton.me, plain@example.com".into(),
        ));
        assert_eq!(notice(&app), None);

        // Only the new address is asked about.
        assert_eq!(app.update(Message::ComposeRecipientsLeft).units(), 1);
        assert_eq!(notice(&app), Some(ProtectionNotice::Checking));
    }

    #[test]
    fn an_answer_for_a_removed_address_is_dropped() {
        let mut app = composing("plain@example.com");
        let _ = app.update(Message::ComposeRecipientsLeft);
        let _ = app.update(Message::ComposeChanged(
            ComposeField::To,
            "ada@proton.me".into(),
        ));
        checked(
            &mut app,
            vec![("plain@example.com".into(), Ok(Protection::Unencrypted))],
        );

        assert_eq!(notice(&app), None);
    }

    #[test]
    fn an_answer_for_another_draft_is_dropped() {
        let mut app = composing("plain@example.com");
        let _ = app.update(Message::ComposeRecipientsLeft);
        let epoch = app.session_epoch;
        let stale = app.compose().unwrap().id() + 1;
        let _ = app.update(Message::ComposeProtectionChecked(
            epoch,
            stale,
            vec![("plain@example.com".into(), Ok(Protection::Unencrypted))],
        ));

        assert_eq!(notice(&app), Some(ProtectionNotice::Checking));
    }

    #[test]
    fn a_reply_says_nothing_about_protection() {
        let mut compose = written("ada@proton.me");
        compose
            .protection
            .push(("ada@proton.me".into(), Checked::Known(Protection::EndToEnd)));
        assert_eq!(
            compose.protection_notice(),
            Some(ProtectionNotice::Encrypted)
        );

        compose.kind = Kind::Reply {
            message_id: "message".into(),
            everyone: false,
        };
        assert_eq!(compose.protection_notice(), None);
    }

    #[test]
    fn a_message_needs_someone_to_go_to() {
        let mut compose = Compose::default();
        assert_eq!(compose.not_ready(), Some(NotReady::NoRecipients));

        compose.set(ComposeField::Subject, "Thursday".to_owned());
        assert_eq!(compose.not_ready(), Some(NotReady::NoRecipients));

        compose.set(ComposeField::To, "alex@example.com".to_owned());
        assert_eq!(compose.not_ready(), None);
    }

    #[test]
    fn a_copy_field_alone_is_enough_to_send() {
        let mut compose = Compose::default();
        compose.set(ComposeField::Bcc, "alex@example.com".to_owned());

        assert_eq!(compose.not_ready(), None);
    }

    #[test]
    fn one_bad_address_stops_the_whole_message() {
        // Sending to everyone else and quietly dropping the rest would reach
        // fewer people than the sender believes.
        let mut compose = written("alex@example.com, oops");
        compose.set(ComposeField::Cc, "also-wrong".to_owned());

        let Some(NotReady::BadAddresses(pieces)) = compose.not_ready() else {
            panic!("expected the addresses to be refused");
        };
        assert_eq!(pieces, ["oops", "also-wrong"]);
        assert!(compose.not_ready().unwrap().message().contains("oops"));
    }

    #[test]
    fn a_ready_message_carries_what_was_typed() {
        let compose = written("alex@example.com; sam@example.org");
        let outgoing = compose.ready(BodyFormat::PlainText).unwrap();

        assert_eq!(outgoing.to, ["alex@example.com", "sam@example.org"]);
        // A subject is tidied, a body is not: its spacing is the message.
        assert_eq!(outgoing.subject, "Thursday");
        assert_eq!(outgoing.body, "See you then.");
        assert_eq!(outgoing.format, BodyFormat::PlainText);
    }

    #[test]
    fn the_format_comes_from_the_settings_not_the_window() {
        let compose = written("alex@example.com");

        assert!(!compose.ready(BodyFormat::PlainText).unwrap().is_html());
        assert!(compose.ready(BodyFormat::Html).unwrap().is_html());
    }

    #[test]
    fn a_message_with_no_subject_may_still_be_sent() {
        let mut compose = Compose::default();
        compose.set(ComposeField::To, "alex@example.com".to_owned());

        assert_eq!(compose.not_ready(), None);
        assert_eq!(compose.ready(BodyFormat::PlainText).unwrap().subject, "");
    }

    #[test]
    fn typing_after_a_refusal_clears_it() {
        let mut compose = written("alex@example.com");
        compose.state = State::Failed(SendError::DemoLimitReached);

        compose.set(ComposeField::Body, "Actually, Friday.".to_owned());

        assert_eq!(compose.sending(), Sending::Writing);
    }

    #[test]
    fn editing_does_not_hide_an_unconfirmed_send_warning() {
        let mut compose = written("alex@example.com");
        compose.state = State::Failed(SendError::Unconfirmed);

        compose.set(ComposeField::Body, "A revised body.".to_owned());
        compose.add_attachments([PathBuf::from("note.txt")]);
        compose.remove_attachment(0);

        assert_eq!(compose.sending(), Sending::Failed(SendError::Unconfirmed));
    }

    #[test]
    fn an_untouched_message_is_told_from_a_written_one() {
        assert!(Compose::default().is_untouched());
        assert!(!written("alex@example.com").is_untouched());

        let mut spaces = Compose::default();
        spaces.set(ComposeField::Body, "   \n ".to_owned());
        assert!(spaces.is_untouched(), "whitespace is not writing");
    }

    #[test]
    fn closing_an_untouched_message_discards_immediately() {
        let mut app = App::new(crate::settings::Settings::default());
        app.open_compose();
        assert!(app.compose().is_some());

        app.close_compose();
        assert!(app.compose().is_none());
    }

    #[test]
    fn closing_a_written_message_asks_for_confirmation() {
        let mut app = App::new(crate::settings::Settings::default());
        app.open_compose();
        app.change_compose(ComposeField::Body, "Draft content".to_owned());

        app.close_compose();
        assert!(app.compose().is_some());
        assert!(app.compose().unwrap().confirming_discard());

        // Cancelling confirmation returns to writing
        app.cancel_discard();
        assert!(!app.compose().unwrap().confirming_discard());
        assert!(app.compose().is_some());

        // Discarding clears the draft
        app.discard_compose();
        assert!(app.compose().is_none());
    }

    #[test]
    fn attachments_make_a_message_touched() {
        let mut compose = Compose::default();
        assert!(compose.is_untouched());

        compose.add_attachments([PathBuf::from("/tmp/test.txt")]);
        assert!(!compose.is_untouched());
        assert_eq!(compose.attachments(), &[PathBuf::from("/tmp/test.txt")]);

        compose.remove_attachment(0);
        assert!(compose.attachments().is_empty());
        assert!(compose.is_untouched());
    }

    #[test]
    fn attachments_are_deduplicated() {
        let mut compose = Compose::default();
        compose.add_attachments([
            PathBuf::from("/tmp/test.txt"),
            PathBuf::from("/tmp/test.txt"),
        ]);
        assert_eq!(compose.attachments().len(), 1);
    }

    #[test]
    fn ready_carries_attachments_without_blocking_io() {
        let mut compose = written("alex@example.com");
        let non_existent = PathBuf::from("/tmp/this_file_does_not_exist_ruston_test.xyz");
        compose.add_attachments([non_existent.clone()]);

        // In-memory UI readiness must not perform blocking disk I/O
        assert_eq!(compose.not_ready(), None);

        let outgoing = compose.ready(BodyFormat::PlainText).unwrap();
        assert_eq!(outgoing.attachments, vec![non_existent]);
    }
}
