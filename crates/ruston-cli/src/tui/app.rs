//! The interface's state and what each key or answer does to it, apart from
//! drawing and from the terminal, so both can be tested without one. What
//! must happen elsewhere, such as loading a page, comes back as an [`Effect`].

use super::command::{Command, CommandLine};
use super::compose::{AnswerKind, Asked, Checked, Compose, Sending};
use super::reading::{Reading, size_label};
use crate::mailbox::{Change, Outgoing, Place, PlaceKind, SendFailure, deletes_forever};
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};
use ruston_core::mail::organize::folder_id_among;
use ruston_core::mail::send::PROTECTION_LOOKUP_LIMIT;
use ruston_core::model::enums::label_ids;
use ruston_core::{Conversation, Error, Label, RecipientProtection, SearchOpts};
use std::cell::Cell;
use std::path::PathBuf;

/// How many rows a notch of the mouse wheel scrolls the reader or the list
/// of keys, as most programs scroll text.
const WHEEL_ROWS: isize = 3;

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

/// Where the view last drew each pane, for the mouse to find what it
/// points at: in the order of [`Pane::ALL`], the area inside its borders,
/// empty for a pane not drawn, and the first row of its list it showed.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Drawn([(Rect, usize); 3]);

impl Drawn {
    /// Notes that `pane` was drawn within `inner`, its list from its row
    /// `first`.
    pub(crate) fn note(&mut self, pane: Pane, inner: Rect, first: usize) {
        if let Some(at) = Pane::ALL.iter().position(|each| *each == pane) {
            self.0[at] = (inner, first);
        }
    }

    /// The pane drawn at `column` and `row`, if any, with the row of its
    /// list there; the reader has none.
    fn at(&self, column: u16, row: u16) -> Option<(Pane, Option<usize>)> {
        let position = Position::new(column, row);
        let (pane, (inner, first)) = Pane::ALL
            .into_iter()
            .zip(self.0)
            .find(|(_, (inner, _))| inner.contains(position))?;
        let listed = pane != Pane::Reader;
        Some((pane, listed.then(|| first + usize::from(row - inner.y))))
    }
}

/// The open conversation's links listed over the panes: which one is
/// selected, and whether opening it waits for a yes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LinkPick {
    pub(crate) selected: usize,
    pub(crate) confirming: bool,
}

/// A search whose results the list shows: what was typed after its
/// command, which names the list, what it asks and of whom, and the label
/// ID and name of the place listed before it, which Esc lists again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Search {
    pub(crate) words: String,
    pub(crate) query: Query,
    pub(crate) back: (String, String),
}

/// What a search asks, and of whom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Query {
    /// Proton's search, for the words and the filters typed beside them.
    Proton(SearchOpts),
    /// The local index, which looks for the words alone, and what it finds
    /// narrowed by the filters typed beside them.
    Local(SearchOpts),
}

/// The last move made out of the folder listed, which `Z` takes back, as
/// the desktop's Undo does: the conversations it moved, and the folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Undo {
    ids: Vec<String>,
    from: String,
}

/// The folders `g` goes to, each with the key after it that names it, its
/// label ID and its name: Proton Mail's own, All mail included.
pub(crate) const GO_TO: [(char, &str, &str); 8] = [
    ('i', label_ids::INBOX, "Inbox"),
    ('a', label_ids::ARCHIVE, "Archive"),
    ('e', label_ids::SENT, "Sent"),
    ('*', label_ids::STARRED, "Starred"),
    ('d', label_ids::DRAFTS, "Drafts"),
    ('t', label_ids::TRASH, "Trash"),
    ('s', label_ids::SPAM, "Spam"),
    ('m', label_ids::ALL_MAIL, "All mail"),
];

/// A question over the panes before something that cannot be undone. Only
/// `y` answers yes; any other key leaves everything as it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Question {
    /// Whether to delete forever the conversations `ids`, whose subjects
    /// are `subjects`, from the place with label ID `place`, named `name`.
    Delete {
        ids: Vec<String>,
        subjects: Vec<String>,
        place: String,
        name: String,
    },
    /// Whether to delete forever everything in the place with label ID
    /// `place`, named `name`.
    Empty { place: String, name: String },
    /// Whether to throw away the message being written, which is kept
    /// nowhere else.
    Discard,
    /// Whether to send `outgoing`, with the files named in `files`,
    /// saying how Proton would protect it for each recipient as the
    /// lookups numbered `request` answer.
    Send {
        request: u64,
        outgoing: Outgoing,
        files: Vec<String>,
        checks: Vec<(String, Checked)>,
    },
}

/// What the interface shows and where the keys go.
pub(crate) struct App {
    /// Whose mail this is, for the status bar.
    pub(crate) account: String,
    /// The pane the keys act on.
    pub(crate) focus: Pane,
    /// Whether the list of keys is open over the panes.
    pub(crate) help: bool,
    /// How many rows down the list of keys is scrolled.
    pub(crate) help_scroll: u16,
    /// The furthest the list of keys could scroll when last drawn, and how
    /// many of its rows fit, which the view sets as it does the reader's.
    pub(crate) help_bottom: Cell<u16>,
    pub(crate) help_rows: Cell<u16>,
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
    /// How many rows down the reader is scrolled; the view scrolls it to
    /// the message `landing` names.
    pub(crate) scroll: Cell<u16>,
    /// The message of the open conversation the reader brings to its top
    /// when next drawn, unless it starts in the upper third already: where
    /// the conversation opens, or one just folded or shown. The view takes
    /// it, as only it knows where each message lands.
    pub(crate) landing: Cell<Option<usize>>,
    /// The message at the reader's top when last drawn, which Enter folds
    /// or shows; the view sets it.
    pub(crate) in_view: Cell<Option<usize>>,
    /// Whether the reader shows quoted passages instead of folding them.
    pub(crate) show_quotes: bool,
    /// Whether the reader shows the messages already read instead of
    /// folding them.
    pub(crate) show_read: bool,
    /// The open conversation's messages, by their place in it, that Enter
    /// folded or showed the other way from what `show_read` says.
    pub(crate) toggled: Vec<usize>,
    /// Whether an unread conversation is marked read once the reader shows
    /// it, as the desktop marks it; `tui --keep-unread` turns it off.
    pub(crate) marks_read: bool,
    /// The open conversation's links, while `b` lists them over the panes.
    pub(crate) picking_link: Option<LinkPick>,
    /// Which of the open conversation's attachments is selected, while `v`
    /// lists them over the panes.
    pub(crate) picking_attachment: Option<usize>,
    /// The question over the panes that waits for a yes, if one does.
    pub(crate) question: Option<Question>,
    /// Whether `g` was pressed, so the next key names a folder to go to.
    pub(crate) going: bool,
    /// The message being written, which then takes the whole screen and
    /// every key.
    pub(crate) compose: Option<Compose>,
    /// The address a new message goes from, for the editor to show.
    pub(crate) sender: String,
    /// The last send asked about; lookups for any other come too late.
    send_request: u64,
    /// The last file checked or path completed, likewise.
    file_request: u64,
    /// The command line, while `:` has it open at the foot.
    pub(crate) command_line: Option<CommandLine>,
    /// The lines run from it, oldest first, for Up and Down.
    history: Vec<String>,
    /// The search the list shows the results of, if it shows one.
    pub(crate) search: Option<Search>,
    /// The conversations marked with Space, by ID, which an action from the
    /// list acts on all of.
    pub(crate) marked: Vec<String>,
    /// The move `Z` can take back, until another change is made or another
    /// place listed.
    pub(crate) undo: Option<Undo>,
    /// The furthest the reader could scroll when last drawn: its wrapped
    /// rows less those that fit. The view sets it, as only it knows the
    /// pane's size.
    pub(crate) reader_bottom: Cell<u16>,
    /// How many rows of the reader fit when last drawn, for a page.
    pub(crate) reader_rows: Cell<u16>,
    /// Where each pane was last drawn, which the view sets, for a click or
    /// the wheel to find what it points at.
    pub(crate) drawn: Cell<Drawn>,
    /// What is loading, or what went wrong, for the status bar.
    pub(crate) notice: Option<String>,
    /// The last list asked for; an answer to any other comes too late.
    list_request: u64,
    /// Whether that list is the one shown, loading again, as after Ctrl+R
    /// or a change, rather than another place's.
    reloading: bool,
    /// What the status bar says once that list has loaded, rather than
    /// nothing: that a message was sent.
    said_once_loaded: Option<String>,
    /// The last conversation asked for, likewise.
    read_request: u64,
}

/// Something the person asked for, or an answer to an [`Effect`].
#[derive(Debug, Clone)]
pub(crate) enum Message {
    NextPane,
    PreviousPane,
    /// Opens what the focused list's selection is on, as Enter does.
    Right,
    /// Moves to the pane on the left, when there is one.
    Left,
    Up,
    Down,
    PageUp,
    PageDown,
    Top,
    Bottom,
    Open,
    /// A click on `pane`, on the row of its list at `row`, if any.
    Click {
        pane: Pane,
        row: Option<usize>,
    },
    /// The mouse wheel turned a notch `by`, down for 1 and up for -1, over
    /// the pane `over`, if any.
    Wheel {
        over: Option<Pane>,
        by: isize,
    },
    /// Waits for the key that names a folder to go to.
    Go,
    /// Goes to the folder at this place in [`GO_TO`], or nowhere when the
    /// key after `g` names none.
    GoTo(Option<usize>),
    ToggleQuotes,
    /// Shows the messages already read, or folds them again.
    ToggleReadMessages,
    /// Lists the open conversation's links, to open one.
    ShowLinks,
    /// Lists the open conversation's attachments, to save one.
    ShowAttachments,
    /// Answers yes to the question on screen.
    Yes,
    /// Marks the selected conversation, or takes its mark away.
    Mark,
    /// Marks every conversation listed, or takes the marks away when all
    /// of them are.
    MarkAll,
    /// Opens the command line.
    Command,
    /// Opens the command line at `:search`.
    Search,
    /// A key typed while the command line is open.
    Typed(KeyEvent),
    /// Stars the conversation acted on, or takes its star away.
    Star,
    /// Marks the conversation acted on read.
    Read,
    /// Marks the conversation acted on unread.
    Unread,
    Archive,
    /// Moves the conversation acted on to Trash.
    Trash,
    /// Moves the conversation acted on to Spam.
    Spam,
    /// Moves the conversation acted on to the Inbox, out of Spam as well.
    Inbox,
    /// Opens the command line at `:label`.
    Label,
    /// Opens the command line at `:unlabel`, when there is a label to take
    /// off.
    Unlabel,
    /// Opens the command line at `:move`.
    Move,
    /// Takes the last move back.
    Undo,
    /// Asks whether to delete the conversation acted on forever.
    DeleteForever,
    /// Opens a new message to write.
    Compose,
    /// Opens an answer to the conversation the reader shows.
    Answer(AnswerKind),
    /// A key typed while a message is being written.
    Edit(KeyEvent),
    /// Text pasted whole, where the terminal marks pastes.
    Pasted(String),
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
    /// Where the attachment `name` was saved, or why it was not.
    AttachmentSaved {
        name: String,
        result: Result<String, String>,
    },
    Changed {
        change: Change,
        /// The conversations it was made to.
        ids: Vec<String>,
        /// The label ID of the place they were listed in.
        place: String,
        result: Result<(), String>,
    },
    /// Whether the place named `name` was emptied.
    Emptied {
        name: String,
        result: Result<(), String>,
    },
    /// How Proton would protect the message the send numbered `request`
    /// asks about, for each of its recipients.
    Protected {
        request: u64,
        checks: Vec<(String, Result<RecipientProtection, String>)>,
    },
    /// Whether the message went.
    Sent(Result<(), SendFailure>),
    /// The file a path named, checked to go with the message being written,
    /// and its size, or what keeps it from going.
    Attached {
        request: u64,
        result: Result<(PathBuf, u64), String>,
    },
    /// The names the path `typed` may go on as.
    Completions {
        request: u64,
        typed: String,
        matches: Vec<String>,
    },
    /// A task stopped on a bug, saying what it was.
    Failed(String),
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
    /// Page `page` of what Proton's search finds for `opts`, numbered as a
    /// list.
    Search {
        request: u64,
        opts: SearchOpts,
        page: u32,
    },
    /// What the words of `opts` find in the local index, narrowed by its
    /// filters, all of it at once.
    LocalSearch {
        request: u64,
        opts: SearchOpts,
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
    SaveAttachment {
        message_id: String,
        attachment_id: String,
        name: String,
    },
    Change {
        ids: Vec<String>,
        place: String,
        change: Change,
    },
    /// Deletes forever everything in the place `place`, named `name`.
    Empty {
        place: String,
        name: String,
    },
    /// Looks up how Proton would protect a message to each of `addresses`.
    Protection {
        request: u64,
        addresses: Vec<String>,
    },
    Send(Outgoing),
    /// Checks the file the path `typed` names to go with the message, beside
    /// the files `attached` already.
    Attach {
        request: u64,
        typed: String,
        attached: Vec<PathBuf>,
    },
    /// Finds the names the path `typed` may go on as.
    Complete {
        request: u64,
        typed: String,
    },
}

impl App {
    pub(crate) fn new(account: String) -> Self {
        Self {
            account,
            focus: Pane::Folders,
            help: false,
            help_scroll: 0,
            help_bottom: Cell::new(0),
            help_rows: Cell::new(1),
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
            scroll: Cell::new(0),
            landing: Cell::new(None),
            in_view: Cell::new(None),
            show_quotes: false,
            show_read: false,
            toggled: Vec::new(),
            marks_read: true,
            picking_link: None,
            picking_attachment: None,
            question: None,
            going: false,
            compose: None,
            sender: String::new(),
            send_request: 0,
            file_request: 0,
            command_line: None,
            history: Vec::new(),
            search: None,
            marked: Vec::new(),
            undo: None,
            reader_bottom: Cell::new(0),
            reader_rows: Cell::new(1),
            drawn: Cell::new(Drawn::default()),
            notice: Some("Loading…".to_owned()),
            list_request: 0,
            reloading: false,
            said_once_loaded: None,
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
            Message::ToggleHelp | Message::Back if self.help => self.help = false,
            Message::ToggleHelp => self.open_help(),
            // The open list of keys covers the panes, so they keep still,
            // and scrolls where it does not fit.
            _ if self.help && is_key(&message) => self.scroll_help(&message),
            // So does the list of links, which takes the keys meanwhile.
            _ if self.picking_link.is_some() && is_key(&message) => {
                return self.pick_link(&message);
            }
            _ if self.picking_attachment.is_some() && is_key(&message) => {
                return self.pick_attachment(&message);
            }
            // And a question, which only a yes answers.
            _ if self.question.is_some() && is_key(&message) => return self.answer(&message),
            // Marks go first.
            Message::Back if self.focus == Pane::Conversations && !self.marked.is_empty() => {
                self.marked.clear();
            }
            // Back from a search's results to the place listed before it.
            Message::Back if self.focus == Pane::Conversations && self.search.is_some() => {
                let (id, name) = self
                    .search
                    .as_ref()
                    .map(|search| search.back.clone())
                    .unwrap_or_default();
                self.selected_conversation = 0;
                return vec![self.list(id, name)];
            }
            // Back from the list or the reader to the pane before it; the
            // folders come first, so nothing is before them.
            Message::Back if self.focus != Pane::Folders => {
                self.focus = self.focus.previous();
            }
            Message::Back => {}
            Message::NextPane => self.focus = self.focus.next(),
            Message::PreviousPane => self.focus = self.focus.previous(),
            Message::Left if self.focus != Pane::Folders => {
                self.focus = self.focus.previous();
            }
            Message::Left => {}
            Message::Up => self.step(self.focus, -1),
            Message::Down => return self.down(self.focus),
            Message::PageUp => self.step(self.focus, -self.page()),
            Message::PageDown => self.step(self.focus, self.page()),
            Message::Top => self.step(self.focus, isize::MIN),
            Message::Bottom => self.step(self.focus, isize::MAX),
            Message::Click { pane, row } => return self.click(pane, row),
            Message::Wheel {
                over: Some(pane),
                by,
            } => return self.wheel(pane, by),
            Message::Wheel { over: None, .. } => {}
            // As Proton Mail's Enter opens the message chosen in a conversation.
            Message::Open if self.focus == Pane::Reader => self.toggle_message(),
            Message::Open | Message::Right => return self.open(),
            Message::Go => self.going = true,
            Message::GoTo(to) => {
                self.going = false;
                if let Some(&(_, id, name)) = to.and_then(|at| GO_TO.get(at)) {
                    return self.go_to(id, name);
                }
            }
            Message::ToggleQuotes => self.toggle_quotes(),
            Message::ToggleReadMessages => self.toggle_read_messages(),
            Message::ShowLinks => self.show_links(),
            Message::ShowAttachments => self.show_attachments(),
            // Nothing on screen waits for a yes.
            Message::Yes => {}
            Message::Mark => self.mark(),
            Message::MarkAll => self.mark_all(),
            Message::Command => self.command_line = Some(CommandLine::default()),
            Message::Search => self.command_line = Some(CommandLine::with("search ")),
            Message::Typed(key) => return self.type_in_command_line(&key),
            // Several marked: all starred unless they all are.
            Message::Star => {
                return self.change(|targets| Change::Star(!targets.iter().all(|c| is_starred(c))));
            }
            // In the reader, Proton Mail's `r` answers the message instead.
            Message::Read if self.focus == Pane::Reader => self.open_answer(AnswerKind::Reply),
            Message::Read => return self.change(|_| Change::Read(true)),
            Message::Unread => return self.change(|_| Change::Read(false)),
            Message::Archive => return self.change(|_| Change::Move(label_ids::ARCHIVE.into())),
            Message::Trash => return self.change(|_| Change::Move(label_ids::TRASH.into())),
            Message::Spam => return self.change(|_| Change::Move(label_ids::SPAM.into())),
            Message::Inbox => return self.change(|_| Change::Move(label_ids::INBOX.into())),
            Message::Label => self.command_line = Some(CommandLine::with("label ")),
            Message::Unlabel => self.start_unlabel(),
            Message::Move => self.command_line = Some(CommandLine::with("move ")),
            Message::Undo => return self.take_back(),
            Message::DeleteForever => self.ask_to_delete(),
            Message::Compose => self.compose = Some(Compose::new(self.sender.clone())),
            Message::Answer(how) => self.open_answer(how),
            Message::Edit(key) => return self.edit(&key),
            Message::Pasted(text) => self.paste(&text),
            Message::Refresh => return self.reload(),
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
                    // A list loaded again keeps the selection on the same
                    // conversation; another place's starts where it was put.
                    let selected = self
                        .conversations
                        .get(self.selected_conversation)
                        .filter(|_| self.reloading)
                        .map(|conversation| conversation.id.clone());
                    self.total = total;
                    self.conversations = conversations;
                    self.next_page = 1;
                    // When it has left the list, the row now in its place is.
                    let in_its_place = self
                        .selected_conversation
                        .min(self.conversations.len().saturating_sub(1));
                    self.selected_conversation = selected
                        .and_then(|id| {
                            self.conversations
                                .iter()
                                .position(|conversation| conversation.id == id)
                        })
                        .unwrap_or(in_its_place);
                    self.notice = self.said_once_loaded.take();
                }
                Err(error) => {
                    self.notice = Some(format!("Cannot load {}: {error}", self.listed));
                }
            },
            Message::ConversationLoaded { request, .. } if request != self.read_request => {}
            Message::ConversationLoaded { result, .. } => match result {
                Ok(reading) => {
                    let id = reading.id.clone();
                    // What was listed for the conversation before is not its.
                    self.picking_link = None;
                    self.picking_attachment = None;
                    self.scroll.set(0);
                    self.landing.set(reading.landing());
                    self.toggled.clear();
                    self.reading = Some(reading);
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
            Message::AttachmentSaved {
                name,
                result: Ok(path),
            } => self.notice = Some(format!("Saved {name} as {path}")),
            Message::AttachmentSaved {
                name,
                result: Err(error),
            } => self.notice = Some(format!("Cannot save {name}: {error}")),
            Message::LinkOpened(Ok(())) => {}
            Message::LinkOpened(Err(error)) => {
                self.notice = Some(format!("Cannot open the link: {error}"));
            }
            // Proton owns the counts, and the list may have changed with
            // them: both load again, the selection kept where it can be.
            Message::Changed {
                change,
                ids,
                place,
                result: Ok(()),
            } => {
                self.close_left(&change, &ids);
                self.undo = self.undoable(&change, ids, place);
                return self.reload();
            }
            Message::Changed {
                change,
                ids,
                result: Err(error),
                ..
            } => {
                let (_, failed) = wording(&change, ids.len());
                self.notice = Some(format!("{failed}: {error}"));
            }
            Message::Emptied { result: Ok(()), .. } => {
                self.undo = None;
                return self.reload();
            }
            Message::Emptied {
                name,
                result: Err(error),
            } => self.notice = Some(format!("Cannot empty {name}: {error}")),
            Message::Protected { request, checks } => self.protected(request, checks),
            Message::Sent(result) => return self.sent(result),
            Message::Attached { request, result } if request == self.file_request => {
                if let Some(compose) = self.compose.as_mut() {
                    compose.attached(result);
                }
            }
            Message::Completions {
                request,
                typed,
                matches,
            } if request == self.file_request => {
                if let Some(compose) = self.compose.as_mut() {
                    compose.completed(&typed, matches);
                }
            }
            // Asked for before another, they come too late.
            Message::Attached { .. } | Message::Completions { .. } => {}
            Message::Failed(error) => {
                self.notice = Some(format!("Something went wrong: {error}"));
            }
        }
        Vec::new()
    }

    /// What a terminal event asks for: while the command line is open, the
    /// key typed in it, while a question waits, a yes or a no, after `g`,
    /// the folder to go to, from the mouse, what [`App::pointed`] makes of
    /// it, or else what [`Message::from_event`] makes of it.
    pub(crate) fn message_for(&self, event: &Event) -> Option<Message> {
        match event {
            // Pasted text goes where keys type, and never answers a question.
            Event::Paste(text) => (self.question.is_none()
                && (self.command_line.is_some() || self.compose.is_some()))
            .then(|| Message::Pasted(text.clone())),
            Event::Key(key) if self.command_line.is_some() => {
                (key.kind != KeyEventKind::Release).then_some(Message::Typed(*key))
            }
            Event::Key(key)
                if self.question.is_some()
                    || self.picking_link.is_some_and(|pick| pick.confirming) =>
            {
                (key.kind != KeyEventKind::Release).then(|| answer_of(key))
            }
            Event::Key(key) if self.compose.is_some() => {
                (key.kind != KeyEventKind::Release).then_some(Message::Edit(*key))
            }
            Event::Key(key) if self.going => {
                (key.kind != KeyEventKind::Release).then(|| Message::GoTo(going_to(key)))
            }
            Event::Mouse(mouse) => self.pointed(mouse),
            _ => Message::from_event(event),
        }
    }

    /// What the mouse asks for: a click with its left button on a pane, or
    /// the wheel turned over one, as the view last drew them; nothing while
    /// the keys go to a message being written, a question, the command line
    /// or the folder after `g`.
    fn pointed(&self, mouse: &MouseEvent) -> Option<Message> {
        if self.compose.is_some()
            || self.question.is_some()
            || self.picking_link.is_some_and(|pick| pick.confirming)
            || self.command_line.is_some()
            || self.going
        {
            return None;
        }
        let at = self.drawn.get().at(mouse.column, mouse.row);
        let over = at.map(|(pane, _)| pane);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                at.map(|(pane, row)| Message::Click { pane, row })
            }
            MouseEventKind::ScrollDown => Some(Message::Wheel { over, by: 1 }),
            MouseEventKind::ScrollUp => Some(Message::Wheel { over, by: -1 }),
            _ => None,
        }
    }

    /// What a key does on the command line: Enter runs it, Esc or Ctrl+C
    /// closes it, Backspace on an empty one too; Up and Down recall the
    /// lines run before, Tab completes a word, and Ctrl+U clears it. Ctrl
    /// with Alt types, as Windows reports what AltGr types.
    fn type_in_command_line(&mut self, key: &KeyEvent) -> Vec<Effect> {
        let shortcut = key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::ALT);
        if key.code == KeyCode::Enter {
            let text = self
                .command_line
                .take()
                .map(|line| line.text.trim().to_owned())
                .unwrap_or_default();
            if !text.is_empty() && self.history.last() != Some(&text) {
                self.history.push(text.clone());
            }
            return match Command::parse(&text) {
                Ok(Some(command)) => self.run(command),
                Ok(None) => Vec::new(),
                Err(error) => {
                    self.notice = Some(error);
                    Vec::new()
                }
            };
        }
        let (places, folders, labels, their_labels) = self.completions();
        let Some(line) = self.command_line.as_mut() else {
            return Vec::new();
        };
        match key.code {
            KeyCode::Esc => self.command_line = None,
            KeyCode::Char('c') if shortcut => self.command_line = None,
            KeyCode::Char('u') if shortcut => line.clear(),
            KeyCode::Backspace => {
                if !line.backspace() {
                    self.command_line = None;
                }
            }
            KeyCode::Up => line.recall(&self.history, true),
            KeyCode::Down => line.recall(&self.history, false),
            KeyCode::Tab | KeyCode::BackTab => {
                let backwards = key.code == KeyCode::BackTab;
                let after = |command: &str| match command {
                    "folder" | "in:" => places.clone(),
                    "move" => folders.clone(),
                    "label" => labels.clone(),
                    "unlabel" => their_labels.clone(),
                    _ => Vec::new(),
                };
                line.complete(after, backwards);
            }
            KeyCode::Char(c) if !shortcut => line.type_char(c),
            _ => {}
        }
        Vec::new()
    }

    /// The names Tab offers after `:folder` and a search's `in:`, every
    /// place's, after `:move`, those of the folders a conversation can move
    /// to, after `:label`, those of the labels, and after `:unlabel`, those
    /// of the labels the conversations acted on bear.
    fn completions(&self) -> (Vec<String>, Vec<String>, Vec<String>, Vec<String>) {
        let names = |takes: fn(&Place) -> bool| -> Vec<String> {
            self.places
                .iter()
                .filter(|place| takes(place))
                .map(|place| place.name.clone())
                .collect()
        };
        (
            names(|_| true),
            names(takes_moves),
            names(|place| place.kind == PlaceKind::Label),
            self.label_names(&self.targets()),
        )
    }

    /// The names of the labels any of `conversations` bears, in the order
    /// the folder list shows them.
    pub(crate) fn label_names(&self, conversations: &[&Conversation]) -> Vec<String> {
        self.places
            .iter()
            .filter(|place| place.kind == PlaceKind::Label)
            .filter(|place| {
                conversations.iter().any(|conversation| {
                    conversation.labels.iter().any(|label| label.id == place.id)
                })
            })
            .map(|place| place.name.clone())
            .collect()
    }

    /// The names of the labels `conversation` bears, as its row in the list
    /// names them: all but the one listed, which every row there bears.
    pub(crate) fn row_labels(&self, conversation: &Conversation) -> Vec<String> {
        self.places
            .iter()
            .filter(|place| place.kind == PlaceKind::Label && place.id != self.listed_id)
            .filter(|place| conversation.labels.iter().any(|label| label.id == place.id))
            .map(|place| place.name.clone())
            .collect()
    }

    /// Opens the command line at `:unlabel` for the conversations acted on,
    /// or says why not, as when they bear no label.
    fn start_unlabel(&mut self) {
        if let Some(why) = self.cannot_act() {
            self.notice = Some(why.to_owned());
        } else if self.label_names(&self.targets()).is_empty() {
            self.notice = Some("There is no label to take off.".to_owned());
        } else {
            self.command_line = Some(CommandLine::with("unlabel "));
        }
    }

    /// Does what a command line asks for.
    fn run(&mut self, command: Command) -> Vec<Effect> {
        match command {
            Command::Quit => self.quit = true,
            Command::Help => self.open_help(),
            Command::Attachments => self.show_attachments(),
            Command::Refresh => return self.update(Message::Refresh),
            Command::Search(words) => match self.search_options("search", &words) {
                Ok(opts) => {
                    self.focus = Pane::Conversations;
                    self.selected_conversation = 0;
                    return vec![self.search_for(words, Query::Proton(opts))];
                }
                Err(error) => self.notice = Some(error),
            },
            Command::Local(words) => match self.search_options("local", &words) {
                // The index finds by words; the filters narrow what it finds.
                Ok(SearchOpts { keyword: None, .. }) => {
                    self.notice = Some(
                        "The local index cannot search by filters alone: type words too".into(),
                    );
                }
                Ok(opts) => {
                    self.focus = Pane::Conversations;
                    self.selected_conversation = 0;
                    return vec![self.search_for(words, Query::Local(opts))];
                }
                Err(error) => self.notice = Some(error),
            },
            Command::Folder(name) => match self.named(&name) {
                Ok(index) => {
                    self.selected_place = index;
                    self.focus = Pane::Folders;
                    return self.open();
                }
                Err(error) => self.notice = Some(error),
            },
            Command::Move(name) => match self.named(&name) {
                Ok(index) if takes_moves(&self.places[index]) => {
                    let id = self.places[index].id.clone();
                    return self.change(|_| Change::Move(id));
                }
                Ok(index) => {
                    let name = &self.places[index].name;
                    self.notice = Some(format!("Conversations cannot be moved to {name}"));
                }
                Err(error) => self.notice = Some(error),
            },
            Command::Star(starred) => return self.change(|_| Change::Star(starred)),
            Command::Read(read) => return self.change(|_| Change::Read(read)),
            Command::Archive => return self.change(|_| Change::Move(label_ids::ARCHIVE.into())),
            Command::Trash => return self.change(|_| Change::Move(label_ids::TRASH.into())),
            Command::Spam => return self.change(|_| Change::Move(label_ids::SPAM.into())),
            Command::NotSpam => return self.change(|_| Change::Move(label_ids::INBOX.into())),
            Command::Undo => return self.take_back(),
            Command::Label(name) => match self.label_named(&name) {
                Ok(id) => return self.change(|_| Change::Label(id)),
                Err(error) => self.notice = Some(error),
            },
            Command::Unlabel(name) => match self.label_named(&name) {
                // Proton would take nothing off, and the status bar say it did.
                Ok(id)
                    if self.cannot_act().is_none()
                        && !self.targets().iter().any(|conversation| {
                            conversation.labels.iter().any(|label| label.id == id)
                        }) =>
                {
                    self.notice = Some(format!("No conversation chosen has the label {name}."));
                }
                Ok(id) => return self.change(|_| Change::Unlabel(id)),
                Err(error) => self.notice = Some(error),
            },
            Command::Delete => self.ask_to_delete(),
            Command::Empty => self.ask_to_empty(),
            Command::Compose => return self.update(Message::Compose),
            Command::Answer(how) => self.open_answer(how),
        }
        Vec::new()
    }

    /// What `words`, typed after the command `command`, `search` or `local`,
    /// ask a search for, with the folder named after `in:` by its label ID.
    fn search_options(&self, command: &str, words: &str) -> Result<SearchOpts, String> {
        let mut opts = super::command::search_options(command, words)?;
        if let Some(folder) = opts.folder.take() {
            opts.folder = Some(self.folder_id(&folder)?);
        }
        Ok(opts)
    }

    /// The label ID `folder`, typed after `in:`, names among the places, by
    /// core's rule for `messages search --folder`: a system folder by its
    /// name, `all` for All mail, a label by its ID, or else a folder or label
    /// by its path, such as `Taxes/2024`, or its name, whatever the case. A
    /// name several share, or none has, is refused before anything is asked.
    fn folder_id(&self, folder: &str) -> Result<String, String> {
        // Core knows the system folders among them by name before it looks.
        let places: Vec<Label> = self
            .places
            .iter()
            .map(|place| Label {
                id: place.id.clone(),
                name: place.name.clone(),
                path: place.path.clone(),
                ..Label::default()
            })
            .collect();
        folder_id_among(&places, folder).map_err(|error| match error {
            Error::Ambiguous(_) => {
                let wanted = folder.to_lowercase();
                let paths: Vec<&str> = self
                    .places
                    .iter()
                    .filter(|place| place.name.to_lowercase() == wanted)
                    .map(|place| place.path.as_str())
                    .collect();
                let paths = paths.join(", ");
                format!("Several folders or labels are named {folder}: {paths}")
            }
            _ => format!("No folder or label is named {folder}"),
        })
    }

    /// The ID of the label named `name`, whatever its case, or what to say
    /// when that names a folder or nothing.
    fn label_named(&self, name: &str) -> Result<String, String> {
        let place = &self.places[self.named(name)?];
        match place.kind {
            PlaceKind::Label => Ok(place.id.clone()),
            PlaceKind::Folder => Err(format!(
                "{} is a folder: :move moves mail to it",
                place.name
            )),
            PlaceKind::System => Err(format!("{} is not a label", place.name)),
        }
    }

    /// Which of the places is named `name`, whatever its case.
    fn named(&self, name: &str) -> Result<usize, String> {
        let wanted = name.to_lowercase();
        let mut named = self
            .places
            .iter()
            .enumerate()
            .filter(|(_, place)| place.name.to_lowercase() == wanted)
            .map(|(index, _)| index);
        match (named.next(), named.next()) {
            (Some(index), None) => Ok(index),
            (None, _) => Err(format!("No folder or label is named {name}")),
            (Some(_), Some(_)) => Err(format!("Several folders or labels are named {name}")),
        }
    }

    /// Marks the selected conversation, for an action on all marked, or
    /// takes its mark away, and moves down a row, as marking several goes.
    fn mark(&mut self) {
        if self.focus != Pane::Conversations {
            return;
        }
        let Some(conversation) = self.conversations.get(self.selected_conversation) else {
            return;
        };
        let id = conversation.id.clone();
        match self.marked.iter().position(|marked| *marked == id) {
            Some(at) => {
                self.marked.remove(at);
            }
            None => self.marked.push(id),
        }
        self.step(Pane::Conversations, 1);
    }

    /// Marks every conversation listed, for an action on all of them, or
    /// takes the marks away when all of them are marked already.
    fn mark_all(&mut self) {
        if self.focus != Pane::Conversations {
            return;
        }
        let all = self
            .conversations
            .iter()
            .all(|conversation| self.marked.contains(&conversation.id));
        self.marked = if all {
            Vec::new()
        } else {
            self.conversations
                .iter()
                .map(|conversation| conversation.id.clone())
                .collect()
        };
    }

    /// Lists the folder with label ID `id`, named `name`, as Enter on it
    /// would, selecting it among the folders when they hold it.
    fn go_to(&mut self, id: &str, name: &str) -> Vec<Effect> {
        if let Some(index) = self.places.iter().position(|place| place.id == id) {
            self.selected_place = index;
        }
        self.focus = Pane::Conversations;
        self.selected_conversation = 0;
        vec![self.list(id.to_owned(), name.to_owned())]
    }

    /// The conversations an action acts on: the marked ones, from the list,
    /// in its order, or else the one `acted_on` names.
    fn targets(&self) -> Vec<&Conversation> {
        if self.focus == Pane::Conversations && !self.marked.is_empty() {
            return self
                .conversations
                .iter()
                .filter(|conversation| self.marked.contains(&conversation.id))
                .collect();
        }
        self.acted_on().into_iter().collect()
    }

    /// The conversation a quick action acts on: the list's selection, or the
    /// one the reader shows while it is listed; none from the folders.
    fn acted_on(&self) -> Option<&Conversation> {
        let id = match self.focus {
            Pane::Folders => return None,
            Pane::Conversations => return self.conversations.get(self.selected_conversation),
            Pane::Reader => &self.reading.as_ref()?.id,
        };
        self.conversations
            .iter()
            .find(|conversation| &conversation.id == id)
    }

    /// Why nothing can be done to the conversations acted on yet, if so.
    fn cannot_act(&self) -> Option<&'static str> {
        // Until its first page comes, the list shows another place's rows.
        if self.focus != Pane::Folders && self.next_page == 0 {
            Some("Wait for the list to load.")
        } else if self.targets().is_empty() {
            Some("Choose a conversation in the list first.")
        } else {
            None
        }
    }

    /// Asks for the change `make` works out for the conversations acted on,
    /// saying so in the status bar meanwhile.
    fn change(&mut self, make: impl FnOnce(&[&Conversation]) -> Change) -> Vec<Effect> {
        if let Some(why) = self.cannot_act() {
            self.notice = Some(why.to_owned());
            return Vec::new();
        }
        let targets = self.targets();
        let change = make(&targets);
        let ids: Vec<String> = targets
            .iter()
            .map(|conversation| conversation.id.clone())
            .collect();
        let (doing, _) = wording(&change, ids.len());
        self.notice = Some(doing);
        vec![Effect::Change {
            ids,
            place: self.listed_id.clone(),
            change,
        }]
    }

    /// What `Z` can take back once `change`, made to the conversations `ids`
    /// listed in `place`, is done: a move out of that place, while it is
    /// listed still, when it is a folder mail lives in. Under a label,
    /// Starred, All mail or a search, as on the desktop, the folder each
    /// conversation came from is not known. Any other change leaves nothing.
    fn undoable(&self, change: &Change, ids: Vec<String>, place: String) -> Option<Undo> {
        let folder = self
            .places
            .iter()
            .any(|listed| listed.id == place && takes_moves(listed));
        match change {
            Change::Move(to) if *to != place && place == self.listed_id && folder => {
                Some(Undo { ids, from: place })
            }
            _ => None,
        }
    }

    /// Moves the conversations of the last move back to the folder they
    /// left, or says there is no move to take back.
    fn take_back(&mut self) -> Vec<Effect> {
        let Some(Undo { ids, from }) = self.undo.take() else {
            self.notice = Some("There is no move to take back.".to_owned());
            return Vec::new();
        };
        let change = Change::Move(from);
        let (doing, _) = wording(&change, ids.len());
        self.notice = Some(doing);
        vec![Effect::Change {
            ids,
            place: self.listed_id.clone(),
            change,
        }]
    }

    /// Asks whether to delete the conversations acted on forever, which
    /// only Trash and Spam do.
    fn ask_to_delete(&mut self) {
        if !deletes_forever(&self.listed_id) {
            self.notice = Some("Only mail in Trash or Spam can be deleted forever.".to_owned());
            return;
        }
        if let Some(why) = self.cannot_act() {
            self.notice = Some(why.to_owned());
            return;
        }
        let targets = self.targets();
        let question = Question::Delete {
            ids: targets.iter().map(|c| c.id.clone()).collect(),
            subjects: targets.iter().map(|c| c.subject.clone()).collect(),
            place: self.listed_id.clone(),
            name: self.listed.clone(),
        };
        self.question = Some(question);
    }

    /// Asks whether to delete everything in the listed place forever,
    /// which only Trash and Spam do.
    fn ask_to_empty(&mut self) {
        if !deletes_forever(&self.listed_id) {
            self.notice = Some("Only Trash and Spam can be emptied.".to_owned());
            return;
        }
        self.question = Some(Question::Empty {
            place: self.listed_id.clone(),
            name: self.listed.clone(),
        });
    }

    /// What a key does to the question on screen: `y` does what it asks,
    /// and any other key puts it away, nothing deleted, discarded or sent;
    /// a message being written is back as it was.
    fn answer(&mut self, message: &Message) -> Vec<Effect> {
        let Some(question) = self.question.take() else {
            return Vec::new();
        };
        let yes = matches!(message, Message::Yes);
        match question {
            Question::Delete { ids, place, .. } if yes => {
                let change = Change::Delete;
                let (doing, _) = wording(&change, ids.len());
                self.notice = Some(doing);
                vec![Effect::Change { ids, place, change }]
            }
            Question::Empty { place, name } if yes => {
                self.notice = Some(format!("Emptying {name}…"));
                vec![Effect::Empty { place, name }]
            }
            Question::Delete { .. } | Question::Empty { .. } => {
                self.notice = Some("Nothing was deleted.".to_owned());
                Vec::new()
            }
            Question::Discard if yes => {
                self.compose = None;
                Vec::new()
            }
            Question::Send { outgoing, .. } if yes => self.send(outgoing),
            Question::Discard | Question::Send { .. } => Vec::new(),
        }
    }

    /// Types `text`, pasted whole, where the keys type: on the command line
    /// as one line, and in the message being written as its field takes it.
    fn paste(&mut self, text: &str) {
        if let Some(line) = self.command_line.as_mut() {
            for c in text.chars() {
                if c.is_whitespace() {
                    line.type_char(' ');
                } else if !crate::terminal::is_filtered(c) {
                    line.type_char(c);
                }
            }
        } else if let Some(compose) = self.compose.as_mut() {
            compose.paste(text);
        }
    }

    /// What a key does to the message being written: it types in it or
    /// moves through it, or closes it, asking first when something is
    /// written, or asks whether to send it.
    fn edit(&mut self, key: &KeyEvent) -> Vec<Effect> {
        let Some(compose) = self.compose.as_mut() else {
            return Vec::new();
        };
        match compose.key(key) {
            Asked::Nothing => {}
            Asked::Close if compose.untouched() => self.compose = None,
            Asked::Close => self.question = Some(Question::Discard),
            Asked::Send => return self.ask_to_send(),
            Asked::Attach(typed) => {
                let attached = compose
                    .attached
                    .iter()
                    .map(|(path, _)| path.clone())
                    .collect();
                self.file_request += 1;
                return vec![Effect::Attach {
                    request: self.file_request,
                    typed,
                    attached,
                }];
            }
            Asked::Complete(typed) => {
                self.file_request += 1;
                return vec![Effect::Complete {
                    request: self.file_request,
                    typed,
                }];
            }
        }
        Vec::new()
    }

    /// Asks whether to send the message being written, once it can go,
    /// looking up meanwhile how Proton would protect it for each of its
    /// recipients; or says why it cannot go yet.
    fn ask_to_send(&mut self) -> Vec<Effect> {
        let Some(compose) = self.compose.as_mut() else {
            return Vec::new();
        };
        match compose.ready() {
            Err(why) => {
                compose.notice = Some(why);
                Vec::new()
            }
            Ok(outgoing) => {
                self.send_request += 1;
                // One request each; the question counts the rest.
                let addresses: Vec<String> = outgoing
                    .recipients()
                    .into_iter()
                    .take(PROTECTION_LOOKUP_LIMIT)
                    .collect();
                let checks = addresses
                    .iter()
                    .map(|address| (address.clone(), Checked::Checking))
                    .collect();
                let files = compose
                    .listed()
                    .into_iter()
                    .filter(|file| !file.left_out)
                    .map(|file| format!("{} ({})", file.name, size_label(file.size)))
                    .collect();
                self.question = Some(Question::Send {
                    request: self.send_request,
                    outgoing,
                    files,
                    checks,
                });
                vec![Effect::Protection {
                    request: self.send_request,
                    addresses,
                }]
            }
        }
    }

    /// Shows how Proton would protect the message about to go for each
    /// recipient looked up, while the question the lookups answer is open.
    fn protected(
        &mut self,
        request: u64,
        answers: Vec<(String, Result<RecipientProtection, String>)>,
    ) {
        let Some(Question::Send {
            request: asked,
            checks,
            ..
        }) = self.question.as_mut()
        else {
            return;
        };
        if *asked != request {
            return;
        }
        for (address, answer) in answers {
            if let Some((_, checked)) = checks.iter_mut().find(|(shown, _)| *shown == address) {
                *checked = answer.map_or(Checked::Unknown, Checked::Known);
            }
        }
    }

    /// Hands `outgoing` over; the message being written waits for the
    /// answer, unchangeable meanwhile.
    fn send(&mut self, outgoing: Outgoing) -> Vec<Effect> {
        if let Some(compose) = self.compose.as_mut() {
            compose.sending = Sending::InFlight;
            compose.notice = None;
        }
        vec![Effect::Send(outgoing)]
    }

    /// Opens an answer to the conversation the reader shows, as `how` says,
    /// or says why there is none to write.
    fn open_answer(&mut self, how: AnswerKind) {
        let Some(reading) = &self.reading else {
            let what = match how {
                AnswerKind::Reply | AnswerKind::ReplyAll => "reply to it",
                AnswerKind::Forward => "forward it",
            };
            self.notice = Some(format!("Open a conversation to {what}."));
            return;
        };
        let Some(answering) = reading.answer.clone() else {
            self.notice = Some("This conversation has nothing to answer.".to_owned());
            return;
        };
        self.compose = Some(Compose::answer(self.sender.clone(), how, *answering));
    }

    /// Once the message went, the editor closes, and the folders and the
    /// list load again, since an answer changes the conversation it answers
    /// and Sent holds it; the status bar says it was sent once they have.
    /// When it did not go, or may not have, the message stays, saying so.
    fn sent(&mut self, result: Result<(), SendFailure>) -> Vec<Effect> {
        let failure = match result {
            Ok(()) => {
                self.compose = None;
                let effects = self.reload();
                self.notice = Some("Sent.".to_owned());
                self.said_once_loaded = Some("Sent.".to_owned());
                return effects;
            }
            Err(failure) => failure,
        };
        if let Some(compose) = self.compose.as_mut() {
            (compose.sending, compose.notice) = match failure {
                SendFailure::Unconfirmed => (Sending::Unconfirmed, None),
                SendFailure::Failed(error) => {
                    (Sending::Writing, Some(format!("Not sent: {error}")))
                }
            };
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

    /// Shows the open conversation's folded quoted passages, or folds them
    /// again, or says why there is nothing to show.
    fn toggle_quotes(&mut self) {
        match &self.reading {
            Some(reading) if !reading.quotes.is_empty() => self.show_quotes = !self.show_quotes,
            Some(_) => {
                self.notice = Some("This conversation has no quoted text to fold.".to_owned());
            }
            None => self.notice = Some("Open a conversation to show its quoted text.".to_owned()),
        }
    }

    /// Shows the open conversation's messages already read, which the reader
    /// folds, or folds them again, those Enter turned included, keeping the
    /// message at the reader's top there; or says why there is nothing to
    /// show.
    fn toggle_read_messages(&mut self) {
        match &self.reading {
            Some(reading) if (0..reading.messages.len()).any(|index| reading.folds(index)) => {
                self.show_read = !self.show_read;
                self.toggled.clear();
                self.landing.set(self.in_view.get());
            }
            Some(_) => {
                self.notice = Some("This conversation has no messages read to fold.".to_owned());
            }
            None => self.notice = Some("Open a conversation to show its messages.".to_owned()),
        }
    }

    /// Folds the message at the reader's top into one line, or shows it,
    /// keeping it there.
    fn toggle_message(&mut self) {
        let Some(index) = self.in_view.get().filter(|_| self.reading.is_some()) else {
            return;
        };
        match self.toggled.iter().position(|toggled| *toggled == index) {
            Some(at) => {
                self.toggled.remove(at);
            }
            None => self.toggled.push(index),
        }
        self.landing.set(Some(index));
    }

    /// Whether the reader folds the open conversation's message at `index`
    /// into one line: one the desktop's reader folds, unless `e` shows them,
    /// the other way when Enter turned it.
    pub(crate) fn folds(&self, index: usize) -> bool {
        let read = self
            .reading
            .as_ref()
            .is_some_and(|reading| reading.folds(index));
        (read && !self.show_read) != self.toggled.contains(&index)
    }

    /// Lists the open conversation's attachments, to save one, or says why
    /// not.
    fn show_attachments(&mut self) {
        match &self.reading {
            Some(reading) if !reading.attachments.is_empty() => {
                self.picking_attachment = Some(0);
            }
            Some(_) => self.notice = Some("This conversation has no attachments.".to_owned()),
            None => {
                self.notice = Some("Open a conversation to list its attachments.".to_owned());
            }
        }
    }

    /// What a key does while the attachments are listed: move through them,
    /// save the selected one, which never replaces a file, or put them away.
    fn pick_attachment(&mut self, message: &Message) -> Vec<Effect> {
        let (Some(mut selected), Some(reading)) = (self.picking_attachment, &self.reading) else {
            self.picking_attachment = None;
            return Vec::new();
        };
        let last = reading.attachments.len().saturating_sub(1);
        match message {
            Message::Up => selected = selected.saturating_sub(1),
            Message::Down => selected = (selected + 1).min(last),
            Message::Top => selected = 0,
            Message::Bottom => selected = last,
            Message::Open => {
                self.picking_attachment = None;
                let Some(attachment) = reading.attachments.get(selected) else {
                    return Vec::new();
                };
                let effect = Effect::SaveAttachment {
                    message_id: attachment.message_id.clone(),
                    attachment_id: attachment.id.clone(),
                    name: attachment.name.clone(),
                };
                self.notice = Some(format!("Saving {}…", attachment.name));
                return vec![effect];
            }
            Message::Back | Message::ShowAttachments => {
                self.picking_attachment = None;
                return Vec::new();
            }
            _ => {}
        }
        self.picking_attachment = Some(selected);
        Vec::new()
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

    /// Moves `pane` down a row, and at the last row of a list with more to
    /// it, asks for its next page, which lands below. While a list's first
    /// page is on its way, the rows shown are from before it, so none follow.
    fn down(&mut self, pane: Pane) -> Vec<Effect> {
        let at_end = self.selected_conversation + 1 >= self.conversations.len();
        // The local index gives all it finds at once.
        let more = self.conversations.len() < usize::try_from(self.total).unwrap_or(usize::MAX)
            && !self
                .search
                .as_ref()
                .is_some_and(|search| matches!(search.query, Query::Local(_)));
        if pane == Pane::Conversations && at_end && more && self.next_page > 0 && !self.loading_more
        {
            self.loading_more = true;
            self.notice = Some(format!("Loading more of {}…", self.listed));
            let (request, page) = (self.list_request, self.next_page);
            return vec![match &self.search {
                Some(Search {
                    query: Query::Proton(opts),
                    ..
                }) => Effect::Search {
                    request,
                    opts: opts.clone(),
                    page,
                },
                _ => Effect::Conversations {
                    request,
                    place: self.listed_id.clone(),
                    page,
                },
            }];
        }
        self.step(pane, 1);
        Vec::new()
    }

    /// What a click on `pane` does, on the row `row` of its list if any:
    /// the pane gets the keys and the row is selected, or opened when it
    /// was selected already in the pane that had them.
    fn click(&mut self, pane: Pane, row: Option<usize>) -> Vec<Effect> {
        let had_keys = self.focus == pane;
        self.focus = pane;
        let (selected, len) = match pane {
            Pane::Folders => (&mut self.selected_place, self.places.len()),
            Pane::Conversations => (&mut self.selected_conversation, self.conversations.len()),
            Pane::Reader => return Vec::new(),
        };
        let Some(row) = row.filter(|row| *row < len) else {
            return Vec::new();
        };
        if had_keys && *selected == row {
            return self.open();
        }
        *selected = row;
        Vec::new()
    }

    /// Scrolls `pane` as the wheel over it turns a notch `by`, leaving the
    /// keys where they are: a list a row, as Up and Down move it, the
    /// reader a few.
    fn wheel(&mut self, pane: Pane, by: isize) -> Vec<Effect> {
        match pane {
            Pane::Reader => self.step(pane, by * WHEEL_ROWS),
            _ if by > 0 => return self.down(pane),
            _ => self.step(pane, by),
        }
        Vec::new()
    }

    /// How many rows a page of the reader moves: as many as fit.
    fn page(&self) -> isize {
        page_of(&self.reader_rows)
    }

    /// Opens the list of keys at its top.
    fn open_help(&mut self) {
        self.help = true;
        self.help_scroll = 0;
    }

    /// Scrolls the open list of keys as the key `message` asks, a row, a
    /// page or to an end, or the wheel, as far as there is to go; another
    /// key does nothing.
    fn scroll_help(&mut self, message: &Message) {
        let by = match message {
            Message::Up => -1,
            Message::Down => 1,
            Message::PageUp => -page_of(&self.help_rows),
            Message::PageDown => page_of(&self.help_rows),
            Message::Top => isize::MIN,
            Message::Bottom => isize::MAX,
            Message::Wheel { by, .. } => by * WHEEL_ROWS,
            _ => return,
        };
        self.help_scroll = scrolled(self.help_scroll, by, self.help_bottom.get());
    }

    /// Moves the selection of `pane`'s list, or the reader, `by` rows, as
    /// far as there is to go.
    fn step(&mut self, pane: Pane, by: isize) {
        let (selected, len) = match pane {
            Pane::Folders => (&mut self.selected_place, self.places.len()),
            Pane::Conversations => (&mut self.selected_conversation, self.conversations.len()),
            Pane::Reader => {
                let bottom = self.reader_bottom.get();
                self.scroll.set(scrolled(self.scroll.get(), by, bottom));
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
                // Another folder's list starts at its top.
                self.selected_conversation = 0;
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
    /// answer still on its way for another one too late. A search shown
    /// before ends.
    fn list(&mut self, id: String, name: String) -> Effect {
        self.search = None;
        let notice = format!("Loading {name}…");
        let request = self.relist(id.clone(), name, notice);
        Effect::Conversations {
            request,
            place: id,
            page: 0,
        }
    }

    /// Asks for the first page of what `query` finds for `words`, typed
    /// after its command, as `list` does for a place, keeping the place
    /// listed before the first search to go back to.
    fn search_for(&mut self, words: String, query: Query) -> Effect {
        let back = match self.search.take() {
            Some(search) => search.back,
            None => (self.listed_id.clone(), self.listed.clone()),
        };
        let (name, notice) = match query {
            Query::Local(_) => (
                format!("Local: {words}"),
                format!("Searching the local index for {words}…"),
            ),
            Query::Proton(_) => (
                format!("Search: {words}"),
                format!("Searching for {words}…"),
            ),
        };
        // All mail holds the results, so marking one unread does it there.
        let id = label_ids::ALL_MAIL.to_owned();
        let request = self.relist(id, name, notice);
        let effect = match &query {
            Query::Local(opts) => Effect::LocalSearch {
                request,
                opts: opts.clone(),
            },
            Query::Proton(opts) => Effect::Search {
                request,
                opts: opts.clone(),
                page: 0,
            },
        };
        self.search = Some(Search { words, query, back });
        effect
    }

    /// Makes the list the place `name`, listed from the label ID `id`, its
    /// first page on its way under a new number, saying `notice` meanwhile.
    fn relist(&mut self, id: String, name: String, notice: String) -> u64 {
        // Marks belong to the rows they were made on.
        self.marked.clear();
        self.list_request += 1;
        self.reloading = id == self.listed_id && name == self.listed;
        // A move is taken back to the place it was made in, listed.
        if !self.reloading {
            self.undo = None;
        }
        self.next_page = 0;
        self.loading_more = false;
        self.notice = Some(notice);
        self.listed = name;
        self.listed_id = id;
        self.list_request
    }

    /// The folders and the list again, a search's results included.
    fn reload(&mut self) -> Vec<Effect> {
        let list = match &self.search {
            Some(search) => {
                let (words, query) = (search.words.clone(), search.query.clone());
                self.search_for(words, query)
            }
            None => {
                let (id, name) = (self.listed_id.clone(), self.listed.clone());
                self.list(id, name)
            }
        };
        vec![Effect::Places, list]
    }

    /// Puts away the conversation the reader shows when `change`, made to
    /// the conversations `ids`, took it out of the place listed, as Proton
    /// does: moved to another folder, deleted forever, or no longer bearing
    /// the label or the star the place lists. The keys go back to the list.
    fn close_left(&mut self, change: &Change, ids: &[String]) {
        let listed = self.listed_id.as_str();
        let left = match change {
            Change::Delete => true,
            // All mail holds what is moved anywhere but Trash and Spam.
            Change::Move(folder) if listed == label_ids::ALL_MAIL => {
                folder == label_ids::TRASH || folder == label_ids::SPAM
            }
            Change::Move(folder) => folder != listed,
            Change::Unlabel(label) => label == listed,
            Change::Star(false) => listed == label_ids::STARRED,
            Change::Star(true) | Change::Read(_) | Change::Label(_) => false,
        };
        if left
            && self
                .reading
                .as_ref()
                .is_some_and(|reading| ids.contains(&reading.id))
        {
            self.reading = None;
            self.scroll.set(0);
            self.picking_link = None;
            self.picking_attachment = None;
            if self.focus == Pane::Reader {
                self.focus = Pane::Conversations;
            }
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

/// Whether conversations can be moved to `place`: a folder of the account's
/// own, or the Inbox, Archive, Spam or Trash, unlike a label or Sent.
fn takes_moves(place: &Place) -> bool {
    match place.kind {
        PlaceKind::Folder => true,
        PlaceKind::Label => false,
        PlaceKind::System => [
            label_ids::INBOX,
            label_ids::ARCHIVE,
            label_ids::SPAM,
            label_ids::TRASH,
        ]
        .contains(&place.id.as_str()),
    }
}

/// How many rows a page moves where `rows` fit: as many, and at least one.
fn page_of(rows: &Cell<u16>) -> isize {
    isize::try_from(rows.get()).map_or(1, |rows| rows.max(1))
}

/// Where a view scrolled down `scroll` rows goes `by` rows on, between its
/// top and `bottom`, the furthest it can go.
fn scrolled(scroll: u16, by: isize, bottom: u16) -> u16 {
    let scroll = usize::from(scroll).saturating_add_signed(by);
    u16::try_from(scroll.min(usize::from(bottom))).unwrap_or(u16::MAX)
}

/// Whether `conversation` is starred.
fn is_starred(conversation: &Conversation) -> bool {
    conversation
        .labels
        .iter()
        .any(|label| label.id == label_ids::STARRED)
}

/// What the status bar says while `change` is made to `count`
/// conversations, and if it fails.
fn wording(change: &Change, count: usize) -> (String, String) {
    let what = if count == 1 {
        "the conversation".to_owned()
    } else {
        format!("{count} conversations")
    };
    let (doing, cannot) = match change {
        Change::Star(true) => (format!("Starring {what}"), format!("star {what}")),
        Change::Star(false) => (format!("Unstarring {what}"), format!("unstar {what}")),
        Change::Read(true) => (format!("Marking {what} read"), format!("mark {what} read")),
        Change::Read(false) => (
            format!("Marking {what} unread"),
            format!("mark {what} unread"),
        ),
        Change::Move(folder) if folder == label_ids::ARCHIVE => {
            (format!("Archiving {what}"), format!("archive {what}"))
        }
        Change::Move(folder) if folder == label_ids::TRASH => (
            format!("Moving {what} to Trash"),
            format!("move {what} to Trash"),
        ),
        Change::Move(folder) if folder == label_ids::SPAM => (
            format!("Moving {what} to Spam"),
            format!("move {what} to Spam"),
        ),
        Change::Move(folder) if folder == label_ids::INBOX => (
            format!("Moving {what} to the Inbox"),
            format!("move {what} to the Inbox"),
        ),
        Change::Move(_) => (format!("Moving {what}"), format!("move {what}")),
        Change::Label(_) => (format!("Labelling {what}"), format!("label {what}")),
        Change::Unlabel(_) => (format!("Unlabelling {what}"), format!("unlabel {what}")),
        Change::Delete => (
            format!("Deleting {what} forever"),
            format!("delete {what} forever"),
        ),
    };
    (format!("{doing}…"), format!("Cannot {cannot}"))
}

/// What `key` answers to a question: yes for `y` alone, and no, as Esc says
/// it, for any other key, `q` and Ctrl+C included.
fn answer_of(key: &KeyEvent) -> Message {
    let alone = !key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
    if key.code == KeyCode::Char('y') && alone {
        Message::Yes
    } else {
        Message::Back
    }
}

/// Which of [`GO_TO`] `key` names after `g`, if any: a key alone, as Proton
/// Mail's are, whatever its case.
fn going_to(key: &KeyEvent) -> Option<usize> {
    if key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        return None;
    }
    let KeyCode::Char(c) = key.code else {
        return None;
    };
    GO_TO
        .iter()
        .position(|(letter, _, _)| *letter == c.to_ascii_lowercase())
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
            | Message::AttachmentSaved { .. }
            | Message::Changed { .. }
            | Message::Emptied { .. }
            | Message::Protected { .. }
            | Message::Sent(_)
            | Message::Attached { .. }
            | Message::Completions { .. }
            | Message::Failed(_)
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

    /// Proton Mail's keys where it has one for the action, the desktop's
    /// Ctrl+R, Ctrl+F and Delete, and keys Proton leaves free for what only
    /// a terminal needs.
    fn from_key(key: &KeyEvent) -> Option<Self> {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return match key.code {
                KeyCode::Char('c') => Some(Self::Quit),
                KeyCode::Char('r') => Some(Self::Refresh),
                KeyCode::Char('f') => Some(Self::Search),
                KeyCode::Char('k') => Some(Self::Command),
                KeyCode::Char('a') => Some(Self::MarkAll),
                KeyCode::Char('n') => Some(Self::Compose),
                KeyCode::Up => Some(Self::Top),
                KeyCode::Down => Some(Self::Bottom),
                // Many terminals send Ctrl+Backspace as Ctrl+H.
                KeyCode::Backspace | KeyCode::Char('h') => Some(Self::DeleteForever),
                _ => None,
            };
        }
        match key.code {
            KeyCode::Char('q') => Some(Self::Quit),
            KeyCode::Char('?') => Some(Self::ToggleHelp),
            KeyCode::Esc => Some(Self::Back),
            KeyCode::Tab => Some(Self::NextPane),
            KeyCode::BackTab => Some(Self::PreviousPane),
            KeyCode::Right => Some(Self::Right),
            KeyCode::Left => Some(Self::Left),
            KeyCode::Up | KeyCode::Char('k') => Some(Self::Up),
            KeyCode::Down | KeyCode::Char('j') => Some(Self::Down),
            KeyCode::PageUp => Some(Self::PageUp),
            KeyCode::PageDown => Some(Self::PageDown),
            KeyCode::Home => Some(Self::Top),
            KeyCode::End => Some(Self::Bottom),
            KeyCode::Enter => Some(Self::Open),
            KeyCode::Char('g') => Some(Self::Go),
            KeyCode::Char('R') => Some(Self::Answer(AnswerKind::ReplyAll)),
            KeyCode::Char('F') => Some(Self::Answer(AnswerKind::Forward)),
            KeyCode::Char('n') => Some(Self::Compose),
            KeyCode::Char('z') => Some(Self::ToggleQuotes),
            KeyCode::Char('e') => Some(Self::ToggleReadMessages),
            KeyCode::Char('b') => Some(Self::ShowLinks),
            KeyCode::Char('v') => Some(Self::ShowAttachments),
            KeyCode::Char('y') => Some(Self::Yes),
            KeyCode::Char('*') => Some(Self::Star),
            KeyCode::Char('r') => Some(Self::Read),
            KeyCode::Char('u') => Some(Self::Unread),
            KeyCode::Char('a') => Some(Self::Archive),
            KeyCode::Char('s') => Some(Self::Spam),
            KeyCode::Char('i') => Some(Self::Inbox),
            KeyCode::Char('t') | KeyCode::Delete => Some(Self::Trash),
            KeyCode::Char('l') => Some(Self::Label),
            KeyCode::Char('L') => Some(Self::Unlabel),
            KeyCode::Char('m') => Some(Self::Move),
            KeyCode::Char('Z') => Some(Self::Undo),
            KeyCode::Char(' ' | 'x') => Some(Self::Mark),
            KeyCode::Char(':') => Some(Self::Command),
            KeyCode::Char('/') => Some(Self::Search),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;
