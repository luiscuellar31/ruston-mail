//! What a key press means, and what the mailbox does about it.
//!
//! The keys themselves are read by `ui`, which says only which key was
//! pressed; deciding what it means belongs here, where the mailbox is.

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
}

/// The meaning of a key press, or `None` when it is not a shortcut. Plain
/// keys carry no modifiers, so typing never moves the mailbox underneath.
fn shortcut(press: KeyPress) -> Option<Shortcut> {
    if press.command {
        return match press.key {
            Key::Character('r') => Some(Shortcut::Refresh),
            Key::Character('f') => Some(Shortcut::Search),
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
        // Only the topmost thing on screen takes a key. The mailbox sits
        // under the settings page and under the link prompt, so while either
        // is up the one shortcut still worth anything backs out of it.
        let covered = self.showing_settings || self.pending_link.is_some();
        if covered && shortcut != Shortcut::Dismiss {
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
                // A message on its way is not dismissible; the rest steps
                // back one layer at a time.
                if self.compose().is_some_and(|writing| !writing.in_flight()) {
                    self.close_compose();
                    return Effects::none();
                }
                if self.showing_settings {
                    self.showing_settings = false;
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

        // A modifier turns a plain shortcut into somebody else's business.
        assert_eq!(shortcut(command(Key::Character('j'))), None);
        assert_eq!(shortcut(command(Key::ArrowDown)), None);
        assert_eq!(shortcut(plain(Key::Character('r'))), None);
        assert_eq!(shortcut(plain(Key::Character('z'))), None);
    }
}
