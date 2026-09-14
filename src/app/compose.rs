//! Writing a message.
//!
//! Sending is the one thing Ruston Mail does that other people receive and
//! that cannot be undone, so everything it depends on is decided here, where
//! it can be tested: who the message is for, whether it is ready to leave,
//! and what to say when it is not. `ui::compose` draws this and adds nothing
//! to it.

use crate::mail::{BodyFormat, Kind, MailMessage, Outgoing, SendError, recipients};

use super::{App, Effects, Message, ReaderState};

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

/// How far along a message is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sending {
    /// Still being written.
    Writing,
    /// Handed over, waiting to hear back.
    InFlight,
    /// It did not leave, and why.
    Failed(SendError),
}

/// The message being answered, as the window describes it. Proton works out
/// the recipients and the subject itself, so this says what is being answered
/// rather than promising who will receive it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answering {
    pub sender: String,
    pub subject: String,
    pub everyone: bool,
}

/// A message being written.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Compose {
    kind: Kind,
    answering: Option<Answering>,
    to: String,
    cc: String,
    bcc: String,
    subject: String,
    body: String,
    /// Whether the copy fields are on show. They start hidden: most mail goes
    /// to one person.
    more: bool,
    state: State,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum State {
    #[default]
    Writing,
    InFlight,
    Failed(SendError),
}

impl Compose {
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
        // Typing after a refusal is an attempt to fix it, so the refusal goes.
        if matches!(self.state, State::Failed(_)) {
            self.state = State::Writing;
        }
    }

    pub fn showing_more(&self) -> bool {
        self.more
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
            State::Failed(error) => Sending::Failed(*error),
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
    }

    /// Why the message cannot be sent, if it cannot.
    pub fn not_ready(&self) -> Option<NotReady> {
        self.ready(BodyFormat::PlainText).err()
    }

    /// The message to send, or what is wrong with it.
    ///
    /// A subject is not required: Proton accepts one without, and refusing
    /// would be Ruston Mail inventing a rule of its own.
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
        })
    }
}

impl App {
    pub fn compose(&self) -> Option<&Compose> {
        self.compose.as_ref()
    }

    pub(super) fn open_compose(&mut self) {
        if self.compose.is_none() {
            self.compose = Some(Compose::default());
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

    /// Answers the message, or passes it on.
    ///
    /// What the window shows is what is being answered, not who will receive
    /// it: for a reply Proton works the recipients out from the message
    /// itself, and repeating that rule here is a second copy of it that could
    /// drift from the first.
    pub(super) fn open_answer(&mut self, kind: Kind, message: &MailMessage, subject: Option<&str>) {
        if self.compose.is_some() {
            return;
        }
        let everyone = matches!(kind, Kind::Reply { everyone: true, .. });
        self.compose = Some(Compose {
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

    /// Puts an unsent message away. One already on its way is left alone: it
    /// is out of the sender's hands, and the window says so until it lands.
    pub(super) fn close_compose(&mut self) {
        if !self.compose.as_ref().is_some_and(Compose::in_flight) {
            self.compose = None;
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

    /// Hands the message over, if it is ready and no other is already gone.
    pub(super) fn send_compose(&mut self) -> Effects {
        let format = self.settings.compose_format;
        let Some(compose) = self.compose.as_mut().filter(|c| !c.in_flight()) else {
            return Effects::none();
        };
        let Ok(outgoing) = compose.ready(format) else {
            // The window already says what is wrong; pressing Send changes
            // nothing until it is fixed.
            return Effects::none();
        };
        compose.state = State::InFlight;

        let Some(backend) = self.backend.clone() else {
            return Effects::none();
        };
        Effects::perform(async move { backend.send(&outgoing).await }, Message::Sent)
    }

    pub(super) fn finish_send(&mut self, result: Result<(), SendError>) -> Effects {
        let Some(compose) = self.compose.as_mut() else {
            return Effects::none();
        };
        match result {
            // Gone. The window closes, and the folder it landed in catches up.
            Ok(()) => {
                self.compose = None;
                if let Some(mailbox) = &mut self.mailbox {
                    mailbox.invalidate_listings();
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

    fn written(to: &str) -> Compose {
        let mut compose = Compose::default();
        compose.set(ComposeField::To, to.to_owned());
        compose.set(ComposeField::Subject, "  Thursday  ".to_owned());
        compose.set(ComposeField::Body, "See you then.".to_owned());
        compose
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
    fn an_untouched_message_is_told_from_a_written_one() {
        assert!(Compose::default().is_untouched());
        assert!(!written("alex@example.com").is_untouched());

        let mut spaces = Compose::default();
        spaces.set(ComposeField::Body, "   \n ".to_owned());
        assert!(spaces.is_untouched(), "whitespace is not writing");
    }
}
