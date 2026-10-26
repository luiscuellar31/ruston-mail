//! The message being written in the terminal interface: the address it goes
//! from, its fields and its text, which of them the keys type in, and
//! whether it is on its way. It lives in memory only, never on disk.

use super::editor::Editor;
use crate::mailbox::Outgoing;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ruston_core::RecipientProtection;
use ruston_core::mail::recipients::{Domains, recipients};
use std::cell::Cell;

/// What the editor says when Proton may have sent the message or may not:
/// sending it again could deliver it twice.
pub(crate) const UNCONFIRMED: &str =
    "Could not confirm whether Proton sent this message. Check Sent before trying again.";

/// The fields of a message, in the order Tab goes through them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Field {
    To,
    Cc,
    Bcc,
    Subject,
    Body,
}

impl Field {
    /// The fields of one line, above the text.
    pub(crate) const HEADERS: [Self; 4] = [Self::To, Self::Cc, Self::Bcc, Self::Subject];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::To => "To",
            Self::Cc => "Cc",
            Self::Bcc => "Bcc",
            Self::Subject => "Subject",
            Self::Body => "",
        }
    }

    fn next(self) -> Self {
        match self {
            Self::To => Self::Cc,
            Self::Cc => Self::Bcc,
            Self::Bcc => Self::Subject,
            Self::Subject => Self::Body,
            Self::Body => Self::To,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::To => Self::Body,
            Self::Cc => Self::To,
            Self::Bcc => Self::Cc,
            Self::Subject => Self::Bcc,
            Self::Body => Self::Subject,
        }
    }

    /// The field above, To being the first.
    fn above(self) -> Self {
        match self {
            Self::To => Self::To,
            other => other.previous(),
        }
    }

    /// The field below, the text being the last.
    fn below(self) -> Self {
        match self {
            Self::Body => Self::Body,
            other => other.next(),
        }
    }
}

/// How far along the message is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sending {
    /// Being written.
    Writing,
    /// Handed over: nothing changes it until Proton answers.
    InFlight,
    /// Proton may have sent it or may not. It is still here to be sent
    /// again, and says so until it goes.
    Unconfirmed,
}

/// What Proton would do for one recipient of the message about to go, as
/// far as is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Checked {
    /// Asked, not answered yet.
    Checking,
    Known(RecipientProtection),
    /// The lookup failed, so nothing is claimed either way.
    Unknown,
}

/// What a key asks of the interface beyond the message itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Asked {
    Nothing,
    Close,
    Send,
}

/// A message being written.
#[derive(Debug)]
pub(crate) struct Compose {
    /// The address it goes from, as core picks it.
    pub(crate) from: String,
    pub(crate) to: Editor,
    pub(crate) cc: Editor,
    pub(crate) bcc: Editor,
    pub(crate) subject: Editor,
    pub(crate) body: Editor,
    /// The field the keys type in.
    pub(crate) focus: Field,
    pub(crate) sending: Sending,
    /// What went wrong, for the editor's foot until the next key.
    pub(crate) notice: Option<String>,
    /// The first of the text's rows on screen. The view moves it to keep
    /// the cursor in sight, as only it knows how many rows fit.
    pub(crate) top: Cell<usize>,
    /// The text's width and height when last drawn, for Up, Down and a
    /// page, which move by the rows it wraps to.
    pub(crate) body_width: Cell<u16>,
    pub(crate) body_rows: Cell<u16>,
}

impl Compose {
    /// A new message from `from`, its keys typing in To.
    pub(crate) fn new(from: String) -> Self {
        Self {
            from,
            to: Editor::default(),
            cc: Editor::default(),
            bcc: Editor::default(),
            subject: Editor::default(),
            body: Editor::default(),
            focus: Field::To,
            sending: Sending::Writing,
            notice: None,
            top: Cell::new(0),
            body_width: Cell::new(0),
            body_rows: Cell::new(0),
        }
    }

    pub(crate) fn field(&self, field: Field) -> &Editor {
        match field {
            Field::To => &self.to,
            Field::Cc => &self.cc,
            Field::Bcc => &self.bcc,
            Field::Subject => &self.subject,
            Field::Body => &self.body,
        }
    }

    fn field_mut(&mut self, field: Field) -> &mut Editor {
        match field {
            Field::To => &mut self.to,
            Field::Cc => &mut self.cc,
            Field::Bcc => &mut self.bcc,
            Field::Subject => &mut self.subject,
            Field::Body => &mut self.body,
        }
    }

    /// What the editor's foot says in place of the keys: that the message
    /// is on its way, what went wrong, or that a send may have gone, which
    /// it says until the message goes.
    pub(crate) fn said(&self) -> Option<&str> {
        match (&self.notice, self.sending) {
            (_, Sending::InFlight) => Some("Sending…"),
            (Some(notice), _) => Some(notice),
            (None, Sending::Unconfirmed) => Some(UNCONFIRMED),
            (None, Sending::Writing) => None,
        }
    }

    /// Whether only blanks were written, so closing it loses nothing.
    pub(crate) fn untouched(&self) -> bool {
        [&self.to, &self.cc, &self.bcc, &self.subject, &self.body]
            .iter()
            .all(|editor| editor.is_blank())
    }

    /// What `key` does: types in the focused field or moves through the
    /// message, or asks for it to be closed or sent. Esc and Ctrl+C close
    /// it; Ctrl+Enter, Proton Mail's key, sends it, and so does Ctrl+S,
    /// since many terminals send Ctrl+Enter as Enter alone and some as
    /// Ctrl+J. Nothing changes it while it is on its way.
    pub(crate) fn key(&mut self, key: &KeyEvent) -> Asked {
        if self.sending == Sending::InFlight {
            return Asked::Nothing;
        }
        // Windows reports what AltGr types as Ctrl with Alt.
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Esc => return Asked::Close,
            KeyCode::Char('c') if ctrl => return Asked::Close,
            KeyCode::Enter if ctrl => return Asked::Send,
            KeyCode::Char('s' | 'j') if ctrl => return Asked::Send,
            _ => {}
        }
        let width = usize::from(self.body_width.get()).max(1);
        let page = isize::try_from(self.body_rows.get()).map_or(1, |rows| rows.max(1));
        let focus = self.focus;
        match key.code {
            KeyCode::Tab => self.focus = focus.next(),
            KeyCode::BackTab => self.focus = focus.previous(),
            KeyCode::Enter if focus == Field::Body => self.body.break_line(),
            KeyCode::Enter => self.focus = focus.next(),
            KeyCode::Up if focus == Field::Body => {
                if !self.body.vertical(width, -1) {
                    self.focus = Field::Subject;
                }
            }
            KeyCode::Up => self.focus = focus.above(),
            KeyCode::Down if focus == Field::Body => {
                self.body.vertical(width, 1);
            }
            KeyCode::Down => self.focus = focus.below(),
            KeyCode::PageUp if focus == Field::Body => {
                self.body.vertical(width, -page);
            }
            KeyCode::PageDown if focus == Field::Body => {
                self.body.vertical(width, page);
            }
            KeyCode::Home => self.field_mut(focus).home(ctrl),
            KeyCode::End => self.field_mut(focus).end(ctrl),
            KeyCode::Left => self.field_mut(focus).left(),
            KeyCode::Right => self.field_mut(focus).right(),
            KeyCode::Backspace => self.field_mut(focus).backspace(),
            KeyCode::Delete => self.field_mut(focus).delete(),
            KeyCode::Char(c) if !ctrl => self.field_mut(focus).insert(c),
            _ => return Asked::Nothing,
        }
        // What went wrong is done with once the message changes.
        self.notice = None;
        Asked::Nothing
    }

    /// The message to send, or why it cannot go yet: each recipient field
    /// read with the rules the desktop reads them with, taking domains in
    /// ASCII only, as the desktop does unless told otherwise.
    pub(crate) fn ready(&self) -> Result<Outgoing, String> {
        let read = |editor: &Editor| recipients(&editor.text(), Domains::Ascii);
        let (to, cc, bcc) = (read(&self.to), read(&self.cc), read(&self.bcc));
        let rejected: Vec<String> = to
            .rejected
            .into_iter()
            .chain(cc.rejected)
            .chain(bcc.rejected)
            .collect();
        if !rejected.is_empty() {
            let pieces = rejected.join(", ");
            // When a domain's script is all that is wrong, say how to type it.
            let script_only = rejected.iter().all(|piece| {
                recipients(piece, Domains::International)
                    .rejected
                    .is_empty()
            });
            return Err(if script_only {
                format!(
                    "Domains in another script are not accepted: {pieces}. Type the domain in its xn-- form."
                )
            } else {
                format!("This does not look like an address: {pieces}")
            });
        }
        if to.accepted.is_empty() && cc.accepted.is_empty() && bcc.accepted.is_empty() {
            return Err("Add someone to send this to.".to_owned());
        }
        Ok(Outgoing {
            to: to.accepted,
            cc: cc.accepted,
            bcc: bcc.accepted,
            subject: self.subject.text().trim().to_owned(),
            body: self.body.text(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(compose: &mut Compose, code: KeyCode) -> Asked {
        compose.key(&KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn ctrl(compose: &mut Compose, code: KeyCode) -> Asked {
        compose.key(&KeyEvent::new(code, KeyModifiers::CONTROL))
    }

    fn write(compose: &mut Compose, text: &str) {
        for c in text.chars() {
            press(compose, KeyCode::Char(c));
        }
    }

    #[test]
    fn keys_type_in_the_focused_field_and_move_between_fields() {
        let mut compose = Compose::new("me@example.test".into());
        // As the view leaves it after drawing.
        compose.body_width.set(40);
        assert!(compose.untouched());
        write(&mut compose, "alex@example.test");
        press(&mut compose, KeyCode::Tab);
        assert_eq!(compose.focus, Field::Cc);
        // Enter in a field goes to the next one; Down and Up as well.
        press(&mut compose, KeyCode::Enter);
        press(&mut compose, KeyCode::Down);
        assert_eq!(compose.focus, Field::Subject);
        write(&mut compose, "Plans");
        press(&mut compose, KeyCode::Enter);
        assert_eq!(compose.focus, Field::Body);
        // In the text, Enter breaks the line, and Up from its first row
        // goes back to the subject.
        write(&mut compose, "Hi,");
        press(&mut compose, KeyCode::Enter);
        write(&mut compose, "see you.");
        assert_eq!(compose.body.text(), "Hi,\nsee you.");
        press(&mut compose, KeyCode::Up);
        assert_eq!(compose.focus, Field::Body);
        press(&mut compose, KeyCode::Up);
        assert_eq!(compose.focus, Field::Subject);
        press(&mut compose, KeyCode::BackTab);
        press(&mut compose, KeyCode::BackTab);
        press(&mut compose, KeyCode::BackTab);
        assert_eq!(compose.focus, Field::To);
        press(&mut compose, KeyCode::Up);
        assert_eq!(compose.focus, Field::To);
        assert_eq!(compose.to.text(), "alex@example.test");
        assert!(!compose.untouched());
        // Keys that are the interface's elsewhere just type here.
        write(&mut compose, "q");
        assert_eq!(compose.to.text(), "alex@example.testq");
    }

    #[test]
    fn esc_and_ctrl_c_close_and_ctrl_enter_ctrl_s_and_ctrl_j_send() {
        let mut compose = Compose::new(String::new());
        assert_eq!(press(&mut compose, KeyCode::Esc), Asked::Close);
        assert_eq!(ctrl(&mut compose, KeyCode::Char('c')), Asked::Close);
        assert_eq!(ctrl(&mut compose, KeyCode::Enter), Asked::Send);
        assert_eq!(ctrl(&mut compose, KeyCode::Char('s')), Asked::Send);
        assert_eq!(ctrl(&mut compose, KeyCode::Char('j')), Asked::Send);
        // AltGr types, as Windows reports it with Ctrl and Alt.
        let altgr = KeyEvent::new(
            KeyCode::Char('@'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        );
        assert_eq!(compose.key(&altgr), Asked::Nothing);
        assert_eq!(compose.to.text(), "@");
        // While it is on its way, nothing changes it.
        compose.sending = Sending::InFlight;
        assert_eq!(press(&mut compose, KeyCode::Esc), Asked::Nothing);
        write(&mut compose, "x");
        assert_eq!(compose.to.text(), "@");
    }

    #[test]
    fn a_message_goes_only_to_addresses_it_can_read() {
        let mut compose = Compose::new(String::new());
        assert_eq!(compose.ready(), Err("Add someone to send this to.".into()));
        write(&mut compose, "alex@example.test, not-an-address");
        assert_eq!(
            compose.ready(),
            Err("This does not look like an address: not-an-address".into())
        );
        compose.to = Editor::default();
        compose.focus = Field::To;
        write(&mut compose, "ana@m\u{fc}nchen.de");
        assert_eq!(
            compose.ready(),
            Err(
                "Domains in another script are not accepted: ana@m\u{fc}nchen.de. Type the domain in its xn-- form."
                    .into()
            )
        );
        compose.to = Editor::default();
        write(&mut compose, "ana@xn--mnchen-3ya.de; Alex@Example.test");
        compose.focus = Field::Bcc;
        write(&mut compose, "alex@example.test");
        compose.focus = Field::Subject;
        write(&mut compose, "  Plans  ");
        compose.focus = Field::Body;
        write(&mut compose, "Hi");
        let outgoing = compose.ready().unwrap();
        assert_eq!(
            outgoing,
            Outgoing {
                to: vec!["ana@xn--mnchen-3ya.de".into(), "Alex@Example.test".into()],
                cc: Vec::new(),
                bcc: vec!["alex@example.test".into()],
                subject: "Plans".into(),
                body: "Hi".into(),
            }
        );
        // The same person in two fields is looked up once.
        assert_eq!(
            outgoing.recipients(),
            ["ana@xn--mnchen-3ya.de", "Alex@Example.test"]
        );
    }

    #[test]
    fn typing_clears_what_went_wrong_but_not_an_unconfirmed_send() {
        let mut compose = Compose::new(String::new());
        assert_eq!(compose.said(), None);
        compose.notice = Some("Add someone to send this to.".into());
        assert_eq!(compose.said(), Some("Add someone to send this to."));
        write(&mut compose, "a");
        assert_eq!(compose.said(), None);
        // A send that may have gone is said again once what went wrong
        // since is done with.
        compose.sending = Sending::Unconfirmed;
        assert_eq!(compose.said(), Some(UNCONFIRMED));
        compose.notice = Some("Add someone to send this to.".into());
        write(&mut compose, "b");
        assert_eq!(compose.said(), Some(UNCONFIRMED));
        compose.sending = Sending::InFlight;
        assert_eq!(compose.said(), Some("Sending…"));
    }
}
