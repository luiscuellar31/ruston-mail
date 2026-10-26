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

/// The open conversation's links listed over the panes: which one is
/// selected, and whether opening it waits for a yes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LinkPick {
    pub(crate) selected: usize,
    pub(crate) confirming: bool,
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
    /// The pages of its conversations loaded so far, newest first.
    pub(crate) conversations: Vec<Conversation>,
    /// The next page of them to ask for; 0 until the first has come.
    next_page: u32,
    /// Whether that page is on its way.
    loading_more: bool,
    /// How many conversations it has in all.
    pub(crate) total: u32,
    /// Which conversation the list's selection is on.
    pub(crate) selected_conversation: usize,
    /// The conversation the reader shows, once it has loaded.
    pub(crate) reading: Option<Reading>,
    /// How many rows down the reader is scrolled.
    pub(crate) scroll: u16,
    /// Whether the reader shows quoted passages instead of folding them.
    pub(crate) show_quotes: bool,
    /// Whether an unread conversation is marked read once the reader shows
    /// it, as the desktop marks it; `tui --keep-unread` turns it off.
    pub(crate) marks_read: bool,
    /// The open conversation's links, while `o` lists them over the panes.
    pub(crate) picking_link: Option<LinkPick>,
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
    ToggleQuotes,
    /// Lists the open conversation's links, to open one.
    ShowLinks,
    /// Answers yes to the question on screen.
    Yes,
    Refresh,
    ToggleHelp,
    /// Closes the list of keys, or else goes back a pane.
    Back,
    Quit,
    PlacesLoaded(Result<Vec<Place>, String>),
    ConversationsLoaded {
        request: u64,
        page: u32,
        result: Result<(u32, Vec<Conversation>), String>,
    },
    ConversationLoaded {
        request: u64,
        result: Result<Reading, String>,
    },
    MarkedRead {
        id: String,
        result: Result<(), String>,
    },
    LinkOpened(Result<(), String>),
}

/// Work the interface hands out, whose answer comes back as a [`Message`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Effect {
    Places,
    Conversations {
        request: u64,
        place: String,
        page: u32,
    },
    Conversation {
        request: u64,
        id: String,
    },
    MarkRead {
        id: String,
        place: String,
    },
    OpenLink {
        url: String,
    },
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
            next_page: 0,
            loading_more: false,
            total: 0,
            selected_conversation: 0,
            reading: None,
            scroll: 0,
            show_quotes: false,
            marks_read: true,
            picking_link: None,
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
            Message::Back if self.help => self.help = false,
            // The open list of keys covers the panes, so they keep still.
            _ if self.help && is_key(&message) => {}
            // So does the list of links, which takes the keys meanwhile.
            _ if self.picking_link.is_some() && is_key(&message) => {
                return self.pick_link(&message);
            }
            // Back from the list or the reader to the pane before it; the
            // folders come first, so nothing is before them.
            Message::Back if self.focus != Pane::Folders => {
                self.focus = self.focus.previous();
            }
            Message::Back => {}
            Message::NextPane => self.focus = self.focus.next(),
            Message::PreviousPane => self.focus = self.focus.previous(),
            Message::Up => self.step(-1),
            Message::Down => return self.down(),
            Message::PageUp => self.step(-self.page()),
            Message::PageDown => self.step(self.page()),
            Message::Top => self.step(isize::MIN),
            Message::Bottom => self.step(isize::MAX),
            Message::Open => return self.open(),
            Message::ToggleQuotes => self.show_quotes = !self.show_quotes,
            Message::ShowLinks => self.show_links(),
            // Nothing on screen waits for a yes.
            Message::Yes => {}
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
            Message::ConversationsLoaded { page, result, .. } if page > 0 => {
                self.loading_more = false;
                match result {
                    Ok((total, more)) => {
                        // Mail that came in meanwhile pushes some of the
                        // listed conversations onto this page too.
                        let more: Vec<Conversation> = more
                            .into_iter()
                            .filter(|new| self.conversations.iter().all(|old| old.id != new.id))
                            .collect();
                        self.conversations.extend(more);
                        self.total = total;
                        self.next_page = page + 1;
                        self.notice = None;
                    }
                    Err(error) => {
                        self.notice = Some(format!("Cannot load more of {}: {error}", self.listed));
                    }
                }
            }
            Message::ConversationsLoaded { result, .. } => match result {
                Ok((total, conversations)) => {
                    // A refresh keeps the selection on the same conversation.
                    let selected = self
                        .conversations
                        .get(self.selected_conversation)
                        .map(|conversation| conversation.id.clone());
                    self.total = total;
                    self.conversations = conversations;
                    self.next_page = 1;
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
                    let id = reading.id.clone();
                    // Links listed for the conversation before are not its.
                    self.picking_link = None;
                    self.reading = Some(reading);
                    self.scroll = 0;
                    self.notice = None;
                    return self.mark_read(id);
                }
                Err(error) => {
                    self.notice = Some(format!("Cannot open the conversation: {error}"));
                }
            },
            Message::MarkedRead { id, result: Ok(()) } => self.marked_read(&id),
            Message::MarkedRead {
                result: Err(error), ..
            } => {
                self.notice = Some(format!("Cannot mark the conversation read: {error}"));
            }
            Message::LinkOpened(Ok(())) => {}
            Message::LinkOpened(Err(error)) => {
                self.notice = Some(format!("Cannot open the link: {error}"));
            }
        }
        Vec::new()
    }

    /// Lists the open conversation's links, to open one, or says why not.
    fn show_links(&mut self) {
        match &self.reading {
            Some(reading) if !reading.links.is_empty() => {
                self.picking_link = Some(LinkPick {
                    selected: 0,
                    confirming: false,
                });
            }
            Some(_) => self.notice = Some("This conversation has no links.".to_owned()),
            None => self.notice = Some("Open a conversation to list its links.".to_owned()),
        }
    }

    /// What a key does while the links are listed: move through them, ask
    /// whether to open the selected one, or put them away. At that question
    /// only `y` opens it; any other key goes back to the links.
    fn pick_link(&mut self, message: &Message) -> Vec<Effect> {
        let (Some(mut pick), Some(reading)) = (self.picking_link, &self.reading) else {
            self.picking_link = None;
            return Vec::new();
        };
        let last = reading.links.len().saturating_sub(1);
        if pick.confirming {
            pick.confirming = false;
            if matches!(message, Message::Yes)
                && let Some(url) = reading.links.get(pick.selected)
            {
                let url = url.clone();
                self.picking_link = None;
                self.notice = Some("Opening the link in your browser…".to_owned());
                return vec![Effect::OpenLink { url }];
            }
        } else {
            match message {
                Message::Up => pick.selected = pick.selected.saturating_sub(1),
                Message::Down => pick.selected = (pick.selected + 1).min(last),
                Message::Top => pick.selected = 0,
                Message::Bottom => pick.selected = last,
                Message::Open => pick.confirming = true,
                Message::Back | Message::ShowLinks => {
                    self.picking_link = None;
                    return Vec::new();
                }
                _ => {}
            }
        }
        self.picking_link = Some(pick);
        Vec::new()
    }

    /// Whether the listed place's first page came with no conversations,
    /// rather than being on its way.
    pub(crate) fn listed_nothing(&self) -> bool {
        self.next_page > 0 && self.conversations.is_empty()
    }

    /// Moves down a row, and at the last row of a list with more to it,
    /// asks for its next page, which lands below. While a list's first page
    /// is on its way, the rows shown are from before it, so none follow.
    fn down(&mut self) -> Vec<Effect> {
        let at_end = self.selected_conversation + 1 >= self.conversations.len();
        let more = self.conversations.len() < usize::try_from(self.total).unwrap_or(usize::MAX);
        if self.focus == Pane::Conversations
            && at_end
            && more
            && self.next_page > 0
            && !self.loading_more
        {
            self.loading_more = true;
            self.notice = Some(format!("Loading more of {}…", self.listed));
            return vec![Effect::Conversations {
                request: self.list_request,
                place: self.listed_id.clone(),
                page: self.next_page,
            }];
        }
        self.step(1);
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
        self.next_page = 0;
        self.loading_more = false;
        self.notice = Some(format!("Loading {name}…"));
        self.listed = name;
        self.listed_id.clone_from(&id);
        Effect::Conversations {
            request: self.list_request,
            place: id,
            page: 0,
        }
    }

    /// Asks for the conversation `id`, which the reader now shows, to be
    /// marked read, as the desktop does, when the list has it unread and
    /// conversations are not to be kept unread.
    fn mark_read(&self, id: String) -> Vec<Effect> {
        let unread = self
            .conversations
            .iter()
            .any(|conversation| conversation.id == id && conversation.num_unread > 0);
        if !self.marks_read || !unread {
            return Vec::new();
        }
        vec![Effect::MarkRead {
            id,
            place: self.listed_id.clone(),
        }]
    }

    /// Shows the conversation `id` read, now that it is, and one fewer
    /// unread conversation in each place that had it unread.
    fn marked_read(&mut self, id: &str) {
        let Some(conversation) = self
            .conversations
            .iter_mut()
            .find(|conversation| conversation.id == id)
        else {
            return;
        };
        conversation.num_unread = 0;
        for label in &mut conversation.labels {
            if label.context_num_unread.unwrap_or(0) > 0 {
                label.context_num_unread = Some(0);
                if let Some(place) = self.places.iter_mut().find(|place| place.id == label.id) {
                    place.unread = (place.unread - 1).max(0);
                }
            }
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
            | Message::MarkedRead { .. }
            | Message::LinkOpened(_)
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
            KeyCode::Esc => Some(Self::Back),
            KeyCode::Tab => Some(Self::NextPane),
            KeyCode::BackTab => Some(Self::PreviousPane),
            KeyCode::Up | KeyCode::Char('k') => Some(Self::Up),
            KeyCode::Down | KeyCode::Char('j') => Some(Self::Down),
            KeyCode::PageUp => Some(Self::PageUp),
            KeyCode::PageDown => Some(Self::PageDown),
            KeyCode::Home | KeyCode::Char('g') => Some(Self::Top),
            KeyCode::End | KeyCode::Char('G') => Some(Self::Bottom),
            KeyCode::Enter => Some(Self::Open),
            KeyCode::Char('z') => Some(Self::ToggleQuotes),
            KeyCode::Char('o') => Some(Self::ShowLinks),
            KeyCode::Char('y') => Some(Self::Yes),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mailbox::PlaceKind;
    use ruston_core::model::ConversationLabel;

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
            page: 0,
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
                    place: label_ids::INBOX.into(),
                    page: 0
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
                place: "folder-1".into(),
                page: 0
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
            page: 0,
            request: 1,
            result: Ok((1, vec![conversation("old")])),
        });
        assert_eq!(app.total, 2);
        app.update(Message::ConversationsLoaded {
            page: 0,
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
            page: 0,
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
            id: "b".into(),
            subject: "Plans".into(),
            lines: (0..lines).map(|n| format!("line {n}")).collect(),
            quotes: Vec::new(),
            links: Vec::new(),
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
    fn z_shows_quoted_text_and_hides_it_again() {
        let mut app = loaded();
        press(&mut app, KeyCode::Char('z'));
        assert!(app.show_quotes);
        press(&mut app, KeyCode::Char('z'));
        assert!(!app.show_quotes);
    }

    /// An unread conversation, in the Inbox and in `label`.
    fn unread(id: &str, label: &str) -> Conversation {
        let unread_in = |label: &str| ConversationLabel {
            id: label.into(),
            context_num_unread: Some(1),
            ..ConversationLabel::default()
        };
        Conversation {
            id: id.into(),
            num_unread: 1,
            labels: vec![unread_in(label_ids::INBOX), unread_in(label)],
            ..Conversation::default()
        }
    }

    #[test]
    fn an_unread_conversation_is_marked_read_once_the_reader_shows_it() {
        let mut app = loaded();
        app.conversations[1] = unread("b", "folder-1");
        app.places[0].unread = 3;
        app.places[1].unread = 1;
        app.focus = Pane::Conversations;
        press(&mut app, KeyCode::Down);
        // Nothing is marked before it shows.
        assert_eq!(
            press(&mut app, KeyCode::Enter),
            [Effect::Conversation {
                request: 1,
                id: "b".into()
            }]
        );
        let effects = app.update(Message::ConversationLoaded {
            request: 1,
            result: Ok(reading(3)),
        });
        assert_eq!(
            effects,
            [Effect::MarkRead {
                id: "b".into(),
                place: label_ids::INBOX.into()
            }]
        );
        assert_eq!(app.conversations[1].num_unread, 1);

        app.update(Message::MarkedRead {
            id: "b".into(),
            result: Ok(()),
        });
        assert_eq!(app.conversations[1].num_unread, 0);
        let unread: Vec<i64> = app.places.iter().map(|place| place.unread).collect();
        assert_eq!(unread, [2, 0]);
        // Read now, it is not marked again.
        press(&mut app, KeyCode::BackTab);
        press(&mut app, KeyCode::Enter);
        let effects = app.update(Message::ConversationLoaded {
            request: 2,
            result: Ok(reading(3)),
        });
        assert!(effects.is_empty());
    }

    #[test]
    fn keep_unread_leaves_an_opened_conversation_unread() {
        let mut app = loaded();
        app.marks_read = false;
        app.conversations[1] = unread("b", "folder-1");
        app.focus = Pane::Conversations;
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        let effects = app.update(Message::ConversationLoaded {
            request: 1,
            result: Ok(reading(3)),
        });
        assert!(effects.is_empty());
    }

    #[test]
    fn a_failed_mark_says_so_and_leaves_the_conversation_unread() {
        let mut app = loaded();
        app.conversations[1] = unread("b", "folder-1");
        app.update(Message::MarkedRead {
            id: "b".into(),
            result: Err("offline".into()),
        });
        assert_eq!(
            app.notice.as_deref(),
            Some("Cannot mark the conversation read: offline")
        );
        assert_eq!(app.conversations[1].num_unread, 1);
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
                    place: label_ids::INBOX.into(),
                    page: 0
                }
            ]
        );
        assert_eq!(app.notice.as_deref(), Some("Loading Inbox…"));
        // New mail came in above the selected conversation, b.
        app.update(Message::ConversationsLoaded {
            page: 0,
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
    fn down_at_the_end_of_a_longer_list_loads_its_next_page_below() {
        let mut app = loaded();
        app.total = 5;
        app.focus = Pane::Conversations;
        press(&mut app, KeyCode::Down);
        assert_eq!(app.selected_conversation, 1);
        let effects = press(&mut app, KeyCode::Down);
        assert_eq!(
            effects,
            [Effect::Conversations {
                request: 1,
                place: label_ids::INBOX.into(),
                page: 1
            }]
        );
        assert_eq!(app.notice.as_deref(), Some("Loading more of Inbox…"));
        // Only once at a time.
        assert!(press(&mut app, KeyCode::Down).is_empty());
        // "b" came back on this page as well, pushed down by new mail.
        app.update(Message::ConversationsLoaded {
            request: 1,
            page: 1,
            result: Ok((5, vec![conversation("b"), conversation("c")])),
        });
        let ids: Vec<&str> = app.conversations.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c"]);
        assert_eq!(app.notice, None);
        assert_eq!(app.selected_conversation, 1);
        press(&mut app, KeyCode::Down);
        assert_eq!(
            press(&mut app, KeyCode::Down),
            [Effect::Conversations {
                request: 1,
                place: label_ids::INBOX.into(),
                page: 2
            }]
        );
    }

    #[test]
    fn a_list_with_nothing_more_to_it_just_stays_at_its_end() {
        let mut app = loaded();
        app.focus = Pane::Conversations;
        press(&mut app, KeyCode::Down);
        assert!(press(&mut app, KeyCode::Down).is_empty());
        assert_eq!(app.selected_conversation, 1);
    }

    #[test]
    fn a_next_page_for_a_list_since_replaced_comes_too_late() {
        let mut app = loaded();
        app.total = 5;
        app.focus = Pane::Conversations;
        press(&mut app, KeyCode::End);
        assert_eq!(press(&mut app, KeyCode::Down).len(), 1);
        let refresh = Event::Key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        app.update(Message::from_event(&refresh).unwrap());
        app.update(Message::ConversationsLoaded {
            request: 1,
            page: 1,
            result: Ok((5, vec![conversation("late")])),
        });
        assert!(app.conversations.iter().all(|c| c.id != "late"));
        // A failed next page can be asked for again.
        app.update(Message::ConversationsLoaded {
            request: 2,
            page: 0,
            result: Ok((5, vec![conversation("a"), conversation("b")])),
        });
        press(&mut app, KeyCode::End);
        press(&mut app, KeyCode::Down);
        app.update(Message::ConversationsLoaded {
            request: 2,
            page: 1,
            result: Err("offline".into()),
        });
        assert_eq!(
            app.notice.as_deref(),
            Some("Cannot load more of Inbox: offline")
        );
        assert_eq!(press(&mut app, KeyCode::Down).len(), 1);
    }

    #[test]
    fn a_list_whose_first_page_is_on_its_way_asks_for_no_more_yet() {
        let mut app = loaded();
        app.total = 5;
        // Work is asked for; the Inbox's rows stay until its answer comes.
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::End);
        assert!(press(&mut app, KeyCode::Down).is_empty());
        app.update(Message::ConversationsLoaded {
            request: 2,
            page: 0,
            result: Ok((5, vec![conversation("w1"), conversation("w2")])),
        });
        press(&mut app, KeyCode::End);
        assert_eq!(
            press(&mut app, KeyCode::Down),
            [Effect::Conversations {
                request: 2,
                place: "folder-1".into(),
                page: 1
            }]
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
    fn o_lists_the_links_and_only_a_yes_opens_the_one_selected() {
        let mut app = loaded();
        press(&mut app, KeyCode::Char('o'));
        assert_eq!(
            app.notice.as_deref(),
            Some("Open a conversation to list its links.")
        );
        app.reading = Some(reading(1));
        press(&mut app, KeyCode::Char('o'));
        assert_eq!(
            app.notice.as_deref(),
            Some("This conversation has no links.")
        );

        let mut linked = reading(1);
        linked.links = vec![
            "https://example.test/a".into(),
            "https://example.test/b".into(),
        ];
        app.reading = Some(linked);
        press(&mut app, KeyCode::Char('o'));
        // The keys move through the links now, not the folders.
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.selected_place, 0);
        let asked = LinkPick {
            selected: 1,
            confirming: true,
        };
        assert!(press(&mut app, KeyCode::Enter).is_empty());
        assert_eq!(app.picking_link, Some(asked));
        // Any other key goes back to the links.
        assert!(press(&mut app, KeyCode::Char('j')).is_empty());
        assert_eq!(app.picking_link.map(|pick| pick.confirming), Some(false));
        press(&mut app, KeyCode::Enter);
        assert_eq!(
            press(&mut app, KeyCode::Char('y')),
            [Effect::OpenLink {
                url: "https://example.test/b".into()
            }]
        );
        assert_eq!(app.picking_link, None);

        app.update(Message::LinkOpened(Err("no browser".into())));
        assert_eq!(
            app.notice.as_deref(),
            Some("Cannot open the link: no browser")
        );
        // Esc puts the links away, and y alone asks nothing.
        press(&mut app, KeyCode::Char('o'));
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.picking_link, None);
        assert!(press(&mut app, KeyCode::Char('y')).is_empty());
    }

    #[test]
    fn a_conversation_that_loads_puts_away_the_links_of_the_one_before() {
        let mut app = loaded();
        let mut linked = reading(1);
        linked.links = vec!["https://example.test/a".into()];
        app.reading = Some(linked);
        press(&mut app, KeyCode::Char('o'));
        app.update(Message::ConversationLoaded {
            request: 0,
            result: Ok(reading(2)),
        });
        assert_eq!(app.picking_link, None);
    }

    #[test]
    fn esc_closes_the_keys_first_then_goes_back_a_pane() {
        let mut app = loaded();
        app.focus = Pane::Reader;
        press(&mut app, KeyCode::Char('?'));
        press(&mut app, KeyCode::Esc);
        assert_eq!((app.help, app.focus), (false, Pane::Reader));
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.focus, Pane::Conversations);
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.focus, Pane::Folders);
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.focus, Pane::Folders);
    }

    #[test]
    fn an_empty_folder_leaves_a_way_back_to_the_folders() {
        let mut app = loaded();
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        // Its first page is on its way: the Inbox's rows still show.
        assert!(!app.listed_nothing());
        app.update(Message::ConversationsLoaded {
            request: 2,
            page: 0,
            result: Ok((0, Vec::new())),
        });
        assert!(app.listed_nothing());
        assert_eq!(app.focus, Pane::Conversations);
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Up);
        assert_eq!((app.focus, app.selected_place), (Pane::Folders, 0));
    }

    #[test]
    fn a_resize_asks_for_nothing() {
        assert!(Message::from_event(&Event::Resize(80, 24)).is_none());
    }
}
