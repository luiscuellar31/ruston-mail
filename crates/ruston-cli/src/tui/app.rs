//! The interface's state and what each key or answer does to it, apart from
//! drawing and from the terminal, so both can be tested without one. What
//! must happen elsewhere, such as loading a page, comes back as an [`Effect`].

use super::reading::Reading;
use crate::mailbox::Place;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ruston_core::Conversation;
use ruston_core::model::enums::label_ids;
use std::cell::Cell;

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
    /// Its label ID, to list it again.
    listed_id: String,
    /// Its first page of conversations, newest first.
    pub(crate) conversations: Vec<Conversation>,
    /// How many conversations it has in all.
    pub(crate) total: u32,
    /// Which conversation the list's selection is on.
    pub(crate) selected_conversation: usize,
    /// The conversation the reader shows, once it has loaded.
    pub(crate) reading: Option<Reading>,
    /// How many rows down the reader is scrolled.
    pub(crate) scroll: u16,
    /// The furthest the reader could scroll when last drawn: its wrapped
    /// rows less those that fit. The view sets it, as only it knows the
    /// pane's size.
    pub(crate) reader_bottom: Cell<u16>,
    /// How many rows of the reader fit when last drawn, for a page.
    pub(crate) reader_rows: Cell<u16>,
    /// What is loading, or what went wrong, for the status bar.
    pub(crate) notice: Option<String>,
    /// The last list asked for; an answer to any other comes too late.
    list_request: u64,
    /// The last conversation asked for, likewise.
    read_request: u64,
}

/// Something the person asked for, or an answer to an [`Effect`].
#[derive(Debug, Clone)]
pub(crate) enum Message {
    NextPane,
    PreviousPane,
    Up,
    Down,
    PageUp,
    PageDown,
    Top,
    Bottom,
    Open,
    Refresh,
    ToggleHelp,
    CloseHelp,
    Quit,
    PlacesLoaded(Result<Vec<Place>, String>),
    ConversationsLoaded {
        request: u64,
        result: Result<(u32, Vec<Conversation>), String>,
    },
    ConversationLoaded {
        request: u64,
        result: Result<Reading, String>,
    },
}

/// Work the interface hands out, whose answer comes back as a [`Message`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Effect {
    Places,
    Conversations { request: u64, place: String },
    Conversation { request: u64, id: String },
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
            listed_id: label_ids::INBOX.to_owned(),
            conversations: Vec::new(),
            total: 0,
            selected_conversation: 0,
            reading: None,
            scroll: 0,
            reader_bottom: Cell::new(0),
            reader_rows: Cell::new(1),
            notice: Some("Loading…".to_owned()),
            list_request: 0,
            read_request: 0,
        }
    }

    /// What to load first: the folders, and the Inbox meanwhile, since it
    /// is the first of them.
    pub(crate) fn start(&mut self) -> Vec<Effect> {
        vec![
            Effect::Places,
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
            Message::PageUp => self.step(-self.page()),
            Message::PageDown => self.step(self.page()),
            Message::Top => self.step(isize::MIN),
            Message::Bottom => self.step(isize::MAX),
            Message::Open => return self.open(),
            Message::Refresh => {
                let (id, name) = (self.listed_id.clone(), self.listed.clone());
                return vec![Effect::Places, self.list(id, name)];
            }
            Message::PlacesLoaded(Ok(places)) => {
                let selected = self
                    .places
                    .get(self.selected_place)
                    .map(|place| place.id.clone());
                self.places = places;
                self.selected_place = selected
                    .and_then(|id| self.places.iter().position(|place| place.id == id))
                    .unwrap_or(0);
            }
            Message::PlacesLoaded(Err(error)) => {
                self.notice = Some(format!("Cannot load the folders: {error}"));
            }
            Message::ConversationsLoaded { request, .. } if request != self.list_request => {}
            Message::ConversationsLoaded { result, .. } => match result {
                Ok((total, conversations)) => {
                    // A refresh keeps the selection on the same conversation.
                    let selected = self
                        .conversations
                        .get(self.selected_conversation)
                        .map(|conversation| conversation.id.clone());
                    self.total = total;
                    self.conversations = conversations;
                    self.selected_conversation = selected
                        .and_then(|id| {
                            self.conversations
                                .iter()
                                .position(|conversation| conversation.id == id)
                        })
                        .unwrap_or(0);
                    self.notice = None;
                }
                Err(error) => {
                    self.notice = Some(format!("Cannot load {}: {error}", self.listed));
                }
            },
            Message::ConversationLoaded { request, .. } if request != self.read_request => {}
            Message::ConversationLoaded { result, .. } => match result {
                Ok(reading) => {
                    self.reading = Some(reading);
                    self.scroll = 0;
                    self.notice = None;
                }
                Err(error) => {
                    self.notice = Some(format!("Cannot open the conversation: {error}"));
                }
            },
        }
        Vec::new()
    }

    /// How many rows a page of the reader moves: as many as fit.
    fn page(&self) -> isize {
        isize::try_from(self.reader_rows.get()).map_or(1, |rows| rows.max(1))
    }

    /// Moves the focused list's selection, or the reader, `by` rows, as far
    /// as there is to go.
    fn step(&mut self, by: isize) {
        let (selected, len) = match self.focus {
            Pane::Folders => (&mut self.selected_place, self.places.len()),
            Pane::Conversations => (&mut self.selected_conversation, self.conversations.len()),
            Pane::Reader => {
                let scroll = usize::from(self.scroll).saturating_add_signed(by);
                let bottom = usize::from(self.reader_bottom.get());
                self.scroll = u16::try_from(scroll.min(bottom)).unwrap_or(u16::MAX);
                return;
            }
        };
        *selected = selected
            .saturating_add_signed(by)
            .min(len.saturating_sub(1));
    }

    /// Opens what the focused list's selection is on, in the pane after it:
    /// a folder's conversations, or a conversation in the reader.
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
            Pane::Conversations => {
                let Some(conversation) = self.conversations.get(self.selected_conversation) else {
                    return Vec::new();
                };
                let id = conversation.id.clone();
                self.read_request += 1;
                self.notice = Some("Opening the conversation…".to_owned());
                self.focus = Pane::Reader;
                vec![Effect::Conversation {
                    request: self.read_request,
                    id,
                }]
            }
            Pane::Reader => Vec::new(),
        }
    }

    /// Asks for the first page of the place `id`, named `name`, making any
    /// answer still on its way for another one too late.
    fn list(&mut self, id: String, name: String) -> Effect {
        self.list_request += 1;
        self.notice = Some(format!("Loading {name}…"));
        self.listed = name;
        self.listed_id.clone_from(&id);
        Effect::Conversations {
            request: self.list_request,
            place: id,
        }
    }
}

/// Whether `message` came from a key, not from an answer.
fn is_key(message: &Message) -> bool {
    !matches!(
        message,
        Message::PlacesLoaded(_)
            | Message::ConversationsLoaded { .. }
            | Message::ConversationLoaded { .. }
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
            return match key.code {
                KeyCode::Char('c') => Some(Self::Quit),
                KeyCode::Char('r') => Some(Self::Refresh),
                _ => None,
            };
        }
        match key.code {
            KeyCode::Char('q') => Some(Self::Quit),
            KeyCode::Char('?') => Some(Self::ToggleHelp),
            KeyCode::Esc => Some(Self::CloseHelp),
            KeyCode::Tab => Some(Self::NextPane),
            KeyCode::BackTab => Some(Self::PreviousPane),
            KeyCode::Up | KeyCode::Char('k') => Some(Self::Up),
            KeyCode::Down | KeyCode::Char('j') => Some(Self::Down),
            KeyCode::PageUp => Some(Self::PageUp),
            KeyCode::PageDown => Some(Self::PageDown),
            KeyCode::Home | KeyCode::Char('g') => Some(Self::Top),
            KeyCode::End | KeyCode::Char('G') => Some(Self::Bottom),
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
        let Effect::Conversations { request, .. } = start[1].clone() else {
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
                Effect::Places,
                Effect::Conversations {
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
            [Effect::Conversations {
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

    fn reading(lines: usize) -> Reading {
        Reading {
            subject: "Plans".into(),
            lines: (0..lines).map(|n| format!("line {n}")).collect(),
        }
    }

    #[test]
    fn enter_on_a_conversation_opens_it_in_the_reader() {
        let mut app = loaded();
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Down);
        let effects = press(&mut app, KeyCode::Enter);
        assert_eq!(
            effects,
            [Effect::Conversation {
                request: 1,
                id: "b".into()
            }]
        );
        assert_eq!(app.focus, Pane::Reader);
        assert_eq!(app.notice.as_deref(), Some("Opening the conversation…"));
        // An answer for a conversation asked for before comes too late.
        app.update(Message::ConversationLoaded {
            request: 0,
            result: Ok(reading(1)),
        });
        assert!(app.reading.is_none());
        app.update(Message::ConversationLoaded {
            request: 1,
            result: Ok(reading(3)),
        });
        assert_eq!(app.reading.as_ref().unwrap().lines.len(), 3);
        assert_eq!(app.notice, None);
        app.update(Message::ConversationLoaded {
            request: 1,
            result: Err("gone".into()),
        });
        assert_eq!(
            app.notice.as_deref(),
            Some("Cannot open the conversation: gone")
        );
    }

    #[test]
    fn the_reader_scrolls_by_rows_and_pages_as_far_as_its_bottom() {
        let mut app = loaded();
        app.focus = Pane::Reader;
        app.reading = Some(reading(100));
        // As the view leaves them after drawing 100 rows 30 at a time.
        app.reader_bottom.set(70);
        app.reader_rows.set(30);
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.scroll, 1);
        press(&mut app, KeyCode::PageDown);
        assert_eq!(app.scroll, 31);
        press(&mut app, KeyCode::Char('G'));
        assert_eq!(app.scroll, 70);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.scroll, 70);
        press(&mut app, KeyCode::PageUp);
        assert_eq!(app.scroll, 40);
        press(&mut app, KeyCode::Char('g'));
        assert_eq!(app.scroll, 0);
        press(&mut app, KeyCode::Up);
        assert_eq!(app.scroll, 0);
    }

    #[test]
    fn home_and_end_go_to_the_ends_of_a_list() {
        let mut app = loaded();
        app.focus = Pane::Conversations;
        press(&mut app, KeyCode::End);
        assert_eq!(app.selected_conversation, 1);
        press(&mut app, KeyCode::Home);
        assert_eq!(app.selected_conversation, 0);
    }

    #[test]
    fn ctrl_r_reloads_the_folders_and_the_list_keeping_the_selection() {
        let mut app = loaded();
        app.selected_place = 1;
        app.focus = Pane::Conversations;
        press(&mut app, KeyCode::Down);
        let refresh = Event::Key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        let effects = app.update(Message::from_event(&refresh).unwrap());
        assert_eq!(
            effects,
            [
                Effect::Places,
                Effect::Conversations {
                    request: 2,
                    place: label_ids::INBOX.into()
                }
            ]
        );
        assert_eq!(app.notice.as_deref(), Some("Loading Inbox…"));
        // New mail came in above the selected conversation, b.
        app.update(Message::ConversationsLoaded {
            request: 2,
            result: Ok((
                3,
                vec![conversation("new"), conversation("a"), conversation("b")],
            )),
        });
        assert_eq!(app.selected_conversation, 2);
        app.update(Message::PlacesLoaded(Ok(vec![
            place("folder-0", "Added"),
            place(label_ids::INBOX, "Inbox"),
            place("folder-1", "Work"),
        ])));
        assert_eq!(app.places[app.selected_place].name, "Work");
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
