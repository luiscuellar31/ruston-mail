//! Mailbox keyboard shortcuts. The UI only reports raw key presses.

use super::{App, Effects, Mailbox, Step, UiEffect};
use crate::settings::ComposePlacement;

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
    Trash,
    /// Fold the sidebar into its strip of icons, or unfold it.
    ToggleSidebar,
}

/// The meaning of a key press, or `None` when it is not a shortcut. Plain
/// keys carry no modifiers, so typing never moves the mailbox underneath.
fn shortcut(press: KeyPress) -> Option<Shortcut> {
    if press.command {
        return match press.key {
            Key::Character('r') => Some(Shortcut::Refresh),
            Key::Character('f') => Some(Shortcut::Search),
            Key::Character(',') => Some(Shortcut::ToggleSettings),
            Key::Character('n') => Some(Shortcut::Compose),
            Key::Character('a') => Some(Shortcut::SelectAll),
            Key::Enter => Some(Shortcut::Send),
            Key::Backspace | Key::Delete => Some(Shortcut::Trash),
            _ => None,
        };
    }
    if press.shift {
        return match press.key {
            Key::ArrowDown => Some(Shortcut::Extend(Step::Next)),
            Key::ArrowUp => Some(Shortcut::Extend(Step::Previous)),
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
        Key::Delete => Some(Shortcut::Trash),
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

    /// Whether this key leaves Settings: the one that backs out of what is
    /// on top, or the one that opens them. The window asks before leaving
    /// unapplied changes, and reads it here so that it asks for every key
    /// the mailbox takes to leave.
    pub fn leaves_settings(self) -> bool {
        matches!(
            shortcut(self),
            Some(Shortcut::Dismiss | Shortcut::ToggleSettings)
        )
    }

    /// Whether this key opens the selected conversation.
    pub fn opens_conversation(self) -> bool {
        shortcut(self) == Some(Shortcut::Open)
    }
}

impl App {
    /// Whether a draft fills the reading pane; a folded one leaves the reader
    /// in view.
    pub(super) fn draft_covers_reader(&self) -> bool {
        self.compose
            .as_ref()
            .is_some_and(|compose| !compose.folded())
            && self.settings.compose_placement == ComposePlacement::ReadingPane
    }

    /// A draft in its own window takes that window's Escape and Send keys;
    /// the keys handled here come from the mailbox window.
    pub(super) fn draft_in_window(&self) -> bool {
        self.compose.is_some() && self.settings.compose_placement == ComposePlacement::Window
    }

    pub(super) fn handle_key(&mut self, press: KeyPress) -> Effects {
        let Some(shortcut) = shortcut(press) else {
            return Effects::none();
        };
        // Settings blocks mailbox shortcuts except Escape, its toggle and the
        // sidebar's, which stays beside it; link prompts allow only Escape.
        let shortcut_blocked = (self.showing_settings
            && !matches!(shortcut, Shortcut::ToggleSettings | Shortcut::ToggleSidebar))
            || self.pending_link.is_some();
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
            // One layer at a time, starting with the most recent.
            Shortcut::Dismiss => {
                // The link prompt is a dialog over the whole window.
                if self.pending_link.take().is_some() {
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
            Shortcut::Trash => {
                // The conversation a folded draft leaves in view can go.
                if self.compose().is_some_and(|compose| !compose.folded()) {
                    return Effects::none();
                }
                let trash = crate::mail::Folder::System(crate::mail::MailFolder::Trash);
                if self.mailbox().is_some_and(|m| m.folder() == &trash) {
                    return Effects::none();
                }
                self.apply_action(crate::mail::MailAction::MoveTo(trash))
            }
            Shortcut::ToggleSidebar => {
                self.remember(|settings| settings.sidebar_folded = !settings.sidebar_folded);
                Effects::none()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_reads_what_a_key_means_from_the_same_table() {
        let (plain, command) = (KeyPress::plain, KeyPress::with_command);
        // What leaves Settings, which the window asks about first.
        assert!(plain(Key::Escape).leaves_settings());
        assert!(command(Key::Character(',')).leaves_settings());
        assert!(!plain(Key::Character(',')).leaves_settings());
        assert!(!command(Key::Escape).leaves_settings());
        // What opens a conversation, which a narrow window then shows.
        assert!(plain(Key::Enter).opens_conversation());
        assert!(!command(Key::Enter).opens_conversation());
        let held = KeyPress {
            other_modifier: true,
            ..plain(Key::Enter)
        };
        assert!(!held.opens_conversation());
        // Neither can drift from the mailbox: a key it takes to back out
        // leaves Settings, whichever key that comes to be.
        for key in [Key::Escape, Key::Enter, Key::Delete, Key::Character('j')] {
            for press in [plain(key), command(key)] {
                assert_eq!(
                    press.leaves_settings(),
                    matches!(
                        shortcut(press),
                        Some(Shortcut::Dismiss | Shortcut::ToggleSettings)
                    ),
                    "{press:?}"
                );
            }
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
        // Command and A choose every conversation listed; A alone is nothing.
        assert_eq!(
            shortcut(command(Key::Character('a'))),
            Some(Shortcut::SelectAll)
        );
        assert_eq!(shortcut(plain(Key::Character('a'))), None);
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
        assert_eq!(shortcut(command(Key::Backspace)), Some(Shortcut::Trash));
        assert_eq!(shortcut(command(Key::Delete)), Some(Shortcut::Trash));
        assert_eq!(shortcut(plain(Key::Delete)), Some(Shortcut::Trash));
        assert_eq!(shortcut(plain(Key::F9)), Some(Shortcut::ToggleSidebar));

        // A modifier turns a plain shortcut into somebody else's business.
        assert_eq!(shortcut(command(Key::Character('j'))), None);
        assert_eq!(shortcut(command(Key::ArrowDown)), None);
        assert_eq!(shortcut(plain(Key::Character('r'))), None);
        assert_eq!(shortcut(plain(Key::Character('z'))), None);
        assert_eq!(shortcut(plain(Key::Backspace)), None);
        assert_eq!(shortcut(command(Key::F9)), None);
        assert_eq!(shortcut(shift(Key::F9)), None);
    }
}
