//! Mailbox keyboard shortcuts. The UI only reports raw key presses.

use super::{AnswerKind, App, Effects, Mailbox, ReaderState, SettingsSection, Step, UiEffect};
use crate::mail::{Folder, ListFilter, MailAction, MailFolder};

/// What a key press means to the mailbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shortcut {
    /// Open the conversation one step away in the list.
    Move(Step),
    /// Move the far end of the rows chosen one step, as Shift and an arrow do.
    Extend(Step),
    /// Choose every conversation listed.
    SelectAll,
    /// Open the selected conversation, which also retries a failed one.
    Open,
    /// Back out of the topmost thing on screen.
    Dismiss,
    Refresh,
    Search,
    ToggleSettings,
    Compose,
    Send,
    /// Move the conversations acted on to one of Proton's folders.
    MoveTo(MailFolder),
    /// Mark the conversations acted on as read, or reply to the open one
    /// when they are read already.
    ReadOrReply,
    MarkUnread,
    /// Star the conversations acted on, or take the star from them when
    /// every one has it.
    Star,
    /// Answer the newest message of the open conversation.
    Answer(AnswerKind),
    /// List only the conversations a filter keeps.
    Show(ListFilter),
    /// Take back the move the list still offers to undo.
    Undo,
    /// Show the list of these keys.
    Help,
    /// Fold the sidebar into its strip of icons, or unfold it.
    ToggleSidebar,
}

/// The meaning of a key press, or `None` when it is not a shortcut. The
/// letters alone are Proton Mail's own, which the terminal interface takes
/// too; the window sends one only while no text field has the keyboard.
fn shortcut(press: KeyPress) -> Option<Shortcut> {
    if press.command {
        return match press.key {
            Key::Character('r') => Some(Shortcut::Refresh),
            Key::Character('f') => Some(Shortcut::Search),
            Key::Character(',') => Some(Shortcut::ToggleSettings),
            Key::Character('n') => Some(Shortcut::Compose),
            Key::Character('a') => Some(Shortcut::SelectAll),
            Key::Character('z') => Some(Shortcut::Undo),
            Key::Enter => Some(Shortcut::Send),
            Key::Backspace | Key::Delete => Some(Shortcut::MoveTo(MailFolder::Trash)),
            _ => None,
        };
    }
    if press.shift {
        return match press.key {
            Key::ArrowDown => Some(Shortcut::Extend(Step::Next)),
            Key::ArrowUp => Some(Shortcut::Extend(Step::Previous)),
            Key::Character('r') => Some(Shortcut::Answer(AnswerKind::ReplyAll)),
            Key::Character('f') => Some(Shortcut::Answer(AnswerKind::Forward)),
            Key::Character('u') => Some(Shortcut::Show(ListFilter::Unread)),
            Key::Character('a') => Some(Shortcut::Show(ListFilter::All)),
            _ => None,
        };
    }
    if press.other_modifier {
        return None;
    }

    match press.key {
        Key::ArrowDown | Key::Character('j') => Some(Shortcut::Move(Step::Next)),
        Key::ArrowUp | Key::Character('k') => Some(Shortcut::Move(Step::Previous)),
        Key::Enter => Some(Shortcut::Open),
        Key::Escape => Some(Shortcut::Dismiss),
        Key::Character('n') => Some(Shortcut::Compose),
        Key::Character('/') => Some(Shortcut::Search),
        Key::Character('r') => Some(Shortcut::ReadOrReply),
        Key::Character('u') => Some(Shortcut::MarkUnread),
        Key::Character('*') => Some(Shortcut::Star),
        Key::Character('i') => Some(Shortcut::MoveTo(MailFolder::Inbox)),
        Key::Character('a') => Some(Shortcut::MoveTo(MailFolder::Archive)),
        Key::Character('s') => Some(Shortcut::MoveTo(MailFolder::Spam)),
        Key::Delete | Key::Character('t') => Some(Shortcut::MoveTo(MailFolder::Trash)),
        Key::Character('?') => Some(Shortcut::Help),
        Key::F9 => Some(Shortcut::ToggleSidebar),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Character(char),
    ArrowDown,
    ArrowUp,
    Enter,
    Escape,
    Backspace,
    Delete,
    F9,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyPress {
    pub key: Key,
    pub command: bool,
    /// Shift held alone, which chooses rows along the list.
    pub shift: bool,
    pub other_modifier: bool,
}

impl KeyPress {
    /// A key pressed alone.
    pub fn plain(key: Key) -> Self {
        Self {
            key,
            command: false,
            shift: false,
            other_modifier: false,
        }
    }

    /// A key pressed with Shift alone.
    pub fn with_shift(key: Key) -> Self {
        Self {
            shift: true,
            ..Self::plain(key)
        }
    }

    /// A key pressed with Command on macOS, or Ctrl elsewhere.
    pub fn with_command(key: Key) -> Self {
        Self {
            command: true,
            ..Self::plain(key)
        }
    }

    /// Whether this key opens the selected conversation.
    pub fn opens_conversation(self) -> bool {
        shortcut(self) == Some(Shortcut::Open)
    }
}

impl App {
    pub(super) fn handle_key(&mut self, press: KeyPress) -> Effects {
        let Some(shortcut) = shortcut(press) else {
            return Effects::none();
        };
        // Settings blocks mailbox shortcuts except Escape, its toggle and the
        // sidebar's, which stays beside it; link prompts allow only Escape.
        let shortcut_blocked = (self.showing_settings
            && !matches!(shortcut, Shortcut::ToggleSettings | Shortcut::ToggleSidebar))
            || self.session.pending_link.is_some();
        if shortcut_blocked && shortcut != Shortcut::Dismiss {
            return Effects::none();
        }

        // The draft covers the reader, so a step through the list would open
        // a conversation, or choose rows, nobody sees.
        if matches!(
            shortcut,
            Shortcut::Move(_) | Shortcut::Extend(_) | Shortcut::SelectAll | Shortcut::Open
        ) && self.draft_covers_reader()
        {
            return Effects::none();
        }

        // A draft being written keeps the conversation behind it as it is; the
        // one a folded draft leaves in view can change.
        let acts_on_rows = matches!(
            shortcut,
            Shortcut::MoveTo(_)
                | Shortcut::ReadOrReply
                | Shortcut::MarkUnread
                | Shortcut::Star
                | Shortcut::Answer(_)
        );
        if acts_on_rows && self.compose().is_some_and(|compose| !compose.folded()) {
            return Effects::none();
        }

        match shortcut {
            Shortcut::Move(step) => {
                let Some(next) = self
                    .active_mailbox()
                    .and_then(|mailbox| mailbox.neighbour(step))
                else {
                    return Effects::none();
                };

                self.open_row(next)
            }
            Shortcut::Extend(step) => {
                let Some(mailbox) = self.active_mailbox() else {
                    return Effects::none();
                };
                if let Some(alone) = mailbox.extend_selection(step) {
                    return self.open_row(alone);
                }
                // The far end can move past the rows on screen.
                let end = mailbox.selection_end().map(str::to_owned);
                end.map_or_else(Effects::none, |end| self.reveal_in_list(&end))
            }
            Shortcut::SelectAll => match self.active_mailbox().and_then(Mailbox::select_all) {
                Some(alone) => self.select_conversation(alone),
                None => Effects::none(),
            },
            Shortcut::Open => {
                let Some(selected) = self
                    .mailbox
                    .as_ref()
                    .and_then(|mailbox| mailbox.selected_conversation())
                    .map(str::to_owned)
                else {
                    return Effects::none();
                };

                self.select_conversation(selected)
            }
            Shortcut::Dismiss => self.dismiss(),
            Shortcut::Refresh => self.refresh_mailbox(),
            Shortcut::Search => Effects::ui(UiEffect::FocusSearch),
            Shortcut::ToggleSettings => {
                self.showing_settings = !self.showing_settings;
                Effects::none()
            }
            Shortcut::Compose => self.open_compose(),
            Shortcut::Send => {
                if self.draft_covers_reader() {
                    return self.send_compose();
                }
                Effects::none()
            }
            Shortcut::MoveTo(folder) => self.move_by_key(folder),
            Shortcut::ReadOrReply => {
                if self.rows_acted_on().iter().any(|row| row.unread) {
                    self.apply_action(MailAction::SetUnread(false))
                } else {
                    self.answer_open(AnswerKind::Reply)
                }
            }
            Shortcut::MarkUnread => self.apply_action(MailAction::SetUnread(true)),
            Shortcut::Star => {
                let starred = self.rows_acted_on().iter().all(|row| row.starred);
                self.apply_action(MailAction::SetStarred(!starred))
            }
            Shortcut::Answer(kind) => self.answer_open(kind),
            Shortcut::Show(filter) => self.show_only(filter),
            Shortcut::Undo => self.undo_move(),
            Shortcut::Help => {
                self.showing_settings = true;
                self.settings_section = SettingsSection::Keyboard;
                Effects::none()
            }
            Shortcut::ToggleSidebar => {
                self.remember(|settings| settings.sidebar_folded = !settings.sidebar_folded);
                Effects::none()
            }
        }
    }

    /// The rows a key acts on: those chosen together, or else the open one.
    fn rows_acted_on(&self) -> Vec<&crate::mail::ConversationSummary> {
        self.mailbox()
            .map(Mailbox::summaries_acted_on)
            .unwrap_or_default()
    }

    /// Moves the rows acted on to `folder`, as the reader's buttons would:
    /// none that is already there, and none of the account's own mail to the
    /// Inbox, which Proton keeps out of it.
    fn move_by_key(&mut self, folder: MailFolder) -> Effects {
        let destination = Folder::System(folder);
        let Some(mailbox) = self.mailbox() else {
            return Effects::none();
        };
        let own_mail = matches!(
            mailbox.folder().system(),
            Some(MailFolder::Sent | MailFolder::Drafts)
        );
        if mailbox.folder() == &destination || (folder == MailFolder::Inbox && own_mail) {
            return Effects::none();
        }
        let rows = mailbox
            .rows_acted_on()
            .into_iter()
            .filter(|row| mailbox.location_of(row) != Some(&destination))
            .collect();
        self.apply_to_rows(rows, MailAction::MoveTo(destination))
    }

    /// Answers the newest message of the conversation the reader shows.
    fn answer_open(&mut self, kind: AnswerKind) -> Effects {
        let Some(mailbox) = self.mailbox() else {
            return Effects::none();
        };
        // Rows chosen together take the reader's place.
        if !mailbox.selected_rows().is_empty() {
            return Effects::none();
        }
        let ReaderState::Loaded(reader) = mailbox.reader_state() else {
            return Effects::none();
        };
        let Some(newest) = reader.detail().messages.last() else {
            return Effects::none();
        };
        let newest = newest.id.clone();
        self.answer(&newest, kind)
    }

    /// What Escape does: it takes away one layer at a time, starting with the
    /// most recent.
    fn dismiss(&mut self) -> Effects {
        // The link prompt is a dialog over the whole window.
        if self.session.pending_link.take().is_some() {
            return Effects::none();
        }
        // Leave settings before touching a message still open behind it.
        if self.showing_settings {
            self.showing_settings = false;
            return Effects::none();
        }
        // In-flight messages cannot be dismissed, and a folded draft
        // leaves the key to the reader it shows.
        if self.draft_covers_reader()
            && let Some(compose) = self.compose().filter(|c| !c.in_flight())
        {
            if compose.confirming_discard() {
                self.cancel_discard();
            } else {
                self.close_compose();
            }
            return Effects::none();
        }
        if let Some(mailbox) = self.active_mailbox() {
            if mailbox.clear_selection() {
                return Effects::none();
            }
            if mailbox.has_search_text() {
                mailbox.set_search_query(String::new());
            } else {
                mailbox.close_reader();
            }
        }

        Effects::none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_reads_what_a_key_means_from_the_same_table() {
        let (plain, command) = (KeyPress::plain, KeyPress::with_command);
        // What opens a conversation, which a narrow window then shows.
        assert!(plain(Key::Enter).opens_conversation());
        assert!(!command(Key::Enter).opens_conversation());
        let held = KeyPress {
            other_modifier: true,
            ..plain(Key::Enter)
        };
        assert!(!held.opens_conversation());
    }

    #[test]
    fn protons_single_keys_are_shortcuts() {
        let key = |letter| shortcut(KeyPress::plain(Key::Character(letter)));
        let shifted = |letter| shortcut(KeyPress::with_shift(Key::Character(letter)));
        assert_eq!(key('n'), Some(Shortcut::Compose));
        assert_eq!(key('/'), Some(Shortcut::Search));
        assert_eq!(key('r'), Some(Shortcut::ReadOrReply));
        assert_eq!(key('u'), Some(Shortcut::MarkUnread));
        assert_eq!(key('*'), Some(Shortcut::Star));
        assert_eq!(key('?'), Some(Shortcut::Help));
        for (letter, folder) in [
            ('i', MailFolder::Inbox),
            ('a', MailFolder::Archive),
            ('s', MailFolder::Spam),
            ('t', MailFolder::Trash),
        ] {
            assert_eq!(key(letter), Some(Shortcut::MoveTo(folder)), "{letter}");
        }
        assert_eq!(shifted('r'), Some(Shortcut::Answer(AnswerKind::ReplyAll)));
        assert_eq!(shifted('f'), Some(Shortcut::Answer(AnswerKind::Forward)));
        assert_eq!(shifted('u'), Some(Shortcut::Show(ListFilter::Unread)));
        assert_eq!(shifted('a'), Some(Shortcut::Show(ListFilter::All)));
        // Proton's keys Ruston Mail has nothing for.
        for letter in ['g', 'm', 'l', 'x', 'o', 'f'] {
            assert_eq!(key(letter), None, "{letter}");
        }
    }

    #[test]
    fn keys_map_to_mailbox_shortcuts() {
        let plain = KeyPress::plain;
        let command = KeyPress::with_command;

        assert_eq!(
            shortcut(plain(Key::Character('j'))),
            Some(Shortcut::Move(Step::Next))
        );
        assert_eq!(
            shortcut(plain(Key::ArrowDown)),
            Some(Shortcut::Move(Step::Next))
        );
        assert_eq!(
            shortcut(plain(Key::Character('k'))),
            Some(Shortcut::Move(Step::Previous))
        );
        assert_eq!(shortcut(plain(Key::Enter)), Some(Shortcut::Open));
        assert_eq!(shortcut(plain(Key::Escape)), Some(Shortcut::Dismiss));
        // Shift and an arrow choose rows along the list; with other keys,
        // Shift means nothing to the mailbox.
        let shift = KeyPress::with_shift;
        assert_eq!(
            shortcut(shift(Key::ArrowDown)),
            Some(Shortcut::Extend(Step::Next))
        );
        assert_eq!(
            shortcut(shift(Key::ArrowUp)),
            Some(Shortcut::Extend(Step::Previous))
        );
        for key in [Key::Character('j'), Key::Enter, Key::Escape, Key::Delete] {
            assert_eq!(shortcut(shift(key)), None, "{key:?}");
        }
        // Command and A choose every conversation listed.
        assert_eq!(
            shortcut(command(Key::Character('a'))),
            Some(Shortcut::SelectAll)
        );
        assert_eq!(
            shortcut(command(Key::Character('r'))),
            Some(Shortcut::Refresh)
        );
        assert_eq!(
            shortcut(command(Key::Character('f'))),
            Some(Shortcut::Search)
        );
        assert_eq!(
            shortcut(command(Key::Character(','))),
            Some(Shortcut::ToggleSettings)
        );
        assert_eq!(
            shortcut(command(Key::Character('n'))),
            Some(Shortcut::Compose)
        );
        assert_eq!(shortcut(command(Key::Enter)), Some(Shortcut::Send));
        let trash = Some(Shortcut::MoveTo(MailFolder::Trash));
        assert_eq!(shortcut(command(Key::Backspace)), trash);
        assert_eq!(shortcut(command(Key::Delete)), trash);
        assert_eq!(shortcut(plain(Key::Delete)), trash);
        assert_eq!(shortcut(command(Key::Character('z'))), Some(Shortcut::Undo));
        assert_eq!(shortcut(plain(Key::F9)), Some(Shortcut::ToggleSidebar));

        // A modifier turns a plain shortcut into somebody else's business.
        assert_eq!(shortcut(command(Key::Character('j'))), None);
        assert_eq!(shortcut(command(Key::ArrowDown)), None);
        assert_eq!(shortcut(plain(Key::Character('z'))), None);
        assert_eq!(shortcut(plain(Key::Backspace)), None);
        assert_eq!(shortcut(command(Key::F9)), None);
        assert_eq!(shortcut(shift(Key::F9)), None);
    }
}
