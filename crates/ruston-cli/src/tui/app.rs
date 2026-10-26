//! The interface's state and what each key or answer does to it, apart from
//! drawing and from the terminal, so both can be tested without one. What
//! must happen elsewhere, such as loading a page, comes back as an [`Effect`].

use crate::mailbox::Place;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ruston_core::Conversation;
use ruston_core::model::enums::label_ids;

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
    /// The folders and labels, once they have loaded.
    pub(crate) places: Vec<Place>,
    /// Which of `places` the folder list's selection is on.
    pub(crate) selected_place: usize,
    /// The name of the place whose conversations the list shows.
    pub(crate) listed: String,
    /// Its first page of conversations, newest first.
    pub(crate) conversations: Vec<Conversation>,
    /// How many conversations it has in all.
    pub(crate) total: u32,
    /// Which conversation the list's selection is on.
    pub(crate) selected_conversation: usize,
    /// What is loading, or what went wrong, for the status bar.
    pub(crate) notice: Option<String>,
    /// The last list asked for; an answer to any other comes too late.
    list_request: u64,
}

/// Something the person asked for, or an answer to an [`Effect`].
#[derive(Debug, Clone)]
pub(crate) enum Message {
    NextPane,
    PreviousPane,
    Up,
    Down,
    Open,
    ToggleHelp,
    CloseHelp,
    Quit,
    PlacesLoaded(Result<Vec<Place>, String>),
    ConversationsLoaded {
        request: u64,
        result: Result<(u32, Vec<Conversation>), String>,
    },
}

/// Work the interface hands out, whose answer comes back as a [`Message`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Effect {
    LoadPlaces,
    LoadConversations { request: u64, place: String },
}

impl App {
    pub(crate) fn new(account: String) -> Self {
        Self {
            account,
            focus: Pane::Folders,
            help: false,
            quit: false,
            places: Vec::new(),
            selected_place: 0,
            listed: "Inbox".to_owned(),
            conversations: Vec::new(),
            total: 0,
            selected_conversation: 0,
            notice: Some("Loading…".to_owned()),
            list_request: 0,
        }
    }

    /// What to load first: the folders, and the Inbox meanwhile, since it
    /// is the first of them.
    pub(crate) fn start(&mut self) -> Vec<Effect> {
        vec![
            Effect::LoadPlaces,
            self.list(label_ids::INBOX.to_owned(), "Inbox".to_owned()),
        ]
    }

    pub(crate) fn update(&mut self, message: Message) -> Vec<Effect> {
        match message {
            Message::Quit => self.quit = true,
            Message::ToggleHelp => self.help = !self.help,
            Message::CloseHelp => self.help = false,
            // The open list of keys covers the panes, so they keep still.
            _ if self.help && is_key(&message) => {}
            Message::NextPane => self.focus = self.focus.next(),
            Message::PreviousPane => self.focus = self.focus.previous(),
            Message::Up => self.step(-1),
            Message::Down => self.step(1),
            Message::Open => return self.open(),
            Message::PlacesLoaded(Ok(places)) => {
                self.places = places;
                self.selected_place = 0;
            }
            Message::PlacesLoaded(Err(error)) => {
                self.notice = Some(format!("Cannot load the folders: {error}"));
            }
            Message::ConversationsLoaded { request, .. } if request != self.list_request => {}
            Message::ConversationsLoaded { result, .. } => match result {
                Ok((total, conversations)) => {
                    self.total = total;
                    self.conversations = conversations;
                    self.selected_conversation = 0;
                    self.notice = None;
                }
                Err(error) => {
                    self.notice = Some(format!("Cannot load {}: {error}", self.listed));
                }
            },
        }
        Vec::new()
    }

    /// Moves the focused list's selection `by` rows, within the list.
    fn step(&mut self, by: isize) {
        let (selected, len) = match self.focus {
            Pane::Folders => (&mut self.selected_place, self.places.len()),
            Pane::Conversations => (&mut self.selected_conversation, self.conversations.len()),
            Pane::Reader => return,
        };
        *selected = selected
            .saturating_add_signed(by)
            .min(len.saturating_sub(1));
    }

    /// Opens what the focused list's selection is on: a folder's
    /// conversations, shown next.
    fn open(&mut self) -> Vec<Effect> {
        match self.focus {
            Pane::Folders => {
                let Some(place) = self.places.get(self.selected_place) else {
                    return Vec::new();
                };
                let (id, name) = (place.id.clone(), place.name.clone());
                self.focus = Pane::Conversations;
                vec![self.list(id, name)]
            }
            Pane::Conversations | Pane::Reader => Vec::new(),
        }
    }

    /// Asks for the first page of the place `id`, named `name`, making any
    /// answer still on its way for another one too late.
    fn list(&mut self, id: String, name: String) -> Effect {
        self.list_request += 1;
        self.notice = Some(format!("Loading {name}…"));
        self.listed = name;
        Effect::LoadConversations {
            request: self.list_request,
            place: id,
        }
    }
}

/// Whether `message` came from a key, not from an answer.
fn is_key(message: &Message) -> bool {
    !matches!(
        message,
        Message::PlacesLoaded(_) | Message::ConversationsLoaded { .. }
    )
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
            KeyCode::Up | KeyCode::Char('k') => Some(Self::Up),
            KeyCode::Down | KeyCode::Char('j') => Some(Self::Down),
            KeyCode::Enter => Some(Self::Open),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mailbox::PlaceKind;

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn press(app: &mut App, code: KeyCode) -> Vec<Effect> {
        Message::from_event(&key(code)).map_or_else(Vec::new, |message| app.update(message))
    }

    fn place(id: &str, name: &str) -> Place {
        Place {
            id: id.into(),
            name: name.into(),
            depth: 0,
            kind: PlaceKind::System,
            unread: 0,
        }
    }

    fn conversation(id: &str) -> Conversation {
        Conversation {
            id: id.into(),
            ..Conversation::default()
        }
    }

    /// An app whose folders loaded, with the Inbox listed.
    fn loaded() -> App {
        let mut app = App::new("demo".into());
        let start = app.start();
        let Effect::LoadConversations { request, .. } = start[1].clone() else {
            panic!("{start:?}");
        };
        app.update(Message::PlacesLoaded(Ok(vec![
            place(label_ids::INBOX, "Inbox"),
            place("folder-1", "Work"),
        ])));
        app.update(Message::ConversationsLoaded {
            request,
            result: Ok((2, vec![conversation("a"), conversation("b")])),
        });
        app
    }

    #[test]
    fn it_loads_the_folders_and_the_inbox_at_once() {
        let mut app = App::new("demo".into());
        assert_eq!(
            app.start(),
            [
                Effect::LoadPlaces,
                Effect::LoadConversations {
                    request: 1,
                    place: label_ids::INBOX.into()
                }
            ]
        );
        let app = loaded();
        assert_eq!(app.places.len(), 2);
        assert_eq!(app.total, 2);
        assert_eq!(app.notice, None);
    }

    #[test]
    fn enter_on_a_folder_lists_it_and_moves_to_its_conversations() {
        let mut app = loaded();
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.selected_place, 1);
        // The selection stops at the last folder.
        press(&mut app, KeyCode::Down);
        assert_eq!(app.selected_place, 1);

        let effects = press(&mut app, KeyCode::Enter);
        assert_eq!(
            effects,
            [Effect::LoadConversations {
                request: 2,
                place: "folder-1".into()
            }]
        );
        assert_eq!(app.focus, Pane::Conversations);
        assert_eq!(app.listed, "Work");
        assert_eq!(app.notice.as_deref(), Some("Loading Work…"));
    }

    #[test]
    fn an_answer_for_a_list_asked_for_before_comes_too_late() {
        let mut app = loaded();
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        // The Inbox's answer, asked for first, arrives after Work was asked for.
        app.update(Message::ConversationsLoaded {
            request: 1,
            result: Ok((1, vec![conversation("old")])),
        });
        assert_eq!(app.total, 2);
        app.update(Message::ConversationsLoaded {
            request: 2,
            result: Ok((1, vec![conversation("work")])),
        });
        assert_eq!(app.conversations[0].id, "work");
        assert_eq!(app.selected_conversation, 0);
    }

    #[test]
    fn a_failed_load_says_so_and_keeps_what_was_shown() {
        let mut app = loaded();
        press(&mut app, KeyCode::Enter);
        app.update(Message::ConversationsLoaded {
            request: 2,
            result: Err("offline".into()),
        });
        assert_eq!(app.notice.as_deref(), Some("Cannot load Inbox: offline"));
        assert_eq!(app.conversations.len(), 2);
        app.update(Message::PlacesLoaded(Err("refused".into())));
        assert_eq!(
            app.notice.as_deref(),
            Some("Cannot load the folders: refused")
        );
    }

    #[test]
    fn up_and_down_move_the_focused_list_only() {
        let mut app = loaded();
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Down);
        assert_eq!((app.selected_place, app.selected_conversation), (0, 1));
        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Char('k'));
        assert_eq!(app.selected_conversation, 0);
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
        assert!(Message::from_event(&Event::Key(release)).is_none());
        // A press, and a held key's repeat, still count.
        release.kind = KeyEventKind::Repeat;
        assert!(matches!(
            Message::from_event(&Event::Key(release)),
            Some(Message::Quit)
        ));
    }

    #[test]
    fn q_and_ctrl_c_quit() {
        let mut app = App::new("demo".into());
        press(&mut app, KeyCode::Char('q'));
        assert!(app.quit);

        let ctrl_c = Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(matches!(Message::from_event(&ctrl_c), Some(Message::Quit)));
        // Other control keys ask for nothing yet, Ctrl+Q included.
        let ctrl_q = Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL));
        assert!(Message::from_event(&ctrl_q).is_none());
    }

    #[test]
    fn the_keys_open_with_a_question_mark_and_hold_everything_still_while_open() {
        let mut app = loaded();
        press(&mut app, KeyCode::Char('?'));
        assert!(app.help);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Down);
        assert!(press(&mut app, KeyCode::Enter).is_empty());
        assert_eq!((app.focus, app.selected_place), (Pane::Folders, 0));
        // Answers still land while it is open.
        app.update(Message::PlacesLoaded(Ok(vec![place("x", "X")])));
        assert_eq!(app.places.len(), 1);
        press(&mut app, KeyCode::Esc);
        assert!(!app.help);
        press(&mut app, KeyCode::Char('?'));
        press(&mut app, KeyCode::Char('?'));
        assert!(!app.help);
    }

    #[test]
    fn a_resize_asks_for_nothing() {
        assert!(Message::from_event(&Event::Resize(80, 24)).is_none());
    }
}
