//! The command line `:` opens at the foot of the interface: what is typed in
//! it, the lines typed before, completing a word with Tab, and what a line
//! asks for. Doing what it asks is the state's business, in `app`.

use super::compose::AnswerKind;
use ruston_core::SearchOpts;
use ruston_core::mail::read::search_window;

/// The filters `:search` takes beside its words, as `messages search` takes
/// options: who sent it, who got it, its first and last day, and the folder
/// or label it is in.
const FILTERS: [&str; 5] = ["from:", "to:", "after:", "before:", "in:"];

/// The commands, in the order Tab offers them.
const COMMANDS: [&str; 26] = [
    "archive",
    "attachments",
    "compose",
    "delete",
    "empty",
    "folder",
    "forward",
    "help",
    "label",
    "local",
    "move",
    "notspam",
    "q",
    "quit",
    "read",
    "refresh",
    "reply",
    "replyall",
    "search",
    "spam",
    "star",
    "trash",
    "undo",
    "unlabel",
    "unread",
    "unstar",
];

/// What a command line asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Command {
    Quit,
    Help,
    Refresh,
    /// List the open conversation's attachments, to save one.
    Attachments,
    /// Open the folder or label with this name.
    Folder(String),
    /// Move the conversation acted on to the folder with this name.
    Move(String),
    /// Search all mail for these words.
    Search(String),
    /// Search the local index for these words, without a connection.
    Local(String),
    Star(bool),
    Read(bool),
    Archive,
    Trash,
    /// Put the label with this name on the conversations acted on.
    Label(String),
    /// Take the label with this name off them.
    Unlabel(String),
    /// Move them to Spam.
    Spam,
    /// Move them out of Spam, to the Inbox.
    NotSpam,
    /// Take the last move back.
    Undo,
    /// Delete them forever, from Trash or Spam, once asked.
    Delete,
    /// Delete everything in Trash or Spam forever, once asked.
    Empty,
    /// Write a new message.
    Compose,
    /// Answer the conversation the reader shows.
    Answer(AnswerKind),
}

impl Command {
    /// What `line` asks for, nothing for a blank one, or what is wrong
    /// with it, as the status bar says it.
    pub(crate) fn parse(line: &str) -> Result<Option<Self>, String> {
        let line = line.trim();
        let (name, argument) = line
            .split_once(char::is_whitespace)
            .map_or((line, ""), |(name, argument)| (name, argument.trim()));
        let needs = |what: &str| {
            if argument.is_empty() {
                Err(format!(":{name} needs {what}"))
            } else {
                Ok(argument.to_owned())
            }
        };
        let command = match name {
            "" => return Ok(None),
            "q" | "quit" => Self::Quit,
            "help" => Self::Help,
            "refresh" => Self::Refresh,
            "attachments" => Self::Attachments,
            "folder" => Self::Folder(needs("the name of a folder or label")?),
            "move" => Self::Move(needs("the name of a folder")?),
            "search" => Self::Search(needs("the words to look for")?),
            "local" => Self::Local(needs("the words to look for")?),
            "label" => Self::Label(needs("the name of a label")?),
            "unlabel" => Self::Unlabel(needs("the name of a label")?),
            "spam" => Self::Spam,
            "notspam" => Self::NotSpam,
            "undo" => Self::Undo,
            "star" => Self::Star(true),
            "unstar" => Self::Star(false),
            "read" => Self::Read(true),
            "unread" => Self::Read(false),
            "archive" => Self::Archive,
            "trash" => Self::Trash,
            "delete" => Self::Delete,
            "empty" => Self::Empty,
            "compose" => Self::Compose,
            "reply" => Self::Answer(AnswerKind::Reply),
            "replyall" => Self::Answer(AnswerKind::ReplyAll),
            "forward" => Self::Answer(AnswerKind::Forward),
            other => return Err(format!("Not a command: :{other}")),
        };
        let takes_words = matches!(
            command,
            Self::Folder(_)
                | Self::Move(_)
                | Self::Search(_)
                | Self::Local(_)
                | Self::Label(_)
                | Self::Unlabel(_)
        );
        if !argument.is_empty() && !takes_words {
            return Err(format!(":{name} takes nothing after it"));
        }
        Ok(Some(command))
    }
}

/// What `:search` asks Proton for in `typed`: its words, and each filter
/// typed beside them, whatever its case, with what follows it up to a space,
/// or between double quotes to hold spaces; the folder as it is named. The
/// days are checked as core checks them, so a range that ends before it
/// starts is refused, as `messages search` refuses it.
pub(crate) fn search_options(typed: &str) -> Result<SearchOpts, String> {
    let mut opts = SearchOpts::default();
    let mut words = Vec::new();
    let mut rest = typed.trim_start();
    while !rest.is_empty() {
        let filter = FILTERS.into_iter().find(|filter| {
            rest.get(..filter.len())
                .is_some_and(|start| start.eq_ignore_ascii_case(filter))
        });
        let Some(filter) = filter else {
            let (word, after) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            words.push(word);
            rest = after.trim_start();
            continue;
        };
        let after = &rest[filter.len()..];
        let (value, after) = match after.strip_prefix('"') {
            Some(quoted) => quoted.split_once('"').unwrap_or((quoted, "")),
            None => after.split_once(char::is_whitespace).unwrap_or((after, "")),
        };
        let value = value.trim();
        if value.is_empty() {
            return Err(format!(":search needs something after {filter}"));
        }
        let field = match filter {
            "from:" => &mut opts.from,
            "to:" => &mut opts.to,
            "after:" => &mut opts.after,
            "before:" => &mut opts.before,
            // in:
            _ => &mut opts.folder,
        };
        if field.replace(value.to_owned()).is_some() {
            return Err(format!(":search takes {filter} once"));
        }
        rest = after.trim_start();
    }
    if !words.is_empty() {
        opts.keyword = Some(words.join(" "));
    }
    search_window(opts.after.as_deref(), opts.before.as_deref())
        .map_err(|error| format!("Cannot search: {error}"))?;
    Ok(opts)
}

/// The line `:` opens: what is typed, which earlier line Up and Down show,
/// and the words Tab goes through.
#[derive(Debug, Default)]
pub(crate) struct CommandLine {
    pub(crate) text: String,
    /// Which earlier line is shown, counted from the oldest.
    recalled: Option<usize>,
    completing: Option<Completing>,
}

/// The words Tab offers for the word that starts at `start`, and which of
/// them it shows.
#[derive(Debug)]
struct Completing {
    start: usize,
    words: Vec<String>,
    shown: usize,
}

impl CommandLine {
    /// A line that starts with `text` already typed.
    pub(crate) fn with(text: &str) -> Self {
        Self {
            text: text.to_owned(),
            ..Self::default()
        }
    }

    pub(crate) fn type_char(&mut self, c: char) {
        self.text.push(c);
        self.completing = None;
    }

    /// Takes back the last character; false when there was none, which
    /// closes the line.
    pub(crate) fn backspace(&mut self) -> bool {
        self.completing = None;
        self.text.pop().is_some()
    }

    pub(crate) fn clear(&mut self) {
        self.text.clear();
        self.completing = None;
    }

    /// Shows the line typed before the one shown, or the one after it,
    /// from `history`, oldest first; after the newest, an empty line.
    pub(crate) fn recall(&mut self, history: &[String], older: bool) {
        let at = match (self.recalled, older) {
            (None, false) => return,
            (None, true) => history.len().checked_sub(1),
            (Some(at), true) => Some(at.saturating_sub(1)),
            (Some(at), false) => (at + 1 < history.len()).then_some(at + 1),
        };
        if at.is_none() && older {
            return;
        }
        self.recalled = at;
        self.text = at.map_or_else(String::new, |at| history[at].clone());
        self.completing = None;
    }

    /// Completes the word being typed: a command's name, what follows it,
    /// or in a search, the folder after `in:`, from the words `after` gives
    /// for that command or for `in:`, matched whatever their case; a folder
    /// with spaces goes between double quotes. Tab again shows the next word
    /// that fits, and `backwards` the one before.
    pub(crate) fn complete(&mut self, after: impl Fn(&str) -> Vec<String>, backwards: bool) {
        if let Some(completing) = &mut self.completing {
            let count = completing.words.len();
            completing.shown = if backwards {
                (completing.shown + count - 1) % count
            } else {
                (completing.shown + 1) % count
            };
            self.text.truncate(completing.start);
            self.text.push_str(&completing.words[completing.shown]);
            return;
        }
        let (start, words) = match self.text.split_once(' ') {
            None => (0, COMMANDS.map(String::from).to_vec()),
            Some(("search" | "local", _)) => {
                let Some(start) = folder_typed(&self.text) else {
                    return;
                };
                let folders = after("in:").iter().map(|name| quoted(name)).collect();
                (start, folders)
            }
            Some((name, rest)) => (self.text.len() - rest.trim_start().len(), after(name)),
        };
        // A folder after `in:` may start with the quote that holds its spaces.
        let typed = self.text[start..].trim_start_matches('"').to_lowercase();
        let words: Vec<String> = words
            .into_iter()
            .filter(|word| {
                word.trim_start_matches('"')
                    .to_lowercase()
                    .starts_with(&typed)
            })
            .collect();
        if words.is_empty() {
            return;
        }
        let shown = if backwards { words.len() - 1 } else { 0 };
        self.text.truncate(start);
        self.text.push_str(&words[shown]);
        self.completing = Some(Completing {
            start,
            words,
            shown,
        });
    }
}

/// Where the folder after the last `in:` of a search line starts, while it
/// is still being typed: up to the end of the line, with no space in it,
/// or with an opening double quote and no closing one.
fn folder_typed(line: &str) -> Option<usize> {
    // ASCII lowercase keeps every character where it was.
    let lowered = line.to_ascii_lowercase();
    let (at, _) = lowered
        .rmatch_indices("in:")
        .find(|(at, _)| lowered[..*at].ends_with(char::is_whitespace))?;
    let start = at + "in:".len();
    let typed = &line[start..];
    let open = match typed.strip_prefix('"') {
        Some(quoted) => !quoted.contains('"'),
        None => !typed.contains(char::is_whitespace),
    };
    open.then_some(start)
}

/// `name` as a filter takes it: between double quotes when it has spaces.
fn quoted(name: &str) -> String {
    if name.contains(char::is_whitespace) {
        format!("\"{name}\"")
    } else {
        name.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_names_a_command_and_what_it_acts_on() {
        assert_eq!(Command::parse("  "), Ok(None));
        assert_eq!(Command::parse("q"), Ok(Some(Command::Quit)));
        assert_eq!(Command::parse(" quit "), Ok(Some(Command::Quit)));
        assert_eq!(
            Command::parse("folder  My work "),
            Ok(Some(Command::Folder("My work".into())))
        );
        assert_eq!(
            Command::parse("move Archive"),
            Ok(Some(Command::Move("Archive".into())))
        );
        assert_eq!(Command::parse("unstar"), Ok(Some(Command::Star(false))));
        assert_eq!(Command::parse("unread"), Ok(Some(Command::Read(false))));
        assert_eq!(
            Command::parse("search  quarterly report "),
            Ok(Some(Command::Search("quarterly report".into())))
        );
        assert_eq!(
            Command::parse("search"),
            Err(":search needs the words to look for".into())
        );
        assert_eq!(
            Command::parse("local budget 2024"),
            Ok(Some(Command::Local("budget 2024".into())))
        );
        assert_eq!(
            Command::parse("label Family trips"),
            Ok(Some(Command::Label("Family trips".into())))
        );
        assert_eq!(
            Command::parse("unlabel"),
            Err(":unlabel needs the name of a label".into())
        );
        assert_eq!(Command::parse("notspam"), Ok(Some(Command::NotSpam)));
        assert_eq!(Command::parse("undo"), Ok(Some(Command::Undo)));
        assert_eq!(Command::parse("delete"), Ok(Some(Command::Delete)));
        assert_eq!(Command::parse("compose"), Ok(Some(Command::Compose)));
        assert_eq!(
            Command::parse("replyall"),
            Ok(Some(Command::Answer(AnswerKind::ReplyAll)))
        );
        assert_eq!(
            Command::parse("empty trash"),
            Err(":empty takes nothing after it".into())
        );
        assert_eq!(
            Command::parse("folder"),
            Err(":folder needs the name of a folder or label".into())
        );
        assert_eq!(
            Command::parse("star it"),
            Err(":star takes nothing after it".into())
        );
        assert_eq!(
            Command::parse("frobnicate"),
            Err("Not a command: :frobnicate".into())
        );
    }

    #[test]
    fn a_search_takes_filters_beside_its_words_as_messages_search_takes_options() {
        assert_eq!(
            search_options(
                "budget From:alex after:2026-01-01  report before:2026-01-31 \
                 to:sam@example.test in:\"Bank statements\" re:"
            ),
            Ok(SearchOpts {
                keyword: Some("budget report re:".into()),
                from: Some("alex".into()),
                to: Some("sam@example.test".into()),
                after: Some("2026-01-01".into()),
                before: Some("2026-01-31".into()),
                folder: Some("Bank statements".into()),
                ..SearchOpts::default()
            })
        );
        // Filters alone are a search too, and one day can be all of it.
        assert_eq!(
            search_options("in:Work after:2026-01-01 before:2026-01-01").map(|opts| opts.keyword),
            Ok(None)
        );
        assert_eq!(
            search_options("plans before:2026-01-31 after:2026-02-01"),
            Err("Cannot search: invalid date range: 2026-02-01 is after 2026-01-31".into())
        );
        assert_eq!(
            search_options("in:\"\" plans"),
            Err(":search needs something after in:".into())
        );
    }

    #[test]
    fn tab_completes_a_command_then_what_follows_it() {
        let places = |command: &str| match command {
            "folder" => vec!["Inbox".to_owned(), "Invoices".to_owned(), "Work".to_owned()],
            _ => Vec::new(),
        };
        let mut line = CommandLine::default();
        for c in "fo".chars() {
            line.type_char(c);
        }
        line.complete(places, false);
        assert_eq!(line.text, "folder");
        line.type_char(' ');
        line.type_char('i');
        line.complete(places, false);
        assert_eq!(line.text, "folder Inbox");
        // Again, the next that fits, and round to the first.
        line.complete(places, false);
        assert_eq!(line.text, "folder Invoices");
        line.complete(places, false);
        assert_eq!(line.text, "folder Inbox");
        line.complete(places, true);
        assert_eq!(line.text, "folder Invoices");
        // Nothing fits: the line stays as typed.
        line.clear();
        for c in "folder x".chars() {
            line.type_char(c);
        }
        line.complete(places, false);
        assert_eq!(line.text, "folder x");
        // Several commands start alike.
        line.clear();
        line.type_char('u');
        line.complete(places, false);
        assert_eq!(line.text, "undo");
        line.complete(places, false);
        assert_eq!(line.text, "unlabel");
    }

    #[test]
    fn tab_completes_the_folder_after_in_in_a_search() {
        let places = |after: &str| match after {
            "in:" => ["Inbox", "Bank statements", "Work"]
                .map(String::from)
                .to_vec(),
            _ => Vec::new(),
        };
        let completed = |typed: &str| {
            let mut line = CommandLine::with(typed);
            line.complete(places, false);
            line.text
        };
        assert_eq!(completed("search plans in:w"), "search plans in:Work");
        // Between quotes, typed with the first or not, to hold its spaces.
        for typed in ["local plans IN:ba", "local plans IN:\"ba"] {
            assert_eq!(completed(typed), "local plans IN:\"Bank statements\"");
        }
        // Again, the next that fits.
        let mut line = CommandLine::with("search in:");
        line.complete(places, false);
        assert_eq!(line.text, "search in:Inbox");
        line.complete(places, false);
        assert_eq!(line.text, "search in:\"Bank statements\"");
        // The words, and a folder typed whole, stay as they are.
        for typed in [
            "search pla",
            "search in:Work pla",
            "search in:\"Bank statements\"",
            "search login:w",
        ] {
            assert_eq!(completed(typed), typed);
        }
    }

    #[test]
    fn up_and_down_go_through_the_lines_typed_before() {
        let history = ["folder Work".to_owned(), "star".to_owned()];
        let mut line = CommandLine::default();
        line.recall(&history, false);
        assert_eq!(line.text, "");
        line.recall(&history, true);
        assert_eq!(line.text, "star");
        line.recall(&history, true);
        line.recall(&history, true);
        assert_eq!(line.text, "folder Work");
        line.recall(&history, false);
        assert_eq!(line.text, "star");
        line.recall(&history, false);
        assert_eq!(line.text, "");
        // With nothing typed before, Up leaves the line as it is.
        let mut empty = CommandLine::default();
        empty.type_char('q');
        empty.recall(&[], true);
        assert_eq!(empty.text, "q");
        assert!(empty.backspace());
        assert!(!empty.backspace());
    }
}
