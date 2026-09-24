//! Mailbox keyboard shortcuts. The UI only reports raw key presses.

use super::{App, Effects, Step, UiEffect};

/// What a key press means to the mailbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shortcut {
    /// Open the conversation one step away in the list.
    Move(Step),
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
            Key::Enter => Some(Shortcut::Send),
            Key::Backspace | Key::Delete => Some(Shortcut::Trash),
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyPress {
    pub key: Key,
    pub command: bool,
    pub other_modifier: bool,
}

impl App {
    pub(super) fn handle_key(&mut self, press: KeyPress) -> Effects {
        let Some(shortcut) = shortcut(press) else {
            return Effects::none();
        };
        // Modal UI blocks mailbox shortcuts except Escape and toggling settings.
        let shortcut_blocked = (self.showing_settings && shortcut != Shortcut::ToggleSettings)
            || self.pending_link.is_some();
        if shortcut_blocked && shortcut != Shortcut::Dismiss {
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

                Effects::batch([self.reveal_in_list(&next), self.select_conversation(next)])
            }
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
                // Leave settings before touching a message still open in the
                // mailbox window.
                if self.showing_settings {
                    self.showing_settings = false;
                    return Effects::none();
                }
                // In-flight messages cannot be dismissed.
                if let Some(compose) = self.compose().filter(|c| !c.in_flight()) {
                    if compose.confirming_discard() {
                        self.cancel_discard();
                    } else {
                        self.close_compose();
                    }
                    return Effects::none();
                }
                if self.pending_link.take().is_some() {
                    return Effects::none();
                }
                if let Some(mailbox) = self.active_mailbox() {
                    if mailbox.is_searching() {
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
            Shortcut::Compose => {
                self.open_compose();
                Effects::none()
            }
            Shortcut::Send => {
                if self.compose.is_some() {
                    return self.send_compose();
                }
                Effects::none()
            }
            Shortcut::Trash => {
                if self.compose.is_some() {
                    return Effects::none();
                }
                let trash = crate::mail::Folder::System(crate::mail::MailFolder::Trash);
                if self.mailbox().is_some_and(|m| m.folder() == &trash) {
                    return Effects::none();
                }
                self.apply_action(crate::mail::MailAction::MoveTo(trash))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_map_to_mailbox_shortcuts() {
        let plain = |key| KeyPress {
            key,
            command: false,
            other_modifier: false,
        };
        let command = |key| KeyPress {
            key,
            command: true,
            other_modifier: false,
        };

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

        // A modifier turns a plain shortcut into somebody else's business.
        assert_eq!(shortcut(command(Key::Character('j'))), None);
        assert_eq!(shortcut(command(Key::ArrowDown)), None);
        assert_eq!(shortcut(plain(Key::Character('r'))), None);
        assert_eq!(shortcut(plain(Key::Character('z'))), None);
        assert_eq!(shortcut(plain(Key::Backspace)), None);
    }
}
