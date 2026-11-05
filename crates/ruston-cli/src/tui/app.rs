//! The interface's state and what each key or answer does to it, apart from
//! drawing and from the terminal, so both can be tested without one. What
//! must happen elsewhere, such as loading a page, comes back as an [`Effect`].

use super::command::{Command, CommandLine};
use super::compose::{AnswerKind, Asked, Checked, Compose, Sending};
use super::reading::{Reading, size_label};
use crate::mailbox::{Change, Outgoing, Place, PlaceKind, SendFailure, deletes_forever};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ruston_core::mail::send::PROTECTION_LOOKUP_LIMIT;
use ruston_core::model::enums::label_ids;
use ruston_core::{Conversation, RecipientProtection};
use std::cell::Cell;
use std::path::PathBuf;

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

/// A search whose results the list shows: its words, whether it looked in
/// the local index rather than asking Proton, and the label ID and name of
/// the place listed before it, which Esc lists again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Search {
    pub(crate) words: String,
    pub(crate) local: bool,
    pub(crate) back: (String, String),
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
    /// Waits for the key that names a folder to go to.
    Go,
    /// Goes to the folder at this place in [`GO_TO`], or nowhere when the
    /// key after `g` names none.
    GoTo(Option<usize>),
    ToggleQuotes,
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
    /// Page `page` of what `words` find in all mail, numbered as a list.
    Search {
        request: u64,
        words: String,
        page: u32,
    },
    /// What `words` find in the local index, all of it at once.
    LocalSearch {
        request: u64,
        words: String,
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
            reader_bottom: Cell::new(0),
            reader_rows: Cell::new(1),
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
            Message::ToggleHelp => self.help = !self.help,
            Message::Back if self.help => self.help = false,
            // The open list of keys covers the panes, so they keep still.
            _ if self.help && is_key(&message) => {}
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
            Message::Up => self.step(-1),
            Message::Down => return self.down(),
            Message::PageUp => self.step(-self.page()),
            Message::PageDown => self.step(self.page()),
            Message::Top => self.step(isize::MIN),
            Message::Bottom => self.step(isize::MAX),
            Message::Open | Message::Right => return self.open(),
            Message::Go => self.going = true,
            Message::GoTo(to) => {
                self.going = false;
                if let Some(&(_, id, name)) = to.and_then(|at| GO_TO.get(at)) {
                    return self.go_to(id, name);
                }
            }
            Message::ToggleQuotes => self.toggle_quotes(),
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
                result: Ok(()),
            } => {
                self.close_left(&change, &ids);
                return self.reload();
            }
            Message::Changed {
                change,
                ids,
                result: Err(error),
            } => {
                let (_, failed) = wording(&change, ids.len());
                self.notice = Some(format!("{failed}: {error}"));
            }
            Message::Emptied { result: Ok(()), .. } => return self.reload(),
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
    /// the folder to go to, or else what [`Message::from_event`] makes of it.
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
            _ => Message::from_event(event),
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
                    "folder" => places.clone(),
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

    /// The names Tab offers after `:folder`, every place's, after `:move`,
    /// those of the folders a conversation can move to, after `:label`,
    /// those of the labels, and after `:unlabel`, those of the labels the
    /// conversations acted on bear.
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
            Command::Help => self.help = true,
            Command::Attachments => self.show_attachments(),
            Command::Refresh => return self.update(Message::Refresh),
            Command::Search(words) => {
                self.focus = Pane::Conversations;
                self.selected_conversation = 0;
                return vec![self.search_for(words, false)];
            }
            Command::Local(words) => {
                self.focus = Pane::Conversations;
                self.selected_conversation = 0;
                return vec![self.search_for(words, true)];
            }
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
        self.step(1);
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

    /// Moves down a row, and at the last row of a list with more to it,
    /// asks for its next page, which lands below. While a list's first page
    /// is on its way, the rows shown are from before it, so none follow.
    fn down(&mut self) -> Vec<Effect> {
        let at_end = self.selected_conversation + 1 >= self.conversations.len();
        // The local index gives all it finds at once.
        let more = self.conversations.len() < usize::try_from(self.total).unwrap_or(usize::MAX)
            && !self.search.as_ref().is_some_and(|search| search.local);
        if self.focus == Pane::Conversations
            && at_end
            && more
            && self.next_page > 0
            && !self.loading_more
        {
            self.loading_more = true;
            self.notice = Some(format!("Loading more of {}…", self.listed));
            let (request, page) = (self.list_request, self.next_page);
            return vec![match &self.search {
                Some(search) => Effect::Search {
                    request,
                    words: search.words.clone(),
                    page,
                },
                None => Effect::Conversations {
                    request,
                    place: self.listed_id.clone(),
                    page,
                },
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

    /// Asks for the first page of what `words` find in all mail, or, when
    /// `local`, in the local index, as `list` does for a place, keeping the
    /// place listed before the first search to go back to.
    fn search_for(&mut self, words: String, local: bool) -> Effect {
        let back = match self.search.take() {
            Some(search) => search.back,
            None => (self.listed_id.clone(), self.listed.clone()),
        };
        let (name, notice) = if local {
            (
                format!("Local: {words}"),
                format!("Searching the local index for {words}…"),
            )
        } else {
            (
                format!("Search: {words}"),
                format!("Searching for {words}…"),
            )
        };
        // All mail holds the results, so marking one unread does it there.
        let id = label_ids::ALL_MAIL.to_owned();
        let request = self.relist(id, name, notice);
        self.search = Some(Search {
            words: words.clone(),
            local,
            back,
        });
        if local {
            Effect::LocalSearch { request, words }
        } else {
            Effect::Search {
                request,
                words,
                page: 0,
            }
        }
    }

    /// Makes the list the place `name`, listed from the label ID `id`, its
    /// first page on its way under a new number, saying `notice` meanwhile.
    fn relist(&mut self, id: String, name: String, notice: String) -> u64 {
        // Marks belong to the rows they were made on.
        self.marked.clear();
        self.list_request += 1;
        self.reloading = id == self.listed_id && name == self.listed;
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
                let (words, local) = (search.words.clone(), search.local);
                self.search_for(words, local)
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
            self.scroll = 0;
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
            KeyCode::Char(' ' | 'x') => Some(Self::Mark),
            KeyCode::Char(':') => Some(Self::Command),
            KeyCode::Char('/') => Some(Self::Search),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mailbox::OutgoingKind;
    use ruston_core::model::ConversationLabel;

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn press(app: &mut App, code: KeyCode) -> Vec<Effect> {
        app.message_for(&key(code))
            .map_or_else(Vec::new, |message| app.update(message))
    }

    /// Presses `code` with Ctrl held.
    fn ctrl(app: &mut App, code: KeyCode) -> Vec<Effect> {
        let event = Event::Key(KeyEvent::new(code, KeyModifiers::CONTROL));
        app.message_for(&event)
            .map_or_else(Vec::new, |message| app.update(message))
    }

    /// Opens the command line, types `line` and runs it.
    fn command(app: &mut App, line: &str) -> Vec<Effect> {
        press(app, KeyCode::Char(':'));
        for c in line.chars() {
            press(app, KeyCode::Char(c));
        }
        press(app, KeyCode::Enter)
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
            attachments: Vec::new(),
            answer: None,
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
        ctrl(&mut app, KeyCode::Down);
        assert_eq!(app.scroll, 70);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.scroll, 70);
        press(&mut app, KeyCode::PageUp);
        assert_eq!(app.scroll, 40);
        ctrl(&mut app, KeyCode::Up);
        assert_eq!(app.scroll, 0);
        press(&mut app, KeyCode::Up);
        assert_eq!(app.scroll, 0);
    }

    #[test]
    fn z_shows_quoted_text_and_hides_it_again_or_says_there_is_none() {
        let mut app = loaded();
        press(&mut app, KeyCode::Char('z'));
        assert!(!app.show_quotes);
        assert_eq!(
            app.notice.as_deref(),
            Some("Open a conversation to show its quoted text.")
        );
        app.reading = Some(reading(3));
        press(&mut app, KeyCode::Char('z'));
        assert!(!app.show_quotes);
        assert_eq!(
            app.notice.as_deref(),
            Some("This conversation has no quoted text to fold.")
        );
        app.reading.as_mut().unwrap().quotes = vec![0..1, 2..3];
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
        ctrl(&mut app, KeyCode::Down);
        assert_eq!(app.selected_conversation, 1);
        ctrl(&mut app, KeyCode::Up);
        assert_eq!(app.selected_conversation, 0);
    }

    #[test]
    fn g_then_a_letter_goes_to_a_folder_as_in_proton_mail() {
        let mut app = loaded();
        app.places.push(place(label_ids::TRASH, "Trash"));
        assert!(press(&mut app, KeyCode::Char('g')).is_empty());
        assert!(app.going);
        let effects = press(&mut app, KeyCode::Char('t'));
        assert!(
            matches!(&effects[..], [Effect::Conversations { place, page: 0, .. }] if place == label_ids::TRASH),
            "{effects:?}"
        );
        assert!(!app.going);
        assert_eq!(
            (app.focus, app.listed.as_str()),
            (Pane::Conversations, "Trash")
        );
        assert_eq!(app.selected_place, 2);
        // All mail is not among the folders, and is listed all the same.
        press(&mut app, KeyCode::Char('g'));
        let effects = press(&mut app, KeyCode::Char('m'));
        assert!(
            matches!(&effects[..], [Effect::Conversations { place, .. }] if place == label_ids::ALL_MAIL),
            "{effects:?}"
        );
        assert_eq!((app.listed.as_str(), app.selected_place), ("All mail", 2));
        // A key that names no folder goes nowhere, and does nothing else.
        press(&mut app, KeyCode::Char('g'));
        assert!(press(&mut app, KeyCode::Char('q')).is_empty());
        assert!(!app.quit && !app.going);
        assert_eq!(app.listed, "All mail");
    }

    #[test]
    fn proton_mail_keys_move_between_panes_mark_and_open_commands() {
        let mut app = loaded();
        // Right opens, as Enter does; Left goes back, but not past the
        // folders.
        press(&mut app, KeyCode::Right);
        assert_eq!(app.focus, Pane::Conversations);
        press(&mut app, KeyCode::Left);
        press(&mut app, KeyCode::Left);
        assert_eq!(app.focus, Pane::Folders);
        press(&mut app, KeyCode::Tab);
        app.update(Message::ConversationsLoaded {
            request: 2,
            page: 0,
            result: Ok((2, vec![conversation("a"), conversation("b")])),
        });
        // x marks as Space does; Ctrl+A marks all, then none.
        press(&mut app, KeyCode::Char('x'));
        assert_eq!(app.marked, ["a"]);
        ctrl(&mut app, KeyCode::Char('a'));
        assert_eq!(app.marked, ["a", "b"]);
        ctrl(&mut app, KeyCode::Char('a'));
        assert!(app.marked.is_empty());
        // i moves to the Inbox; m, Ctrl+K and Ctrl+F open the command line.
        let effects = press(&mut app, KeyCode::Char('i'));
        assert!(
            matches!(&effects[..], [Effect::Change { change: Change::Move(to), .. }] if to == label_ids::INBOX),
            "{effects:?}"
        );
        press(&mut app, KeyCode::Char('m'));
        assert_eq!(app.command_line.as_ref().unwrap().text, "move ");
        press(&mut app, KeyCode::Esc);
        ctrl(&mut app, KeyCode::Char('k'));
        assert_eq!(app.command_line.as_ref().unwrap().text, "");
        press(&mut app, KeyCode::Esc);
        ctrl(&mut app, KeyCode::Char('f'));
        assert_eq!(app.command_line.as_ref().unwrap().text, "search ");
    }

    #[test]
    fn another_place_lists_from_its_top_whatever_was_selected_before() {
        let mut app = loaded();
        app.selected_place = 1;
        let effects = press(&mut app, KeyCode::Enter);
        let [Effect::Conversations { request, .. }] = effects[..] else {
            panic!("{effects:?}");
        };
        // Work holds the conversation the Inbox's selection was on, second.
        app.update(Message::ConversationsLoaded {
            request,
            page: 0,
            result: Ok((2, vec![conversation("w"), conversation("a")])),
        });
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
        press(&mut app, KeyCode::Char('b'));
        assert_eq!(
            app.notice.as_deref(),
            Some("Open a conversation to list its links.")
        );
        app.reading = Some(reading(1));
        press(&mut app, KeyCode::Char('b'));
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
        press(&mut app, KeyCode::Char('b'));
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
        // Any other key goes back to the links, one that does nothing
        // elsewhere and q too.
        for other in ['j', 'n', 'q'] {
            assert!(press(&mut app, KeyCode::Char(other)).is_empty());
            assert_eq!(app.picking_link.map(|pick| pick.confirming), Some(false));
            press(&mut app, KeyCode::Enter);
            assert_eq!(app.picking_link, Some(asked));
        }
        assert!(!app.quit);
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
        press(&mut app, KeyCode::Char('b'));
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.picking_link, None);
        assert!(press(&mut app, KeyCode::Char('y')).is_empty());
    }

    #[test]
    fn v_lists_the_attachments_and_enter_saves_the_one_selected() {
        let mut app = loaded();
        press(&mut app, KeyCode::Char('v'));
        assert_eq!(
            app.notice.as_deref(),
            Some("Open a conversation to list its attachments.")
        );
        app.reading = Some(reading(1));
        command(&mut app, "attachments");
        assert_eq!(
            app.notice.as_deref(),
            Some("This conversation has no attachments.")
        );

        let mut attached = reading(1);
        attached.attachments = ["plan.pdf", "budget.xlsx"]
            .iter()
            .enumerate()
            .map(|(index, name)| super::super::reading::ReadingAttachment {
                message_id: "m1".into(),
                id: format!("a{index}"),
                name: (*name).into(),
                size: 1_000,
            })
            .collect();
        app.reading = Some(attached);
        press(&mut app, KeyCode::Char('v'));
        assert_eq!(app.picking_attachment, Some(0));
        // The keys move through the attachments, not the folders.
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        assert_eq!((app.picking_attachment, app.selected_place), (Some(1), 0));
        assert_eq!(
            press(&mut app, KeyCode::Enter),
            [Effect::SaveAttachment {
                message_id: "m1".into(),
                attachment_id: "a1".into(),
                name: "budget.xlsx".into()
            }]
        );
        assert_eq!(app.picking_attachment, None);
        assert_eq!(app.notice.as_deref(), Some("Saving budget.xlsx…"));
        app.update(Message::AttachmentSaved {
            name: "budget.xlsx".into(),
            result: Ok("/home/me/Downloads/budget (1).xlsx".into()),
        });
        assert_eq!(
            app.notice.as_deref(),
            Some("Saved budget.xlsx as /home/me/Downloads/budget (1).xlsx")
        );
        app.update(Message::AttachmentSaved {
            name: "budget.xlsx".into(),
            result: Err("attachment signature could not be verified".into()),
        });
        assert_eq!(
            app.notice.as_deref(),
            Some("Cannot save budget.xlsx: attachment signature could not be verified")
        );
        // Esc puts them away.
        press(&mut app, KeyCode::Char('v'));
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.picking_attachment, None);
    }

    #[test]
    fn a_conversation_that_loads_puts_away_the_links_of_the_one_before() {
        let mut app = loaded();
        let mut linked = reading(1);
        linked.links = vec!["https://example.test/a".into()];
        app.reading = Some(linked);
        press(&mut app, KeyCode::Char('b'));
        app.update(Message::ConversationLoaded {
            request: 0,
            result: Ok(reading(2)),
        });
        assert_eq!(app.picking_link, None);
    }

    #[test]
    fn the_quick_actions_change_the_conversation_acted_on_then_reload() {
        let mut app = loaded();
        app.conversations[1] = unread("b", "folder-1");
        // From the folders, nothing is acted on.
        assert!(press(&mut app, KeyCode::Char('*')).is_empty());
        assert_eq!(
            app.notice.as_deref(),
            Some("Choose a conversation in the list first.")
        );
        app.focus = Pane::Conversations;
        press(&mut app, KeyCode::Down);
        let change = |change: Change| {
            vec![Effect::Change {
                ids: vec!["b".into()],
                place: label_ids::INBOX.into(),
                change,
            }]
        };
        assert_eq!(
            press(&mut app, KeyCode::Char('*')),
            change(Change::Star(true))
        );
        assert_eq!(app.notice.as_deref(), Some("Starring the conversation…"));
        assert_eq!(
            press(&mut app, KeyCode::Char('r')),
            change(Change::Read(true))
        );
        assert_eq!(
            press(&mut app, KeyCode::Char('u')),
            change(Change::Read(false))
        );
        let archive = Change::Move(label_ids::ARCHIVE.into());
        assert_eq!(press(&mut app, KeyCode::Char('a')), change(archive));
        let trash = Change::Move(label_ids::TRASH.into());
        assert_eq!(press(&mut app, KeyCode::Char('t')), change(trash.clone()));
        assert_eq!(press(&mut app, KeyCode::Delete), change(trash.clone()));

        // Done, the folders and the list load again.
        let effects = app.update(Message::Changed {
            change: trash.clone(),
            ids: vec!["b".into()],
            result: Ok(()),
        });
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
        // Meanwhile the rows are the old ones: nothing is acted on.
        assert!(press(&mut app, KeyCode::Char('*')).is_empty());
        assert_eq!(app.notice.as_deref(), Some("Wait for the list to load."));
        app.update(Message::ConversationsLoaded {
            request: 2,
            page: 0,
            result: Ok((2, vec![conversation("a"), unread("b", "folder-1")])),
        });
        app.update(Message::Changed {
            change: trash,
            ids: vec!["b".into()],
            result: Err("offline".into()),
        });
        assert_eq!(
            app.notice.as_deref(),
            Some("Cannot move the conversation to Trash: offline")
        );

        // In the reader, the conversation it shows is acted on.
        app.focus = Pane::Reader;
        app.selected_conversation = 0;
        app.conversations[1].labels.push(ConversationLabel {
            id: label_ids::STARRED.into(),
            ..ConversationLabel::default()
        });
        app.reading = Some(reading(1));
        assert_eq!(
            press(&mut app, KeyCode::Char('*')),
            change(Change::Star(false))
        );
        // In the reader, r is Proton's reply, not marking read.
        assert!(press(&mut app, KeyCode::Char('r')).is_empty());
    }

    #[test]
    fn space_marks_conversations_and_an_action_acts_on_all_marked() {
        let mut app = loaded();
        app.conversations.push(unread("c", "folder-1"));
        app.total = 3;
        app.focus = Pane::Conversations;
        // Space marks and moves down, so several mark in a row.
        press(&mut app, KeyCode::Char(' '));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(app.marked, ["a", "c"]);
        assert_eq!(app.selected_conversation, 2);
        // Space on a marked one takes its mark away.
        press(&mut app, KeyCode::Char('k'));
        press(&mut app, KeyCode::Char('k'));
        press(&mut app, KeyCode::Char(' '));
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(app.marked, ["c", "b"]);

        // Marked: all are marked read, in the list's order.
        assert_eq!(
            press(&mut app, KeyCode::Char('r')),
            [Effect::Change {
                ids: vec!["b".into(), "c".into()],
                place: label_ids::INBOX.into(),
                change: Change::Read(true)
            }]
        );
        assert_eq!(app.notice.as_deref(), Some("Marking 2 conversations read…"));
        // None starred: all are starred.
        let effects = press(&mut app, KeyCode::Char('*'));
        assert!(
            matches!(&effects[..], [Effect::Change { change: Change::Star(true), ids, .. }] if ids.len() == 2),
            "{effects:?}"
        );
        app.update(Message::Changed {
            change: Change::Star(true),
            ids: vec!["b".into(), "c".into()],
            result: Err("offline".into()),
        });
        assert_eq!(
            app.notice.as_deref(),
            Some("Cannot star 2 conversations: offline")
        );
        // From the reader, the open conversation alone is acted on.
        app.focus = Pane::Reader;
        app.reading = Some(reading(1));
        let effects = press(&mut app, KeyCode::Char('a'));
        assert!(
            matches!(&effects[..], [Effect::Change { ids, .. }] if ids == &["b".to_owned()]),
            "{effects:?}"
        );
        // Esc in the list takes the marks away before going back.
        app.focus = Pane::Conversations;
        press(&mut app, KeyCode::Esc);
        assert!(app.marked.is_empty());
        assert_eq!(app.focus, Pane::Conversations);
        // A change made, the list loads again without them.
        press(&mut app, KeyCode::Char(' '));
        app.update(Message::Changed {
            change: Change::Star(true),
            ids: vec!["b".into()],
            result: Ok(()),
        });
        assert!(app.marked.is_empty());
    }

    #[test]
    fn labels_go_on_and_off_by_name_and_spam_goes_both_ways() {
        let mut app = loaded();
        app.places = vec![
            place(label_ids::INBOX, "Inbox"),
            place(label_ids::SPAM, "Spam"),
            Place {
                kind: PlaceKind::Folder,
                ..place("folder-1", "Work")
            },
            Place {
                kind: PlaceKind::Label,
                ..place("label-1", "Family")
            },
        ];
        app.focus = Pane::Conversations;
        let change = |change: Change| {
            vec![Effect::Change {
                ids: vec!["a".into()],
                place: label_ids::INBOX.into(),
                change,
            }]
        };
        assert_eq!(
            command(&mut app, "label family"),
            change(Change::Label("label-1".into()))
        );
        assert_eq!(app.notice.as_deref(), Some("Labelling the conversation…"));
        // Only a label it bears comes off, as Proton reports once it has.
        assert!(command(&mut app, "unlabel Family").is_empty());
        assert_eq!(
            app.notice.as_deref(),
            Some("No conversation chosen has the label Family.")
        );
        app.conversations[0].labels = vec![ConversationLabel {
            id: "label-1".into(),
            ..ConversationLabel::default()
        }];
        assert_eq!(
            command(&mut app, "unlabel Family"),
            change(Change::Unlabel("label-1".into()))
        );
        assert!(command(&mut app, "label Work").is_empty());
        assert_eq!(
            app.notice.as_deref(),
            Some("Work is a folder: :move moves mail to it")
        );
        assert!(command(&mut app, "label Inbox").is_empty());
        assert_eq!(app.notice.as_deref(), Some("Inbox is not a label"));
        // l starts the line at :label, and Tab offers only labels.
        press(&mut app, KeyCode::Char('l'));
        assert_eq!(app.command_line.as_ref().unwrap().text, "label ");
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.command_line.as_ref().unwrap().text, "label Family");
        press(&mut app, KeyCode::Esc);

        // s moves to Spam, and i to the Inbox.
        assert_eq!(
            press(&mut app, KeyCode::Char('s')),
            change(Change::Move(label_ids::SPAM.into()))
        );
        assert_eq!(
            app.notice.as_deref(),
            Some("Moving the conversation to Spam…")
        );
        assert_eq!(
            command(&mut app, "notspam"),
            change(Change::Move(label_ids::INBOX.into()))
        );
        app.focus = Pane::Folders;
        app.selected_place = 1;
        press(&mut app, KeyCode::Enter);
        app.update(Message::ConversationsLoaded {
            request: 2,
            page: 0,
            result: Ok((1, vec![conversation("junk")])),
        });
        assert_eq!(
            press(&mut app, KeyCode::Char('i')),
            [Effect::Change {
                ids: vec!["junk".into()],
                place: label_ids::SPAM.into(),
                change: Change::Move(label_ids::INBOX.into())
            }]
        );
        assert_eq!(
            app.notice.as_deref(),
            Some("Moving the conversation to the Inbox…")
        );
    }

    #[test]
    fn a_row_names_the_labels_of_its_conversation_but_the_one_listed() {
        let mut app = loaded();
        let label = |id: &str, name: &str| Place {
            kind: PlaceKind::Label,
            ..place(id, name)
        };
        app.places = vec![
            place(label_ids::INBOX, "Inbox"),
            label("label-1", "Family"),
            label("label-2", "Travel"),
        ];
        app.conversations[0].labels = [label_ids::INBOX, "label-2", "label-1"]
            .map(|id| ConversationLabel {
                id: id.into(),
                ..ConversationLabel::default()
            })
            .to_vec();
        assert_eq!(app.row_labels(&app.conversations[0]), ["Family", "Travel"]);
        // Every row under Family bears it.
        command(&mut app, "folder Family");
        assert_eq!(app.row_labels(&app.conversations[0]), ["Travel"]);
    }

    #[test]
    fn shift_l_takes_off_one_of_the_labels_the_conversation_bears() {
        let mut app = loaded();
        let label = |id: &str, name: &str| Place {
            kind: PlaceKind::Label,
            ..place(id, name)
        };
        app.places = vec![
            place(label_ids::INBOX, "Inbox"),
            label("label-1", "Family"),
            label("label-2", "Travel"),
            label("label-3", "Work"),
        ];
        press(&mut app, KeyCode::Char('L'));
        assert!(app.command_line.is_none());
        assert_eq!(
            app.notice.as_deref(),
            Some("Choose a conversation in the list first.")
        );
        app.focus = Pane::Conversations;
        press(&mut app, KeyCode::Char('L'));
        assert!(app.command_line.is_none());
        assert_eq!(
            app.notice.as_deref(),
            Some("There is no label to take off.")
        );
        // Tab offers only the labels it bears, in the folder list's order.
        app.conversations[0].labels = ["label-3", label_ids::INBOX, "label-2"]
            .map(|id| ConversationLabel {
                id: id.into(),
                ..ConversationLabel::default()
            })
            .to_vec();
        press(&mut app, KeyCode::Char('L'));
        assert_eq!(app.command_line.as_ref().unwrap().text, "unlabel ");
        let mut offered = Vec::new();
        for _ in 0..3 {
            press(&mut app, KeyCode::Tab);
            offered.push(app.command_line.as_ref().unwrap().text.clone());
        }
        assert_eq!(
            offered,
            ["unlabel Travel", "unlabel Work", "unlabel Travel"]
        );
        assert_eq!(
            press(&mut app, KeyCode::Enter),
            [Effect::Change {
                ids: vec!["a".into()],
                place: label_ids::INBOX.into(),
                change: Change::Unlabel("label-2".into())
            }]
        );
    }

    #[test]
    fn deleting_forever_asks_first_and_only_a_yes_deletes() {
        let mut app = loaded();
        app.places.push(place(label_ids::TRASH, "Trash"));
        app.focus = Pane::Conversations;
        // Nothing outside Trash and Spam is deleted forever.
        assert!(ctrl(&mut app, KeyCode::Backspace).is_empty());
        assert_eq!(app.question, None);
        assert_eq!(
            app.notice.as_deref(),
            Some("Only mail in Trash or Spam can be deleted forever.")
        );
        assert!(command(&mut app, "empty").is_empty());
        assert_eq!(app.question, None);
        assert_eq!(
            app.notice.as_deref(),
            Some("Only Trash and Spam can be emptied.")
        );

        let effects = command(&mut app, "folder trash");
        let [Effect::Conversations { request, .. }] = effects[..] else {
            panic!("{effects:?}");
        };
        let old = |id: &str| Conversation {
            subject: format!("About {id}"),
            ..conversation(id)
        };
        app.update(Message::ConversationsLoaded {
            request,
            page: 0,
            result: Ok((2, vec![old("x"), old("y")])),
        });
        // Ctrl+Backspace asks about the selected conversation; any key but
        // y keeps it.
        assert!(ctrl(&mut app, KeyCode::Backspace).is_empty());
        assert_eq!(
            app.question,
            Some(Question::Delete {
                ids: vec!["x".into()],
                subjects: vec!["About x".into()],
                place: label_ids::TRASH.into(),
                name: "Trash".into(),
            })
        );
        assert!(press(&mut app, KeyCode::Char('n')).is_empty());
        assert_eq!(app.question, None);
        assert_eq!(app.notice.as_deref(), Some("Nothing was deleted."));
        // q too, which quits only once nothing is asked; Ctrl+H asks as
        // Ctrl+Backspace does.
        ctrl(&mut app, KeyCode::Char('h'));
        assert!(app.question.is_some());
        let ctrl_y = Event::Key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL));
        assert!(matches!(app.message_for(&ctrl_y), Some(Message::Back)));
        assert!(press(&mut app, KeyCode::Char('q')).is_empty());
        assert!(!app.quit);
        assert_eq!(app.question, None);
        // With some marked, about all of them; Esc keeps them, marks and
        // all.
        press(&mut app, KeyCode::Char(' '));
        press(&mut app, KeyCode::Char(' '));
        ctrl(&mut app, KeyCode::Backspace);
        assert!(
            matches!(&app.question, Some(Question::Delete { ids, .. }) if ids == &["x", "y"]),
            "{:?}",
            app.question
        );
        assert!(press(&mut app, KeyCode::Esc).is_empty());
        assert_eq!(app.question, None);
        assert_eq!(app.marked, ["x", "y"]);
        // A yes deletes them, and the list loads again.
        assert!(command(&mut app, "delete").is_empty());
        assert_eq!(
            press(&mut app, KeyCode::Char('y')),
            [Effect::Change {
                ids: vec!["x".into(), "y".into()],
                place: label_ids::TRASH.into(),
                change: Change::Delete,
            }]
        );
        assert_eq!(app.question, None);
        assert_eq!(
            app.notice.as_deref(),
            Some("Deleting 2 conversations forever…")
        );
        app.update(Message::Changed {
            change: Change::Delete,
            ids: vec!["x".into(), "y".into()],
            result: Err("offline".into()),
        });
        assert_eq!(
            app.notice.as_deref(),
            Some("Cannot delete 2 conversations forever: offline")
        );
        let effects = app.update(Message::Changed {
            change: Change::Delete,
            ids: vec!["x".into(), "y".into()],
            result: Ok(()),
        });
        assert_eq!(effects[0], Effect::Places);

        // :empty asks about all that is in the place listed.
        assert!(command(&mut app, "empty").is_empty());
        assert_eq!(
            app.question,
            Some(Question::Empty {
                place: label_ids::TRASH.into(),
                name: "Trash".into(),
            })
        );
        assert_eq!(
            press(&mut app, KeyCode::Char('y')),
            [Effect::Empty {
                place: label_ids::TRASH.into(),
                name: "Trash".into(),
            }]
        );
        assert_eq!(app.notice.as_deref(), Some("Emptying Trash…"));
        app.update(Message::Emptied {
            name: "Trash".into(),
            result: Err("offline".into()),
        });
        assert_eq!(app.notice.as_deref(), Some("Cannot empty Trash: offline"));
        let effects = app.update(Message::Emptied {
            name: "Trash".into(),
            result: Ok(()),
        });
        assert!(
            matches!(&effects[..], [Effect::Places, Effect::Conversations { place, .. }] if place == label_ids::TRASH),
            "{effects:?}"
        );

        // A search's results hold mail from anywhere.
        command(&mut app, "search news");
        ctrl(&mut app, KeyCode::Backspace);
        assert_eq!(app.question, None);
        assert_eq!(
            app.notice.as_deref(),
            Some("Only mail in Trash or Spam can be deleted forever.")
        );
    }

    /// Types `text` where the keys go.
    fn write(app: &mut App, text: &str) {
        for c in text.chars() {
            press(app, KeyCode::Char(c));
        }
    }

    #[test]
    fn n_opens_a_new_message_that_takes_every_key_until_it_closes() {
        let mut app = loaded();
        app.sender = "me@example.test".into();
        assert!(press(&mut app, KeyCode::Char('n')).is_empty());
        assert_eq!(app.compose.as_ref().unwrap().from, "me@example.test");
        // q types instead of quitting.
        press(&mut app, KeyCode::Char('q'));
        assert!(!app.quit);
        assert_eq!(app.compose.as_ref().unwrap().to.text(), "q");
        // Written, Esc asks first, and any other key goes back to it.
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.question, Some(Question::Discard));
        press(&mut app, KeyCode::Char('n'));
        assert_eq!(app.question, None);
        assert_eq!(app.compose.as_ref().unwrap().to.text(), "q");
        // Ctrl+C asks as Esc does, and y discards it.
        ctrl(&mut app, KeyCode::Char('c'));
        assert_eq!(app.question, Some(Question::Discard));
        press(&mut app, KeyCode::Char('y'));
        assert!(app.compose.is_none());
        assert!(!app.quit);
        // Untouched, it closes at once; Ctrl+N and :compose open one too.
        ctrl(&mut app, KeyCode::Char('n'));
        press(&mut app, KeyCode::Esc);
        assert!(app.compose.is_none() && app.question.is_none());
        command(&mut app, "compose");
        assert!(app.compose.is_some());
    }

    #[test]
    fn only_the_first_hundred_recipients_are_looked_up_before_sending() {
        let mut app = loaded();
        press(&mut app, KeyCode::Char('n'));
        let addresses: Vec<String> = (0..101).map(|n| format!("p{n}@proton.me")).collect();
        write(&mut app, &addresses.join(", "));
        let effects = ctrl(&mut app, KeyCode::Enter);
        let [
            Effect::Protection {
                addresses: asked, ..
            },
        ] = effects.as_slice()
        else {
            panic!("{effects:?}");
        };
        assert_eq!(asked.as_slice(), &addresses[..PROTECTION_LOOKUP_LIMIT]);
        let Some(Question::Send {
            outgoing, checks, ..
        }) = &app.question
        else {
            panic!("{:?}", app.question);
        };
        assert_eq!(checks.len(), PROTECTION_LOOKUP_LIMIT);
        // The message still goes to every one of them.
        assert_eq!(outgoing.recipients().len(), 101);
    }

    #[test]
    fn a_message_goes_only_after_a_yes_to_who_gets_it_and_how() {
        let mut app = loaded();
        press(&mut app, KeyCode::Char('n'));
        // With nobody to send it to, it says so and asks nothing.
        assert!(ctrl(&mut app, KeyCode::Char('s')).is_empty());
        assert_eq!(app.question, None);
        let said = |app: &App| app.compose.as_ref().unwrap().said().map(str::to_owned);
        assert_eq!(said(&app).as_deref(), Some("Add someone to send this to."));
        write(&mut app, "sam@proton.me, alex@example.test");
        let addresses = vec!["sam@proton.me".to_owned(), "alex@example.test".to_owned()];
        assert_eq!(
            ctrl(&mut app, KeyCode::Enter),
            [Effect::Protection {
                request: 1,
                addresses: addresses.clone(),
            }]
        );
        let Some(Question::Send {
            outgoing, checks, ..
        }) = &app.question
        else {
            panic!("{:?}", app.question);
        };
        assert_eq!(outgoing.to, addresses);
        assert!(
            checks
                .iter()
                .all(|(_, checked)| *checked == Checked::Checking)
        );
        // An answer to a question asked before comes too late.
        app.update(Message::Protected {
            request: 0,
            checks: vec![("sam@proton.me".into(), Ok(RecipientProtection::Unencrypted))],
        });
        app.update(Message::Protected {
            request: 1,
            checks: vec![
                ("sam@proton.me".into(), Ok(RecipientProtection::EndToEnd)),
                ("alex@example.test".into(), Err("offline".into())),
            ],
        });
        let Some(Question::Send { checks, .. }) = &app.question else {
            panic!("{:?}", app.question);
        };
        assert_eq!(
            checks,
            &[
                (
                    "sam@proton.me".to_owned(),
                    Checked::Known(RecipientProtection::EndToEnd)
                ),
                ("alex@example.test".to_owned(), Checked::Unknown),
            ]
        );
        // Any key but y goes back to the message; Ctrl+J asks again.
        press(&mut app, KeyCode::Char('n'));
        assert!(app.question.is_none() && app.compose.is_some());
        let effects = ctrl(&mut app, KeyCode::Char('j'));
        assert!(
            matches!(&effects[..], [Effect::Protection { request: 2, .. }]),
            "{effects:?}"
        );
        let effects = press(&mut app, KeyCode::Char('y'));
        assert!(
            matches!(&effects[..], [Effect::Send(outgoing)] if outgoing.to == addresses),
            "{effects:?}"
        );
        assert_eq!(said(&app).as_deref(), Some("Sending…"));
        // On its way, nothing changes it or closes it.
        press(&mut app, KeyCode::Char('x'));
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.question, None);
        assert_eq!(
            app.compose.as_ref().unwrap().to.text(),
            "sam@proton.me, alex@example.test"
        );

        // Not sent: it stays, saying why.
        app.update(Message::Sent(Err(SendFailure::Failed("offline".into()))));
        assert_eq!(said(&app).as_deref(), Some("Not sent: offline"));
        // Perhaps sent: it says to check Sent, until the message goes.
        ctrl(&mut app, KeyCode::Char('s'));
        press(&mut app, KeyCode::Char('y'));
        app.update(Message::Sent(Err(SendFailure::Unconfirmed)));
        let unconfirmed = Some(super::super::compose::UNCONFIRMED);
        assert_eq!(said(&app).as_deref(), unconfirmed);
        write(&mut app, ",");
        ctrl(&mut app, KeyCode::Char('s'));
        press(&mut app, KeyCode::Char('y'));
        assert_eq!(said(&app).as_deref(), Some("Sending…"));
        // Sent: the editor closes, the folders and the list load again,
        // and the status bar says so before and after.
        let effects = app.update(Message::Sent(Ok(())));
        let [Effect::Places, Effect::Conversations { request, .. }] = effects[..] else {
            panic!("{effects:?}");
        };
        assert!(app.compose.is_none());
        assert_eq!(app.notice.as_deref(), Some("Sent."));
        app.update(Message::ConversationsLoaded {
            request,
            page: 0,
            result: Ok((2, vec![conversation("a"), conversation("b")])),
        });
        assert_eq!(app.notice.as_deref(), Some("Sent."));
    }

    #[test]
    fn a_file_is_checked_before_it_is_attached_and_a_late_answer_is_dropped() {
        let mut app = loaded();
        press(&mut app, KeyCode::Char('n'));
        ctrl(&mut app, KeyCode::Char('a'));
        write(&mut app, "notes");
        assert_eq!(
            press(&mut app, KeyCode::Tab),
            [Effect::Complete {
                request: 1,
                typed: "notes".into(),
            }]
        );
        write(&mut app, ".txt");
        assert_eq!(
            press(&mut app, KeyCode::Enter),
            [Effect::Attach {
                request: 2,
                typed: "notes.txt".into(),
                attached: Vec::new(),
            }]
        );
        // The completion asked for first comes too late.
        app.update(Message::Completions {
            request: 1,
            typed: "notes".into(),
            matches: vec!["notes/".into()],
        });
        let compose = app.compose.as_ref().unwrap();
        assert_eq!(compose.files.as_ref().unwrap().path.text(), "notes.txt");
        app.update(Message::Attached {
            request: 2,
            result: Ok((PathBuf::from("notes.txt"), 12)),
        });
        let compose = app.compose.as_ref().unwrap();
        assert_eq!(compose.attached, [(PathBuf::from("notes.txt"), 12)]);
        // The next check names the files attached already.
        write(&mut app, "more.txt");
        assert_eq!(
            press(&mut app, KeyCode::Enter),
            [Effect::Attach {
                request: 3,
                typed: "more.txt".into(),
                attached: vec![PathBuf::from("notes.txt")],
            }]
        );
    }

    #[test]
    fn a_paste_goes_where_keys_type_and_never_answers_a_question() {
        let mut app = loaded();
        let paste = |text: &str| Event::Paste(text.into());
        // In the list, it does nothing.
        assert!(app.message_for(&paste("q")).is_none());
        press(&mut app, KeyCode::Char(':'));
        let message = app.message_for(&paste("folder\nInbox\u{7}")).unwrap();
        app.update(message);
        assert_eq!(app.command_line.as_ref().unwrap().text, "folder Inbox");
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Char('n'));
        let message = app.message_for(&paste("sam@proton.me")).unwrap();
        app.update(message);
        assert_eq!(app.compose.as_ref().unwrap().to.text(), "sam@proton.me");
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.question, Some(Question::Discard));
        assert!(app.message_for(&paste("y")).is_none());
        assert_eq!(app.question, Some(Question::Discard));
    }

    #[test]
    fn r_shift_r_and_shift_f_answer_the_conversation_the_reader_shows() {
        let mut app = loaded();
        app.sender = "me@example.test".into();
        press(&mut app, KeyCode::Char('R'));
        assert_eq!(
            app.notice.as_deref(),
            Some("Open a conversation to reply to it.")
        );
        press(&mut app, KeyCode::Char('F'));
        assert_eq!(
            app.notice.as_deref(),
            Some("Open a conversation to forward it.")
        );
        let mut open = reading(1);
        open.answer = Some(Box::new(super::super::reading::Answerable {
            message_id: "m1".into(),
            from: None,
            subject: "Plans".into(),
            sender: "Alex Reed".into(),
            to: vec!["alex@example.test".into()],
            cc: vec!["sam@example.test".into()],
            redirected_from: None,
            opened: true,
            files: Vec::new(),
        }));
        app.reading = Some(open);
        // In the list, r marks read; in the reader, it replies.
        app.focus = Pane::Conversations;
        let effects = press(&mut app, KeyCode::Char('r'));
        assert!(
            matches!(
                &effects[..],
                [Effect::Change {
                    change: Change::Read(true),
                    ..
                }]
            ),
            "{effects:?}"
        );
        assert!(app.compose.is_none());
        app.focus = Pane::Reader;
        press(&mut app, KeyCode::Char('r'));
        let kind = |app: &App| app.compose.as_ref().map(|compose| compose.kind.clone());
        assert!(
            matches!(
                kind(&app),
                Some(OutgoingKind::Reply {
                    everyone: false,
                    ..
                })
            ),
            "{:?}",
            kind(&app)
        );
        assert_eq!(app.compose.as_ref().unwrap().from, "me@example.test");
        // Untouched, an answer closes at once.
        press(&mut app, KeyCode::Esc);
        assert!(app.compose.is_none() && app.question.is_none());
        press(&mut app, KeyCode::Char('R'));
        assert!(matches!(
            kind(&app),
            Some(OutgoingKind::Reply { everyone: true, .. })
        ));
        press(&mut app, KeyCode::Esc);
        command(&mut app, "forward");
        assert!(matches!(kind(&app), Some(OutgoingKind::Forward { .. })));
        write(&mut app, "sam@proton.me");
        ctrl(&mut app, KeyCode::Char('s'));
        let effects = press(&mut app, KeyCode::Char('y'));
        assert!(
            matches!(&effects[..], [Effect::Send(outgoing)] if outgoing.kind == OutgoingKind::Forward { message_id: "m1".into(), quote: ruston_core::Quote::Original, leave_out: Vec::new() }),
            "{effects:?}"
        );
    }

    #[test]
    fn sent_lists_a_message_once_it_goes() {
        let mut app = loaded();
        press(&mut app, KeyCode::Char('g'));
        let effects = press(&mut app, KeyCode::Char('e'));
        let [Effect::Conversations { request, .. }] = effects[..] else {
            panic!("{effects:?}");
        };
        app.update(Message::ConversationsLoaded {
            request,
            page: 0,
            result: Ok((0, Vec::new())),
        });
        press(&mut app, KeyCode::Char('n'));
        write(&mut app, "sam@proton.me");
        ctrl(&mut app, KeyCode::Char('s'));
        press(&mut app, KeyCode::Char('y'));
        let effects = app.update(Message::Sent(Ok(())));
        assert!(
            matches!(&effects[..], [Effect::Places, Effect::Conversations { place, .. }] if place == label_ids::SENT),
            "{effects:?}"
        );
        assert_eq!(app.notice.as_deref(), Some("Sent."));
    }

    #[test]
    fn the_reader_puts_away_a_conversation_a_change_took_out_of_the_list() {
        let changed = |app: &mut App, change: Change, id: &str| {
            app.update(Message::Changed {
                change,
                ids: vec![id.into()],
                result: Ok(()),
            });
        };
        let mut app = loaded();
        app.reading = Some(reading(3));
        app.focus = Pane::Reader;
        // Starred, labelled or moved to the Inbox it shows, it stays.
        changed(&mut app, Change::Star(true), "b");
        changed(&mut app, Change::Label("label-1".into()), "b");
        changed(&mut app, Change::Move(label_ids::INBOX.into()), "b");
        // So does it when another conversation leaves.
        changed(&mut app, Change::Move(label_ids::ARCHIVE.into()), "a");
        assert!(app.reading.is_some());
        assert_eq!(app.focus, Pane::Reader);
        // Archived, it goes, and the keys go back to the list.
        changed(&mut app, Change::Move(label_ids::ARCHIVE.into()), "b");
        assert!(app.reading.is_none());
        assert_eq!(app.focus, Pane::Conversations);
        // In All mail, only Trash and Spam take it out.
        let mut app = loaded();
        command(&mut app, "search plans");
        app.reading = Some(reading(3));
        changed(&mut app, Change::Move(label_ids::ARCHIVE.into()), "b");
        assert!(app.reading.is_some());
        changed(&mut app, Change::Move(label_ids::TRASH.into()), "b");
        assert!(app.reading.is_none());
        // Unstarred in Starred, it goes too.
        let mut app = loaded();
        press(&mut app, KeyCode::Char('g'));
        press(&mut app, KeyCode::Char('*'));
        app.reading = Some(reading(3));
        changed(&mut app, Change::Star(false), "b");
        assert!(app.reading.is_none());
    }

    #[test]
    fn a_conversation_that_left_the_list_leaves_the_selection_in_its_place() {
        let mut app = loaded();
        app.focus = Pane::Conversations;
        press(&mut app, KeyCode::Down);
        let refresh = Event::Key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        app.update(Message::from_event(&refresh).unwrap());
        app.update(Message::ConversationsLoaded {
            request: 2,
            page: 0,
            result: Ok((2, vec![conversation("a"), conversation("c")])),
        });
        assert_eq!(app.conversations[app.selected_conversation].id, "c");
        // Another folder starts at the top of its list.
        app.focus = Pane::Folders;
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        app.update(Message::ConversationsLoaded {
            request: 3,
            page: 0,
            result: Ok((2, vec![conversation("w1"), conversation("w2")])),
        });
        assert_eq!(app.selected_conversation, 0);
    }

    #[test]
    fn a_command_line_opens_folders_by_name_and_remembers_what_ran() {
        let mut app = loaded();
        assert_eq!(
            command(&mut app, "folder WORK"),
            [Effect::Conversations {
                request: 2,
                place: "folder-1".into(),
                page: 0
            }]
        );
        assert_eq!((app.focus, app.selected_place), (Pane::Conversations, 1));
        assert!(app.command_line.is_none());
        command(&mut app, "folder Nowhere");
        assert_eq!(
            app.notice.as_deref(),
            Some("No folder or label is named Nowhere")
        );
        command(&mut app, "fly");
        assert_eq!(app.notice.as_deref(), Some("Not a command: :fly"));

        // Up recalls the lines run before, and Tab completes.
        press(&mut app, KeyCode::Char(':'));
        press(&mut app, KeyCode::Up);
        assert_eq!(app.command_line.as_ref().unwrap().text, "fly");
        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Up);
        assert_eq!(app.command_line.as_ref().unwrap().text, "folder WORK");
        press(&mut app, KeyCode::Char('u'));
        let ctrl_u = Event::Key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        app.update(app.message_for(&ctrl_u).unwrap());
        for c in "folder in".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.command_line.as_ref().unwrap().text, "folder Inbox");
        // While it is open, keys type: q does not quit, j does not move.
        press(&mut app, KeyCode::Char('q'));
        assert!(!app.quit);
        assert_eq!(app.command_line.as_ref().unwrap().text, "folder Inboxq");
        press(&mut app, KeyCode::Esc);
        assert!(app.command_line.is_none());
        // Backspace on an empty line closes it, as Ctrl+C does.
        press(&mut app, KeyCode::Char(':'));
        press(&mut app, KeyCode::Backspace);
        assert!(app.command_line.is_none());
        press(&mut app, KeyCode::Char(':'));
        let ctrl_c = Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        app.update(app.message_for(&ctrl_c).unwrap());
        assert!(app.command_line.is_none());
        assert!(!app.quit);
        command(&mut app, "q");
        assert!(app.quit);
    }

    #[test]
    fn altgr_types_on_the_command_line() {
        let mut app = loaded();
        press(&mut app, KeyCode::Char(':'));
        let at = KeyEvent::new(
            KeyCode::Char('@'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        );
        app.update(app.message_for(&Event::Key(at)).unwrap());
        assert_eq!(app.command_line.as_ref().unwrap().text, "@");
    }

    #[test]
    fn commands_make_the_quick_actions_and_move_to_any_folder() {
        let mut app = loaded();
        app.places = vec![
            place(label_ids::INBOX, "Inbox"),
            place(label_ids::SENT, "Sent"),
            Place {
                kind: PlaceKind::Folder,
                ..place("folder-1", "Work")
            },
            Place {
                kind: PlaceKind::Label,
                ..place("label-1", "Family")
            },
        ];
        app.focus = Pane::Conversations;
        let change = |change: Change| {
            vec![Effect::Change {
                ids: vec!["a".into()],
                place: label_ids::INBOX.into(),
                change,
            }]
        };
        assert_eq!(command(&mut app, "star"), change(Change::Star(true)));
        assert_eq!(command(&mut app, "unstar"), change(Change::Star(false)));
        assert_eq!(command(&mut app, "read"), change(Change::Read(true)));
        assert_eq!(command(&mut app, "unread"), change(Change::Read(false)));
        let archive = Change::Move(label_ids::ARCHIVE.into());
        assert_eq!(command(&mut app, "archive"), change(archive));
        let trash = Change::Move(label_ids::TRASH.into());
        assert_eq!(command(&mut app, "trash"), change(trash));
        let work = Change::Move("folder-1".into());
        assert_eq!(command(&mut app, "move work"), change(work));
        assert!(command(&mut app, "move Family").is_empty());
        assert_eq!(
            app.notice.as_deref(),
            Some("Conversations cannot be moved to Family")
        );
        assert!(command(&mut app, "move Sent").is_empty());
        // Tab offers only the folders a conversation can move to.
        press(&mut app, KeyCode::Char(':'));
        for c in "move ".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        let mut offered = Vec::new();
        for _ in 0..3 {
            press(&mut app, KeyCode::Tab);
            offered.push(app.command_line.as_ref().unwrap().text.clone());
        }
        assert_eq!(offered, ["move Inbox", "move Work", "move Inbox"]);
        press(&mut app, KeyCode::Esc);
        command(&mut app, "help");
        assert!(app.help);
    }

    #[test]
    fn a_task_that_stopped_on_a_bug_says_so() {
        let mut app = loaded();
        press(&mut app, KeyCode::Char('?'));
        // An answer, even with the keys open.
        app.update(Message::Failed("the renderer broke".into()));
        assert_eq!(
            app.notice.as_deref(),
            Some("Something went wrong: the renderer broke")
        );
    }

    #[test]
    fn a_search_lists_what_it_finds_and_esc_goes_back_to_the_place() {
        let mut app = loaded();
        press(&mut app, KeyCode::Char('/'));
        assert_eq!(app.command_line.as_ref().unwrap().text, "search ");
        for c in "plans".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        let effects = press(&mut app, KeyCode::Enter);
        assert_eq!(
            effects,
            [Effect::Search {
                request: 2,
                words: "plans".into(),
                page: 0
            }]
        );
        assert_eq!(app.focus, Pane::Conversations);
        assert_eq!(app.listed, "Search: plans");
        assert_eq!(app.notice.as_deref(), Some("Searching for plans…"));
        app.update(Message::ConversationsLoaded {
            request: 2,
            page: 0,
            result: Ok((3, vec![conversation("p1"), conversation("p2")])),
        });
        // More of it loads as a list's does, and Ctrl+R searches again.
        press(&mut app, KeyCode::End);
        assert_eq!(
            press(&mut app, KeyCode::Down),
            [Effect::Search {
                request: 2,
                words: "plans".into(),
                page: 1
            }]
        );
        let refresh = Event::Key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        let effects = app.update(Message::from_event(&refresh).unwrap());
        assert_eq!(
            effects,
            [
                Effect::Places,
                Effect::Search {
                    request: 3,
                    words: "plans".into(),
                    page: 0
                }
            ]
        );
        // A second search goes back to where the first came from.
        command(&mut app, "search notes");
        assert_eq!(app.listed, "Search: notes");
        assert_eq!(
            press(&mut app, KeyCode::Esc),
            [Effect::Conversations {
                request: 5,
                place: label_ids::INBOX.into(),
                page: 0
            }]
        );
        assert_eq!((app.search.clone(), app.focus), (None, Pane::Conversations));
        assert_eq!(app.listed, "Inbox");
    }

    #[test]
    fn a_local_search_lists_all_it_finds_at_once_and_searches_the_index_again() {
        let mut app = loaded();
        assert_eq!(
            command(&mut app, "local budget"),
            [Effect::LocalSearch {
                request: 2,
                words: "budget".into()
            }]
        );
        assert_eq!(app.listed, "Local: budget");
        assert_eq!(
            app.notice.as_deref(),
            Some("Searching the local index for budget…")
        );
        app.update(Message::ConversationsLoaded {
            request: 2,
            page: 0,
            result: Ok((2, vec![conversation("b1"), conversation("b2")])),
        });
        // All of it came: Down at the end asks for nothing more.
        press(&mut app, KeyCode::End);
        assert!(press(&mut app, KeyCode::Down).is_empty());
        let refresh = Event::Key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        assert_eq!(
            app.update(Message::from_event(&refresh).unwrap()),
            [
                Effect::Places,
                Effect::LocalSearch {
                    request: 3,
                    words: "budget".into()
                }
            ]
        );
        press(&mut app, KeyCode::Esc);
        assert_eq!((app.search.clone(), app.listed.as_str()), (None, "Inbox"));
    }

    #[test]
    fn a_change_to_a_search_result_searches_again() {
        let mut app = loaded();
        command(&mut app, "search plans");
        app.update(Message::ConversationsLoaded {
            request: 2,
            page: 0,
            result: Ok((1, vec![conversation("p1")])),
        });
        assert_eq!(
            press(&mut app, KeyCode::Char('*')),
            [Effect::Change {
                ids: vec!["p1".into()],
                place: label_ids::ALL_MAIL.into(),
                change: Change::Star(true)
            }]
        );
        let effects = app.update(Message::Changed {
            change: Change::Star(true),
            ids: vec!["p1".into()],
            result: Ok(()),
        });
        assert_eq!(
            effects,
            [
                Effect::Places,
                Effect::Search {
                    request: 3,
                    words: "plans".into(),
                    page: 0
                }
            ]
        );
        // Opening a folder ends the search.
        app.focus = Pane::Folders;
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.search, None);
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
