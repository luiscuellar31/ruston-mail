//! The command line `:` opens at the foot of the interface: what is typed in
//! it, the lines typed before, completing a word with Tab, and what a line
//! asks for. Doing what it asks is the state's business, in `app`.

/// The commands, in the order Tab offers them.
const COMMANDS: [&str; 12] = [
    "archive", "folder", "help", "move", "q", "quit", "read", "refresh", "star", "trash", "unread",
    "unstar",
];

/// What a command line asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Command {
    Quit,
    Help,
    Refresh,
    /// Open the folder or label with this name.
    Folder(String),
    /// Move the conversation acted on to the folder with this name.
    Move(String),
    Star(bool),
    Read(bool),
    Archive,
    Trash,
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
            "folder" => Self::Folder(needs("the name of a folder or label")?),
            "move" => Self::Move(needs("the name of a folder")?),
            "star" => Self::Star(true),
            "unstar" => Self::Star(false),
            "read" => Self::Read(true),
            "unread" => Self::Read(false),
            "archive" => Self::Archive,
            "trash" => Self::Trash,
            other => return Err(format!("Not a command: :{other}")),
        };
        if !argument.is_empty() && !matches!(command, Self::Folder(_) | Self::Move(_)) {
            return Err(format!(":{name} takes nothing after it"));
        }
        Ok(Some(command))
    }
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

    /// Completes the word being typed: a command's name, or what follows
    /// it, from the words `after` gives for that command, matched whatever
    /// their case. Tab again shows the next word that fits, and `backwards`
    /// the one before.
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
            Some((name, rest)) => (self.text.len() - rest.trim_start().len(), after(name)),
        };
        let typed = self.text[start..].to_lowercase();
        let words: Vec<String> = words
            .into_iter()
            .filter(|word| word.to_lowercase().starts_with(&typed))
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
        assert_eq!(line.text, "unread");
        line.complete(places, false);
        assert_eq!(line.text, "unstar");
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
