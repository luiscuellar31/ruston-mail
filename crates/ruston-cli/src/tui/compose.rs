//! The message being written in the terminal interface: the address it goes
//! from, its fields and its text, which of them the keys type in, and
//! whether it is on its way. It lives in memory only, never on disk.

use super::editor::Editor;
use super::reading::Answerable;
use crate::mailbox::{Outgoing, OutgoingKind};
use crate::terminal::screen_line;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ruston_core::mail::recipients::{Domains, recipients};
use ruston_core::{Quote, RecipientProtection};
use std::cell::Cell;
use std::path::{Path, PathBuf};

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
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::To => "To",
            Self::Cc => "Cc",
            Self::Bcc => "Bcc",
            Self::Subject => "Subject",
            Self::Body => "",
        }
    }
}

/// A new message's fields, in the order Tab goes through them.
const NEW_FIELDS: [Field; 5] = [
    Field::To,
    Field::Cc,
    Field::Bcc,
    Field::Subject,
    Field::Body,
];

/// An answer's: core writes its subject.
const ANSWER_FIELDS: [Field; 4] = [Field::To, Field::Cc, Field::Bcc, Field::Body];

/// How a message is answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AnswerKind {
    /// To whoever the message asks replies to go to.
    Reply,
    /// To them, and copied to everyone else it went to.
    ReplyAll,
    /// Passed on, with its files, to recipients typed.
    Forward,
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Asked {
    Nothing,
    Close,
    Send,
    /// To check the file the path typed names, and attach it.
    Attach(String),
    /// To find the names the path typed may go on as.
    Complete(String),
}

/// The files listed over the message while Ctrl+A has them open: the path
/// typed to attach one, a file chosen to take away, the names the path may
/// go on as, and what went wrong.
#[derive(Debug, Default)]
pub(crate) struct Files {
    pub(crate) path: Editor,
    /// Which of the files listed is chosen, or none while a path is typed.
    pub(crate) chosen: Option<usize>,
    /// The names the path may go on as, once Tab found several.
    pub(crate) matches: Vec<String>,
    pub(crate) notice: Option<String>,
}

/// A file the message carries, as the attachments list it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Listed {
    /// Its name, safe to draw.
    pub(crate) name: String,
    pub(crate) size: u64,
    /// Whether it is the original's, which a forward carries.
    pub(crate) original: bool,
    /// Whether the forward leaves it out.
    pub(crate) left_out: bool,
}

/// A message being written.
#[derive(Debug)]
pub(crate) struct Compose {
    /// The address it goes from, as core picks it.
    pub(crate) from: String,
    /// What it answers, and whether the original goes under its text.
    pub(crate) kind: OutgoingKind,
    /// The message it answers, as the editor names it.
    pub(crate) answering: Option<Answerable>,
    /// What To and Cc opened with: for a reply, whom core would answer.
    /// Leaving them as they were is not writing.
    opened_to: String,
    opened_cc: String,
    /// Files of this computer it carries, with their sizes.
    pub(crate) attached: Vec<(PathBuf, u64)>,
    /// The files it carries, while listed over it.
    pub(crate) files: Option<Files>,
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
            kind: OutgoingKind::New,
            answering: None,
            opened_to: String::new(),
            opened_cc: String::new(),
            attached: Vec::new(),
            files: None,
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

    /// An answer to `answering`, as `how` says: a reply addressed as core
    /// would address it, its keys typing in the text, or a forward typing in
    /// To. It goes from the address the original reached, or `from`, with
    /// the original under its text when the original opened.
    pub(crate) fn answer(from: String, how: AnswerKind, answering: Answerable) -> Self {
        let message_id = answering.message_id.clone();
        let quote = Quote::Original;
        let (kind, to, cc) = match how {
            AnswerKind::Reply => (
                OutgoingKind::Reply {
                    message_id,
                    everyone: false,
                    quote,
                },
                answering.to.join(", "),
                String::new(),
            ),
            AnswerKind::ReplyAll => (
                OutgoingKind::Reply {
                    message_id,
                    everyone: true,
                    quote,
                },
                answering.to.join(", "),
                answering.cc.join(", "),
            ),
            AnswerKind::Forward => (
                OutgoingKind::Forward {
                    message_id,
                    quote,
                    leave_out: Vec::new(),
                },
                String::new(),
                String::new(),
            ),
        };
        let mut compose = Self::new(answering.from.clone().unwrap_or(from));
        compose.to = Editor::with(&to);
        compose.cc = Editor::with(&cc);
        compose.opened_to = to;
        compose.opened_cc = cc;
        compose.kind = kind;
        compose.focus = if how == AnswerKind::Forward {
            Field::To
        } else {
            Field::Body
        };
        compose.answering = Some(answering);
        compose
    }

    /// The fields of one line, above the text: a subject only for a new
    /// message, since core writes an answer's.
    pub(crate) fn headers(&self) -> &'static [Field] {
        let fields: &'static [Field] = if self.kind == OutgoingKind::New {
            &NEW_FIELDS
        } else {
            &ANSWER_FIELDS
        };
        &fields[..fields.len() - 1]
    }

    /// The field `by` places from the focused one, in the order Tab goes
    /// through them; past either end, round to the other with `round`, or
    /// else stopping there.
    fn field_after(&self, by: isize, round: bool) -> Field {
        let fields: &[Field] = if self.kind == OutgoingKind::New {
            &NEW_FIELDS
        } else {
            &ANSWER_FIELDS
        };
        let last = isize::try_from(fields.len() - 1).unwrap_or(0);
        let at = fields
            .iter()
            .position(|field| *field == self.focus)
            .and_then(|at| isize::try_from(at).ok())
            .unwrap_or(0);
        let next = if round {
            (at + by).rem_euclid(last + 1)
        } else {
            (at + by).clamp(0, last)
        };
        fields[usize::try_from(next).unwrap_or(0)]
    }

    /// Puts the original under an answer's text, or leaves it out; only an
    /// original that opened can go.
    fn toggle_quote(&mut self) {
        let opened = self.answering.as_ref().is_some_and(|answer| answer.opened);
        if let (true, OutgoingKind::Reply { quote, .. } | OutgoingKind::Forward { quote, .. }) =
            (opened, &mut self.kind)
        {
            *quote = match quote {
                Quote::Original => Quote::Omitted,
                Quote::Omitted => Quote::Original,
            };
        }
    }

    /// The files it carries: those attached, then a forward's of the
    /// original, saying which the forward leaves out.
    pub(crate) fn listed(&self) -> Vec<Listed> {
        let attached = self.attached.iter().map(|(path, size)| Listed {
            name: file_name(path),
            size: *size,
            original: false,
            left_out: false,
        });
        let (forwarded, leave_out) = match (&self.kind, &self.answering) {
            (OutgoingKind::Forward { leave_out, .. }, Some(answering)) => {
                (answering.files.as_slice(), leave_out.as_slice())
            }
            _ => (&[][..], &[][..]),
        };
        let originals = forwarded.iter().map(|file| Listed {
            name: file.name.clone(),
            size: file.size,
            original: true,
            left_out: leave_out.contains(&file.id),
        });
        attached.chain(originals).collect()
    }

    /// Takes away the file listed at `index`: an attached one goes, and a
    /// forward's of the original is left out, or put back.
    fn take_away(&mut self, index: usize) {
        if index < self.attached.len() {
            self.attached.remove(index);
            return;
        }
        let Some(id) = self
            .answering
            .as_ref()
            .and_then(|answering| answering.files.get(index - self.attached.len()))
            .map(|file| file.id.clone())
        else {
            return;
        };
        if let OutgoingKind::Forward { leave_out, .. } = &mut self.kind {
            match leave_out.iter().position(|left| *left == id) {
                Some(at) => {
                    leave_out.remove(at);
                }
                None => leave_out.push(id),
            }
        }
    }

    /// What a key does while the files are listed over the message: it
    /// types the path of one to attach, Tab completes it and Enter attaches
    /// it; Up and Down choose a file listed, which Delete takes away; Esc
    /// puts the list away.
    fn files_key(&mut self, key: &KeyEvent, ctrl: bool) -> Asked {
        let listed = self.listed().len();
        let Some(files) = self.files.as_mut() else {
            return Asked::Nothing;
        };
        let mut take_away = None;
        match key.code {
            KeyCode::Esc => {
                self.files = None;
                return Asked::Nothing;
            }
            KeyCode::Tab => {
                files.chosen = None;
                return Asked::Complete(files.path.text());
            }
            KeyCode::Enter if files.chosen.is_none() && !files.path.is_blank() => {
                return Asked::Attach(files.path.text());
            }
            KeyCode::Up => {
                files.chosen = match files.chosen {
                    None => listed.checked_sub(1),
                    Some(at) => Some(at.saturating_sub(1)),
                };
            }
            KeyCode::Down => {
                files.chosen = files
                    .chosen
                    .and_then(|at| (at + 1 < listed).then_some(at + 1));
            }
            KeyCode::Delete | KeyCode::Backspace if files.chosen.is_some() => {
                take_away = files.chosen;
            }
            KeyCode::Backspace => files.path.backspace(),
            KeyCode::Delete => files.path.delete(),
            KeyCode::Left => files.path.left(),
            KeyCode::Right => files.path.right(),
            KeyCode::Home => files.path.home(false),
            KeyCode::End => files.path.end(false),
            KeyCode::Char(c) if !ctrl => {
                files.chosen = None;
                files.matches.clear();
                files.path.insert(c);
            }
            _ => return Asked::Nothing,
        }
        files.notice = None;
        if let Some(at) = take_away {
            self.take_away(at);
            let left = self.listed().len();
            if let Some(files) = self.files.as_mut() {
                files.chosen = (left > 0).then(|| at.min(left - 1));
            }
        }
        Asked::Nothing
    }

    /// Lists `path`, `size` bytes, among the files the message carries,
    /// once checked, or says what keeps it from going.
    pub(crate) fn attached(&mut self, checked: Result<(PathBuf, u64), String>) {
        let Some(files) = self.files.as_mut() else {
            return;
        };
        match checked {
            Ok((path, size)) => {
                *files = Files::default();
                if !self.attached.iter().any(|(listed, _)| *listed == path) {
                    self.attached.push((path, size));
                }
            }
            Err(why) => files.notice = Some(why),
        }
    }

    /// Goes on with the path `typed` as the names `matches` say: to the one
    /// that fits, or as far as several share, listing them. A path typed on
    /// meanwhile is left as it is.
    pub(crate) fn completed(&mut self, typed: &str, matches: Vec<String>) {
        let Some(files) = self
            .files
            .as_mut()
            .filter(|files| files.path.text() == typed)
        else {
            return;
        };
        match matches.as_slice() {
            [] => files.notice = Some("No file or folder there begins so.".to_owned()),
            [only] => {
                files.path = Editor::with(only);
                files.matches.clear();
            }
            several => {
                files.path = Editor::with(&shared_start(several));
                files.matches = several.to_vec();
            }
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

    /// Whether nothing was written, so closing it loses nothing: a reply
    /// still addressed as it opened is untouched.
    pub(crate) fn untouched(&self) -> bool {
        self.attached.is_empty()
            && self.to.text().trim() == self.opened_to.trim()
            && self.cc.text().trim() == self.opened_cc.trim()
            && [&self.bcc, &self.subject, &self.body]
                .iter()
                .all(|editor| editor.is_blank())
    }

    /// What `key` does: types in the focused field or moves through the
    /// message, or asks for it to be closed or sent. Esc and Ctrl+C close
    /// it; Ctrl+Enter, Proton Mail's key, sends it, and so does Ctrl+S,
    /// since many terminals send Ctrl+Enter as Enter alone and some as
    /// Ctrl+J. Ctrl+Q puts the original under an answer or leaves it out.
    /// Ctrl+A, as Proton Mail's Ctrl+Shift+A, which many terminals cannot
    /// tell from it, lists the files it carries, to attach more. Nothing
    /// changes it while it is on its way.
    pub(crate) fn key(&mut self, key: &KeyEvent) -> Asked {
        if self.sending == Sending::InFlight {
            return Asked::Nothing;
        }
        // Windows reports what AltGr types as Ctrl with Alt.
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::ALT);
        if self.files.is_some() {
            return self.files_key(key, ctrl);
        }
        match key.code {
            KeyCode::Char('a' | 'A') if ctrl => {
                self.files = Some(Files::default());
                return Asked::Nothing;
            }
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
            KeyCode::Tab => self.focus = self.field_after(1, true),
            KeyCode::BackTab => self.focus = self.field_after(-1, true),
            KeyCode::Enter if focus == Field::Body => self.body.break_line(),
            KeyCode::Enter => self.focus = self.field_after(1, false),
            KeyCode::Up if focus == Field::Body => {
                if !self.body.vertical(width, -1) {
                    self.focus = self.field_after(-1, false);
                }
            }
            KeyCode::Up => self.focus = self.field_after(-1, false),
            KeyCode::Down if focus == Field::Body => {
                self.body.vertical(width, 1);
            }
            KeyCode::Down => self.focus = self.field_after(1, false),
            KeyCode::Char('q') if ctrl => self.toggle_quote(),
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

    /// Types `text`, pasted whole, as the focused field takes it: in the
    /// text, its lines stay lines; in a field of one line, they are joined,
    /// with commas between addresses and spaces in the subject; in the path
    /// of a file to attach, they go. Tabs become spaces.
    pub(crate) fn paste(&mut self, text: &str) {
        if self.sending == Sending::InFlight {
            return;
        }
        let text = text
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .replace('\t', "    ");
        if let Some(files) = self.files.as_mut() {
            files.chosen = None;
            files.matches.clear();
            files.notice = None;
            for c in text.chars().filter(|c| *c != '\n') {
                files.path.insert(c);
            }
            return;
        }
        let focus = self.focus;
        let between = match focus {
            Field::To | Field::Cc | Field::Bcc => Some(", "),
            Field::Subject => Some(" "),
            Field::Body => None,
        };
        let editor = self.field_mut(focus);
        for (index, line) in text.split('\n').enumerate() {
            if index > 0 {
                match between {
                    Some(joint) => joint.chars().for_each(|c| editor.insert(c)),
                    None => editor.break_line(),
                }
            }
            line.chars().for_each(|c| editor.insert(c));
        }
        self.notice = None;
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
            kind: self.kind.clone(),
            to: to.accepted,
            cc: cc.accepted,
            bcc: bcc.accepted,
            subject: self.subject.text().trim().to_owned(),
            body: self.body.text(),
            attachments: self.attached.iter().map(|(path, _)| path.clone()).collect(),
        })
    }
}

/// The name of the file at `path`, safe to draw.
fn file_name(path: &Path) -> String {
    let name = path
        .file_name()
        .map_or_else(|| path.to_string_lossy(), |name| name.to_string_lossy());
    screen_line(&name)
}

/// How far all of `words` begin alike, in whole characters.
fn shared_start(words: &[String]) -> String {
    let Some((first, rest)) = words.split_first() else {
        return String::new();
    };
    let mut end = first.len();
    for word in rest {
        end = first
            .char_indices()
            .zip(word.chars())
            .find(|((_, a), b)| a != b)
            .map_or(end.min(word.len()), |((at, _), _)| at.min(end));
    }
    first[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::super::reading::ReadingAttachment;
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
                kind: OutgoingKind::New,
                to: vec!["ana@xn--mnchen-3ya.de".into(), "Alex@Example.test".into()],
                cc: Vec::new(),
                bcc: vec!["alex@example.test".into()],
                subject: "Plans".into(),
                body: "Hi".into(),
                attachments: Vec::new(),
            }
        );
        // The same person in two fields is looked up once.
        assert_eq!(
            outgoing.recipients(),
            ["ana@xn--mnchen-3ya.de", "Alex@Example.test"]
        );
    }

    fn answering() -> Answerable {
        Answerable {
            message_id: "m1".into(),
            from: Some("alias@example.test".into()),
            subject: "Plans".into(),
            sender: "Alex Reed".into(),
            to: vec!["alex@example.test".into()],
            cc: vec!["sam@example.test".into()],
            redirected_from: None,
            opened: true,
            files: vec![ReadingAttachment {
                message_id: "m1".into(),
                id: "a1".into(),
                name: "plan.pdf".into(),
                size: 24_576,
            }],
        }
    }

    #[test]
    fn an_answer_opens_addressed_as_core_would_and_asks_no_subject() {
        let reply = Compose::answer("me@example.test".into(), AnswerKind::Reply, answering());
        assert_eq!(reply.from, "alias@example.test");
        assert_eq!(reply.to.text(), "alex@example.test");
        assert!(reply.cc.is_blank());
        assert_eq!(reply.focus, Field::Body);
        assert_eq!(reply.headers(), [Field::To, Field::Cc, Field::Bcc]);
        assert!(reply.untouched());

        let mut all = Compose::answer("me@example.test".into(), AnswerKind::ReplyAll, answering());
        assert_eq!(all.cc.text(), "sam@example.test");
        // Tab goes round the fields without a subject.
        let mut order = Vec::new();
        for _ in 0..4 {
            press(&mut all, KeyCode::Tab);
            order.push(all.focus);
        }
        assert_eq!(order, [Field::To, Field::Cc, Field::Bcc, Field::Body]);
        // Ctrl+Q leaves the original out, and puts it back.
        ctrl(&mut all, KeyCode::Char('q'));
        assert_eq!(all.kind.quote(), Quote::Omitted);
        ctrl(&mut all, KeyCode::Char('q'));
        assert_eq!(all.kind.quote(), Quote::Original);
        write(&mut all, "Count me in.");
        assert!(!all.untouched());
        let outgoing = all.ready().unwrap();
        assert_eq!(
            outgoing.kind,
            OutgoingKind::Reply {
                message_id: "m1".into(),
                everyone: true,
                quote: Quote::Original,
            }
        );
        assert_eq!(outgoing.subject, "");

        let forward = Compose::answer("me@example.test".into(), AnswerKind::Forward, answering());
        assert!(forward.to.is_blank() && forward.cc.is_blank());
        assert_eq!(forward.focus, Field::To);
        // An original that did not open has nothing to quote.
        let unopened = Answerable {
            opened: false,
            ..answering()
        };
        let mut reply = Compose::answer("me@example.test".into(), AnswerKind::Reply, unopened);
        ctrl(&mut reply, KeyCode::Char('q'));
        assert_eq!(reply.kind.quote(), Quote::Original);
    }

    #[test]
    fn ctrl_a_lists_the_files_to_attach_more_or_take_one_away() {
        let mut compose =
            Compose::answer("me@example.test".into(), AnswerKind::Forward, answering());
        assert_eq!(ctrl(&mut compose, KeyCode::Char('a')), Asked::Nothing);
        assert!(compose.files.is_some());
        // Keys type the path; Tab asks to complete it, Enter to attach it.
        write(&mut compose, "~/re");
        assert_eq!(
            press(&mut compose, KeyCode::Tab),
            Asked::Complete("~/re".into())
        );
        compose.completed("~/re", vec!["~/report.pdf".into(), "~/reports/".into()]);
        let files = compose.files.as_ref().unwrap();
        assert_eq!(files.path.text(), "~/report");
        assert_eq!(files.matches.len(), 2);
        // Completions for a path typed on since come too late.
        write(&mut compose, ".");
        compose.completed("~/report", vec!["~/report.pdf".into()]);
        assert_eq!(compose.files.as_ref().unwrap().path.text(), "~/report.");
        compose.completed("~/report.", vec!["~/report.pdf".into()]);
        assert_eq!(compose.files.as_ref().unwrap().path.text(), "~/report.pdf");
        assert_eq!(
            press(&mut compose, KeyCode::Enter),
            Asked::Attach("~/report.pdf".into())
        );
        compose.attached(Err("Cannot attach ~/report.pdf: gone".into()));
        assert_eq!(
            compose.files.as_ref().unwrap().notice.as_deref(),
            Some("Cannot attach ~/report.pdf: gone")
        );
        compose.attached(Ok((PathBuf::from("/home/me/report.pdf"), 1_536)));
        assert!(compose.files.as_ref().unwrap().path.is_blank());
        assert!(!compose.untouched());
        let listed = compose.listed();
        assert_eq!(
            listed
                .iter()
                .map(|file| (file.name.as_str(), file.original, file.left_out))
                .collect::<Vec<_>>(),
            [("report.pdf", false, false), ("plan.pdf", true, false)]
        );
        // Up chooses the last file; Delete leaves the original's out, then
        // puts it back; on an attached one, it takes it away.
        press(&mut compose, KeyCode::Up);
        assert_eq!(compose.files.as_ref().unwrap().chosen, Some(1));
        press(&mut compose, KeyCode::Delete);
        assert!(compose.listed()[1].left_out);
        press(&mut compose, KeyCode::Up);
        press(&mut compose, KeyCode::Delete);
        assert_eq!(compose.listed().len(), 1);
        assert_eq!(compose.files.as_ref().unwrap().chosen, Some(0));
        // Typing goes back to the path; Esc puts the list away.
        write(&mut compose, "x");
        assert_eq!(compose.files.as_ref().unwrap().chosen, None);
        press(&mut compose, KeyCode::Esc);
        assert!(compose.files.is_none());
        compose.to = Editor::with("sam@example.test");
        let outgoing = compose.ready().unwrap();
        assert!(outgoing.attachments.is_empty());
        assert_eq!(
            outgoing.kind,
            OutgoingKind::Forward {
                message_id: "m1".into(),
                quote: Quote::Original,
                leave_out: vec!["a1".into()],
            }
        );
    }

    #[test]
    fn a_paste_keeps_its_lines_in_the_text_and_joins_them_in_a_field() {
        let mut compose = Compose::new(String::new());
        compose.paste("alex@example.test\r\nsam@example.test\n");
        assert_eq!(compose.to.text(), "alex@example.test, sam@example.test, ");
        compose.focus = Field::Subject;
        compose.paste("Plans\nfor May");
        assert_eq!(compose.subject.text(), "Plans for May");
        compose.focus = Field::Body;
        compose.paste("Hi,\r\n\tsee you\rthere\u{1b}[2J.");
        assert_eq!(compose.body.text(), "Hi,\n    see you\nthere[2J.");
        // The path of a file to attach is one line.
        ctrl(&mut compose, KeyCode::Char('a'));
        compose.paste("~/notes\n.txt");
        assert_eq!(compose.files.as_ref().unwrap().path.text(), "~/notes.txt");
        // On its way, nothing changes it.
        compose.files = None;
        compose.sending = Sending::InFlight;
        compose.paste("more");
        assert_eq!(compose.body.text(), "Hi,\n    see you\nthere[2J.");
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
