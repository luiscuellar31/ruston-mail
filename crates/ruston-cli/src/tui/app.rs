//! The interface's state and what each key does to it, apart from drawing
//! and from the terminal, so both can be tested without one.

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

/// The three panes, in the order Tab visits them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pane {
    Folders,
    Conversations,
    Reader,
}

impl Pane {
    pub(crate) const ALL: [Self; 3] = [Self::Folders, Self::Conversations, Self::Reader];

    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Folders => "Folders",
            Self::Conversations => "Conversations",
            Self::Reader => "Reader",
        }
    }

    fn next(self) -> Self {
        match self {
            Self::Folders => Self::Conversations,
            Self::Conversations => Self::Reader,
            Self::Reader => Self::Folders,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::Folders => Self::Reader,
            Self::Conversations => Self::Folders,
            Self::Reader => Self::Conversations,
        }
    }
}

/// What the interface shows and where the keys go.
pub(crate) struct App {
    /// Whose mail this is, for the status bar.
    pub(crate) account: String,
    /// The pane the keys act on.
    pub(crate) focus: Pane,
    /// Whether the list of keys is open over the panes.
    pub(crate) help: bool,
    /// Set once the person asks to quit; the loop then ends.
    pub(crate) quit: bool,
}

impl App {
    pub(crate) fn new(account: String) -> Self {
        Self {
            account,
            focus: Pane::Folders,
            help: false,
            quit: false,
        }
    }

    pub(crate) fn update(&mut self, message: Message) {
        match message {
            Message::Quit => self.quit = true,
            Message::ToggleHelp => self.help = !self.help,
            Message::CloseHelp => self.help = false,
            // The open list of keys covers the panes, so focus stays put.
            Message::NextPane if !self.help => self.focus = self.focus.next(),
            Message::PreviousPane if !self.help => self.focus = self.focus.previous(),
            Message::NextPane | Message::PreviousPane => {}
        }
    }
}

/// Something the person asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Message {
    NextPane,
    PreviousPane,
    ToggleHelp,
    CloseHelp,
    Quit,
}

impl Message {
    /// What a terminal event asks for, if anything. Windows reports a key's
    /// release as well as its press, so a release asks for nothing, or every
    /// key would act twice there.
    pub(crate) fn from_event(event: &Event) -> Option<Self> {
        let Event::Key(key) = event else {
            return None;
        };
        if key.kind == KeyEventKind::Release {
            return None;
        }
        Self::from_key(key)
    }

    fn from_key(key: &KeyEvent) -> Option<Self> {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return (key.code == KeyCode::Char('c')).then_some(Self::Quit);
        }
        match key.code {
            KeyCode::Char('q') => Some(Self::Quit),
            KeyCode::Char('?') => Some(Self::ToggleHelp),
            KeyCode::Esc => Some(Self::CloseHelp),
            KeyCode::Tab => Some(Self::NextPane),
            KeyCode::BackTab => Some(Self::PreviousPane),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn press(app: &mut App, code: KeyCode) {
        if let Some(message) = Message::from_event(&key(code)) {
            app.update(message);
        }
    }

    #[test]
    fn tab_goes_through_the_panes_and_shift_tab_comes_back() {
        let mut app = App::new("demo".into());
        let mut seen = Vec::new();
        for _ in 0..3 {
            seen.push(app.focus);
            press(&mut app, KeyCode::Tab);
        }
        assert_eq!(seen, Pane::ALL);
        assert_eq!(app.focus, Pane::Folders);
        press(&mut app, KeyCode::BackTab);
        assert_eq!(app.focus, Pane::Reader);
    }

    #[test]
    fn a_released_key_does_nothing() {
        let mut release = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        assert_eq!(Message::from_event(&Event::Key(release)), None);
        // A press, and a held key's repeat, still count.
        release.kind = KeyEventKind::Repeat;
        assert_eq!(
            Message::from_event(&Event::Key(release)),
            Some(Message::Quit)
        );
    }

    #[test]
    fn q_and_ctrl_c_quit() {
        let mut app = App::new("demo".into());
        press(&mut app, KeyCode::Char('q'));
        assert!(app.quit);

        let ctrl_c = Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert_eq!(Message::from_event(&ctrl_c), Some(Message::Quit));
        // Other control keys ask for nothing yet, Ctrl+Q included.
        let ctrl_q = Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL));
        assert_eq!(Message::from_event(&ctrl_q), None);
    }

    #[test]
    fn the_keys_open_with_a_question_mark_and_hold_the_focus_while_open() {
        let mut app = App::new("demo".into());
        press(&mut app, KeyCode::Char('?'));
        assert!(app.help);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.focus, Pane::Folders);
        press(&mut app, KeyCode::Esc);
        assert!(!app.help);
        press(&mut app, KeyCode::Char('?'));
        press(&mut app, KeyCode::Char('?'));
        assert!(!app.help);
    }

    #[test]
    fn a_resize_asks_for_nothing() {
        assert_eq!(Message::from_event(&Event::Resize(80, 24)), None);
    }
}
