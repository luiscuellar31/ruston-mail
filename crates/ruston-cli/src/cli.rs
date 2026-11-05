//! The commands and options `ruston-cli` takes. Clap parses them, and their
//! doc comments are the text `--help` shows.

use clap::{Args, Parser, Subcommand, ValueEnum};
use ruston_core::{SearchOpts, SendOptions};
use std::path::PathBuf;

/// What each exit status means, shown at the end of `--help`. Status 2 is
/// clap's; the others are `ruston_core::error::exit_status`.
const EXIT_STATUS: &str = "Exit status:
  0  Success, also when the reader of the output stops early (as `| head` does)
  1  Any other error
  2  A usage error
  3  Not found
  4  An ambiguous reference, or a conflict Proton reports (409, 422)
  5  A Proton server error
  6  A session that is missing, expired or refused: sign in again";

/// Environment variables that may carry sign-in secrets. Programs the CLI
/// starts, such as a browser for human verification, never inherit them.
pub(crate) const SECRET_ENV_VARS: [&str; 4] = [
    "PROTON_PASSWORD",
    "PROTON_MAILBOX_PASSWORD",
    "PROTON_TOTP",
    "PROTON_HV_TOKEN",
];

/// Ruston Mail command-line client.
#[derive(Debug, Parser)]
#[command(
    name = "ruston-cli",
    version,
    about = "Ruston Mail command-line client",
    after_help = EXIT_STATUS
)]
pub struct Cli {
    /// Session profile to use (one name, no path separators; shared with desktop by default).
    #[arg(long, global = true, default_value = ruston_core::DEFAULT_SESSION_PROFILE, value_parser = parse_profile)]
    pub profile: String,

    /// Write the output as JSON, for scripts, rather than as text.
    #[arg(long, global = true)]
    pub json: bool,

    /// Log to standard error what the CLI does: sign-in steps, cryptography
    /// and every HTTP request. `-v` logs at debug level, `-vv` traces core too,
    /// and `-vvv` traces everything; `RUST_LOG`, when set, decides instead.
    #[arg(short = 'v', long = "verbose", global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Run in offline demo mode using fictional data without contacting Proton;
    /// `RUSTON_DEMO=1` turns it on too.
    #[arg(long, global = true)]
    pub demo: bool,

    /// Without one, a terminal opens the terminal interface; anywhere else
    /// the help is shown, as a usage error.
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// How the terminal interface opens.
#[derive(Debug, Default, Args)]
pub struct TuiArgs {
    /// Leave the conversations you open unread, instead of marking them
    /// read as the desktop does.
    #[arg(long)]
    pub keep_unread: bool,
}

/// What `login` alone takes: where and as which client to sign in, and the
/// secrets a sign-in may ask for. The username and password come from
/// `PROTON_USER` and `PROTON_PASSWORD`, or are asked for.
#[derive(Debug, Args)]
pub struct LoginArgs {
    /// Override the API base URL: https, or plain http to this machine for a
    /// local test server.
    #[arg(long, value_parser = parse_api_url)]
    pub api_url: Option<String>,

    /// The `x-pm-appversion` header to sign in with, instead of the client's.
    #[arg(long)]
    pub app_version: Option<String>,

    // Secrets read from the environment: help names the variable, never its value.
    /// TOTP two-factor code (prompted for when required if omitted).
    #[arg(long, env = "PROTON_TOTP", hide_env_values = true)]
    pub totp: Option<String>,

    /// The mailbox password of an account that has a second password. On a
    /// terminal it is asked for, hidden, when the account needs one and it is
    /// not given.
    #[arg(long, env = "PROTON_MAILBOX_PASSWORD", hide_env_values = true)]
    pub mailbox_password: Option<String>,

    /// A CAPTCHA token solved beforehand, sent as it is instead of opening a
    /// browser to solve one.
    #[arg(long, env = "PROTON_HV_TOKEN", hide_env_values = true)]
    pub captcha_token: Option<String>,

    /// Identify as this official Proton app, with the app-version header and
    /// User-Agent it sends, unless `--app-version` or `--user-agent` gives
    /// another.
    #[arg(long, value_enum)]
    pub client: Option<ClientPreset>,

    /// Override the User-Agent header (default `ruston-cli/<version> (<os>)`).
    #[arg(long)]
    pub user_agent: Option<String>,

    /// Solve the CAPTCHA on verify.proton.me in an isolated Chrome window, or
    /// Chromium, Brave or Edge (throwaway profile); press Enter once verified
    /// and the window is closed.
    #[arg(long)]
    pub captcha_chrome: bool,
}

fn parse_profile(value: &str) -> std::result::Result<String, String> {
    ruston_core::session::validate_profile_name(value).map_err(|error| error.to_string())?;
    Ok(value.to_owned())
}

fn parse_search_date(value: &str) -> std::result::Result<String, String> {
    ruston_core::mail::read::validate_search_date(value)
        .map_err(|_| "expected a date as YYYY-MM-DD".to_owned())?;
    Ok(value.to_owned())
}

/// The largest `--page-size`: the most Ruston itself asks Proton for at once.
/// A page Proton returns short marks the end of a listing, so a page larger
/// than Proton serves would end paging after the first page.
const MAX_PAGE_SIZE: i64 = 100;

/// Page sizes from 1 to [`MAX_PAGE_SIZE`].
fn page_size() -> clap::builder::RangedI64ValueParser<u32> {
    clap::value_parser!(u32).range(1..=MAX_PAGE_SIZE)
}

/// An API address the sign-in may send credentials to: https, or plain http
/// only to this machine, where a local test server listens.
fn parse_api_url(value: &str) -> std::result::Result<String, String> {
    let url = url::Url::parse(value).map_err(|error| format!("not a URL: {error}"))?;
    let this_machine = match url.host() {
        Some(url::Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    };
    match url.scheme() {
        "https" => Ok(value.to_owned()),
        "http" if this_machine => Ok(value.to_owned()),
        _ => Err("expected an https URL, or http to this machine".to_owned()),
    }
}

/// The time now, in Unix seconds, as `--until` takes a time.
pub(crate) fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_secs()).unwrap_or(i64::MAX)
        })
}

/// A Unix time in seconds that has not passed yet.
fn parse_future_time(value: &str) -> std::result::Result<i64, String> {
    let time: i64 = value
        .parse()
        .map_err(|_| "expected a Unix time in seconds".to_owned())?;
    if time <= unix_now() {
        return Err("that time has already passed".to_owned());
    }
    Ok(time)
}

/// `30m`, `2h`, `1d`, `45s` or bare seconds, as a positive number of seconds.
pub(crate) fn parse_duration_secs(value: &str) -> std::result::Result<i64, String> {
    let value = value.trim();
    let split = value
        .find(|c: char| c.is_alphabetic())
        .unwrap_or(value.len());
    let (number, unit) = value.split_at(split);
    let number: i64 = number
        .parse()
        .map_err(|_| format!("bad duration: {value}"))?;
    if number <= 0 {
        return Err(format!("bad duration: {value} (must be more than zero)"));
    }
    let seconds = match unit {
        "" | "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86400,
        other => return Err(format!("bad duration unit: {other}")),
    };
    number
        .checked_mul(seconds)
        .ok_or_else(|| format!("duration too long: {value}"))
}

/// The official apps `--client` can identify the sign-in as.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ClientPreset {
    Web,
    Ios,
    Android,
}

/// How an official client names itself to Proton, in the two headers that
/// say which app and release is asking.
struct ClientIdentity {
    app_version: &'static str,
    user_agent: &'static str,
}

impl ClientPreset {
    /// What a release of this client sends.
    fn identity(self) -> ClientIdentity {
        match self {
            ClientPreset::Web => ClientIdentity {
                app_version: "web-mail@5.0.99.0",
                user_agent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36",
            },
            ClientPreset::Ios => ClientIdentity {
                app_version: "ios-mail@5.4.0",
                user_agent: "ProtonMail/5.4.0 (iOS/17.4; iPhone)",
            },
            ClientPreset::Android => ClientIdentity {
                app_version: "android-mail@4.5.0",
                user_agent: "ProtonMail/4.5.0 (Android/14; OnePlus)",
            },
        }
    }

    /// The `x-pm-appversion` header this client sends, unless
    /// `--app-version` gives another.
    pub fn app_version(self) -> &'static str {
        self.identity().app_version
    }

    /// The `User-Agent` this client sends, unless `--user-agent` gives
    /// another.
    pub fn user_agent(self) -> &'static str {
        self.identity().user_agent
    }
}

/// What command handlers read of the global options. `demo` is also on when
/// `RUSTON_DEMO` asks for it.
pub struct Ctx {
    pub profile: String,
    pub json: bool,
    pub demo: bool,
}

impl From<&Cli> for Ctx {
    fn from(c: &Cli) -> Self {
        let env_demo = std::env::var(ruston_core::DEMO_ENV)
            .is_ok_and(|value| ruston_core::demo_requested(&value));

        Ctx {
            profile: c.profile.clone(),
            json: c.json,
            demo: c.demo || env_demo,
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Log in to Proton Mail. The username and password come from
    /// `PROTON_USER` and `PROTON_PASSWORD`, or are asked for.
    Login(LoginArgs),
    /// End the session at Proton and forget it on this device. A session that
    /// can no longer be resumed is forgotten all the same.
    Logout,
    /// Print the primary address of the account signed in.
    Whoami,
    /// Open the terminal interface: folders, conversations and a reader in
    /// one screen. `ruston-cli` with no command opens it too.
    Tui(TuiArgs),
    /// Message operations.
    Messages {
        #[command(subcommand)]
        cmd: MessagesCmd,
    },
    /// Read and organize conversations (threads).
    Conversations {
        #[command(subcommand)]
        cmd: ConversationsCmd,
    },
    /// List or save the attachments of a message.
    Attachments {
        #[command(subcommand)]
        cmd: AttachmentsCmd,
    },
    /// List, save, edit or delete drafts, without sending anything.
    Drafts {
        #[command(subcommand)]
        cmd: DraftsCmd,
    },
    /// Manage the Sieve filters Proton runs on its servers.
    Filters {
        #[command(subcommand)]
        cmd: FiltersCmd,
    },
    /// Contacts (read).
    Contacts {
        #[command(subcommand)]
        cmd: ContactsCmd,
    },
    /// Address operations.
    Addresses {
        #[command(subcommand)]
        cmd: AddressesCmd,
    },
    /// Show the mail settings, or turn one of a few on or off.
    Settings {
        #[command(subcommand)]
        cmd: SettingsCmd,
    },
    /// Count the messages, or conversations, in each folder and label, and how
    /// many are unread.
    Counts {
        /// Count each conversation once, not each of its messages.
        #[arg(long)]
        conversations: bool,
    },
    /// Export reconstructed messages and attachments to `.eml` files.
    ///
    /// Existing files are never replaced: running it again into the same
    /// directory resumes. Messages that cannot be decrypted are skipped and
    /// listed, and the command then exits with an error.
    Export {
        /// A system folder (all, inbox, sent…), or one of your folders or
        /// labels by its name, path or ID.
        #[arg(long, default_value = "all")]
        folder: String,
        /// Output directory.
        #[arg(long)]
        out: PathBuf,
        #[arg(long, default_value_t = 100)]
        max: u32,
    },
    /// Keep the cache in step with Proton's event stream until stopped. Network
    /// errors, Proton being busy or down, and the cache changing during a sync
    /// are tried again after a wait that doubles, up to 10 minutes; a refused
    /// session, or any other error, stops it.
    Watch {
        #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u64).range(1..))]
        interval: u64,
        /// Backfill this folder once, then update its local search index when mail changes.
        /// `all` includes Trash and Spam; `almost-all` leaves them out.
        #[arg(long)]
        folder: Option<String>,
    },
    /// Build the local full-text index (stores decrypted message bodies in SQLite).
    Index {
        /// Folder to index. The default, `almost-all`, is every folder except
        /// Trash and Spam; `all` includes them.
        #[arg(long, default_value = "almost-all")]
        folder: String,
        #[arg(long = "max-pages", default_value_t = 3)]
        max_pages: u32,
        #[arg(long = "page-size", default_value_t = 50, value_parser = page_size())]
        page_size: u32,
    },
    /// Search the local index that `index` builds, offline, sending nothing to
    /// Proton.
    Search {
        /// The words to find; a message must have every one.
        query: String,
        #[arg(long, default_value_t = 25)]
        limit: u32,
    },
    /// Bring the local cache up to date with Proton's events, and with
    /// `--backfill` cache a folder too.
    Sync {
        /// A folder to cache as well, such as `inbox`, up to `--max-pages`
        /// pages of it.
        #[arg(long)]
        backfill: Option<String>,
        #[arg(long = "max-pages", default_value_t = 4)]
        max_pages: u32,
        #[arg(long = "page-size", default_value_t = 50, value_parser = page_size())]
        page_size: u32,
    },
    /// List, create, change or delete labels and folders.
    Labels {
        #[command(subcommand)]
        cmd: LabelsCmd,
    },
}

/// What `messages search` and `conversations search` look for.
#[derive(Debug, Args)]
pub struct SearchArgs {
    /// Free-text keyword.
    #[arg(long)]
    pub keyword: Option<String>,
    /// Only mail whose sender's address matches this.
    #[arg(long)]
    pub from: Option<String>,
    /// Only mail with a recipient whose address matches this.
    #[arg(long)]
    pub to: Option<String>,
    /// Match the subject.
    #[arg(long)]
    pub subject: Option<String>,
    /// Only results from this day on (YYYY-MM-DD, a day in UTC).
    #[arg(long, value_parser = parse_search_date)]
    pub after: Option<String>,
    /// Only results up to this day, including it (YYYY-MM-DD, a day in UTC).
    #[arg(long, value_parser = parse_search_date)]
    pub before: Option<String>,
    /// Restrict to a folder or label: a system folder, or one of yours by
    /// its name, path or ID.
    #[arg(long)]
    pub folder: Option<String>,
    /// Find unread mail only.
    #[arg(long)]
    pub unread: bool,
    /// Show at most this many results.
    #[arg(long)]
    pub limit: Option<u32>,
}

impl From<SearchArgs> for SearchOpts {
    fn from(args: SearchArgs) -> Self {
        Self {
            keyword: args.keyword,
            from: args.from,
            to: args.to,
            subject: args.subject,
            after: args.after,
            before: args.before,
            folder: args.folder,
            unread: args.unread,
            limit: args.limit,
        }
    }
}

/// What a message holds: everything a saved draft keeps.
#[derive(Debug, Args)]
pub struct DraftArgs {
    /// An address for the To field; repeat the option for more.
    #[arg(long = "to", required = true)]
    pub to: Vec<String>,
    /// An address for the Cc field; repeat the option for more.
    #[arg(long = "cc")]
    pub cc: Vec<String>,
    /// An address for the Bcc field, which no other recipient sees; repeat the
    /// option for more.
    #[arg(long = "bcc")]
    pub bcc: Vec<String>,
    /// Sender address. `messages send` asks for one on a terminal when the
    /// account has several; otherwise a new message comes from the primary
    /// address, and an edited draft from its own.
    #[arg(long)]
    pub from: Option<String>,
    /// Message subject.
    #[arg(long)]
    pub subject: String,
    /// The message's text; `-` reads it from standard input.
    #[arg(long)]
    pub body: String,
    /// Treat the body as HTML. `messages send` may ask on a terminal when the
    /// body looks like HTML.
    #[arg(long)]
    pub html: bool,
    /// A file to attach; repeat the option for more.
    #[arg(long = "attach")]
    pub attach: Vec<PathBuf>,
}

impl DraftArgs {
    /// The message these arguments describe, with `body` as its text: the
    /// body argument itself may only say where to read it from.
    pub fn into_send_options(self, body: String) -> SendOptions {
        SendOptions {
            to: self.to,
            cc: self.cc,
            bcc: self.bcc,
            from: self.from,
            subject: self.subject,
            body,
            html: self.html,
            attachments: self.attach,
            ..SendOptions::default()
        }
    }
}

/// Compose options for `send`: a message, and how it is sent.
#[derive(Debug, Args)]
pub struct SendArgs {
    #[command(flatten)]
    pub message: DraftArgs,
    /// Have Proton send it later, at this Unix time in seconds.
    #[arg(long = "send-at")]
    pub send_at: Option<i64>,
    /// Have the message delete itself this many seconds after it is sent.
    #[arg(long = "expires")]
    pub expires: Option<i64>,
    /// A password that opens the message for recipients without an encryption
    /// key. Without a value, the password is asked for twice, hidden, on the
    /// terminal.
    #[arg(long = "eo-password", num_args = 0..=1, default_missing_value = "")]
    pub eo_password: Option<String>,
    /// A hint for recalling the `--eo-password`, shown to the recipients who
    /// need it.
    #[arg(long = "eo-hint")]
    pub eo_hint: Option<String>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ReadFormat {
    /// Readable text with a header block; HTML becomes Markdown-style text.
    Text,
    /// HTML body with a header block; terminal controls are filtered on a terminal.
    Html,
    /// Raw decrypted body only; terminal controls are filtered on a terminal.
    Raw,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ReadState {
    Read,
    Unread,
}

impl ReadState {
    pub fn as_bool(self) -> bool {
        matches!(self, ReadState::Read)
    }
}

/// Whether `messages label` puts the label on or takes it off.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum LabelAction {
    Add,
    Rm,
}

/// The folders `messages empty` clears. Emptying any other folder or a label
/// would permanently delete mail nobody had thrown away.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum EmptyFolder {
    Trash,
    Spam,
}

impl EmptyFolder {
    /// The folder name core resolves.
    pub fn as_str(self) -> &'static str {
        match self {
            EmptyFolder::Trash => "trash",
            EmptyFolder::Spam => "spam",
        }
    }

    /// The folder as people see it.
    pub fn name(self) -> &'static str {
        match self {
            EmptyFolder::Trash => "Trash",
            EmptyFolder::Spam => "Spam",
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum MessagesCmd {
    /// List a folder's messages, a page at a time.
    List {
        /// A system folder (inbox, sent, archive…), or one of your folders or
        /// labels by its name, path or ID; with --cached, a system folder or
        /// an ID.
        #[arg(long, default_value = "inbox")]
        folder: String,
        #[arg(long, default_value_t = 0)]
        page: u32,
        #[arg(long = "page-size", default_value_t = 25, value_parser = page_size())]
        page_size: u32,
        #[arg(long)]
        unread: bool,
        /// List what the local cache holds, offline, instead of asking Proton.
        #[arg(long)]
        cached: bool,
    },
    /// Search messages.
    Search(SearchArgs),
    /// Show a message: its headers, attachments and body.
    Read {
        /// The message: its ID, or words that find it and no other.
        reference: String,
        #[arg(long, value_enum, default_value_t = ReadFormat::Text)]
        format: ReadFormat,
        /// Show the body alone, without the headers.
        #[arg(long = "body-only")]
        body_only: bool,
        /// Save the body in this file instead of showing it.
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Send a new message.
    Send(SendArgs),
    /// Reply to a message.
    Reply {
        reference: String,
        /// Reply to everyone the message went to, not only its sender.
        #[arg(long)]
        all: bool,
        /// Sender address (defaults to the address the original message reached).
        #[arg(long)]
        from: Option<String>,
        /// The reply's text; `-` reads it from standard input.
        #[arg(long)]
        body: String,
        #[arg(long = "attach")]
        attach: Vec<PathBuf>,
        /// Send the reply without the original quoted under the body.
        #[arg(long)]
        no_quote: bool,
    },
    /// Forward a message.
    Forward {
        reference: String,
        #[arg(long = "to", required = true)]
        to: Vec<String>,
        /// Sender address (defaults to the address the original message reached).
        #[arg(long)]
        from: Option<String>,
        #[arg(long)]
        body: String,
        #[arg(long = "attach")]
        attach: Vec<PathBuf>,
        /// Forward without the original's text quoted under the body; its
        /// attachments still go.
        #[arg(long)]
        no_quote: bool,
        /// An attachment of the original, by ID, that does not go with the
        /// forward (repeatable); `attachments list` shows their IDs.
        #[arg(long = "leave-out", value_name = "ATTACHMENT_ID")]
        leave_out: Vec<String>,
    },
    /// Call off a message scheduled to be sent later.
    CancelSend { reference: String },
    /// Tell Proton messages are spam, which moves them to Spam.
    Spam {
        #[arg(required = true)]
        references: Vec<String>,
    },
    /// Tell Proton messages are not spam, which moves them to the Inbox.
    Ham {
        #[arg(required = true)]
        references: Vec<String>,
    },
    /// Leave the mailing list a message came from, by the list's one-click
    /// unsubscribe.
    Unsubscribe { reference: String },
    /// Permanently delete every message in Trash or Spam. Asks first; outside
    /// a terminal `--yes` is required.
    Empty {
        #[arg(value_enum)]
        folder: EmptyFolder,
        /// Empty without asking.
        #[arg(long)]
        yes: bool,
    },
    /// Put a label on messages, or take it off.
    Label {
        #[arg(value_enum)]
        action: LabelAction,
        /// The label, by its name, path or ID.
        label_id: String,
        /// Message references.
        #[arg(required = true)]
        references: Vec<String>,
    },
    /// Send the read receipt a message asks for.
    Receipt { reference: String },
    /// Move messages to the folder `--dest` names.
    Move {
        #[arg(required = true)]
        references: Vec<String>,
        /// A system folder, or one of your folders by its name, path or ID.
        #[arg(long)]
        dest: String,
    },
    /// Move messages to Trash.
    Trash {
        #[arg(required = true)]
        references: Vec<String>,
    },
    /// Permanently delete messages. Lists them and asks first; outside a
    /// terminal `--yes` is required.
    Delete {
        #[arg(required = true)]
        references: Vec<String>,
        /// Delete without asking.
        #[arg(long)]
        yes: bool,
    },
    /// Mark messages read or unread.
    Mark {
        state: ReadState,
        #[arg(required = true)]
        references: Vec<String>,
    },
    /// Star messages.
    Star {
        #[arg(required = true)]
        references: Vec<String>,
    },
    /// Unstar messages.
    Unstar {
        #[arg(required = true)]
        references: Vec<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum ConversationsCmd {
    /// List a folder's conversations, a page at a time.
    List {
        /// A system folder (inbox, sent, archive…), or one of your folders or
        /// labels by its name, path or ID.
        #[arg(long, default_value = "inbox")]
        folder: String,
        #[arg(long, default_value_t = 0)]
        page: u32,
        #[arg(long = "page-size", default_value_t = 25, value_parser = page_size())]
        page_size: u32,
        #[arg(long)]
        unread: bool,
    },
    /// Search conversations.
    Search(SearchArgs),
    /// Show a conversation with every message in it.
    Read {
        /// Conversation ID.
        id: String,
    },
    /// Move conversations to the folder `--dest` names.
    Move {
        #[arg(required = true)]
        ids: Vec<String>,
        /// A system folder, or one of your folders by its name, path or ID.
        #[arg(long)]
        dest: String,
    },
    /// Move conversations to Trash.
    Trash {
        #[arg(required = true)]
        ids: Vec<String>,
    },
    /// Mark conversations read or unread.
    Mark {
        state: ReadState,
        /// Where to mark them: a folder or label, or `all` for all mail.
        #[arg(long, default_value = "all")]
        folder: String,
        #[arg(required = true)]
        ids: Vec<String>,
    },
    /// Star conversations.
    Star {
        #[arg(required = true)]
        ids: Vec<String>,
    },
    /// Unstar conversations.
    Unstar {
        #[arg(required = true)]
        ids: Vec<String>,
    },
    /// Snooze conversations until a time: a Unix time with `--until`, or a
    /// duration from now with `--in`, such as `1h` or `2d`.
    Snooze {
        #[arg(required = true)]
        ids: Vec<String>,
        /// Unix time in seconds, still to come.
        #[arg(
            long,
            value_parser = parse_future_time,
            conflicts_with = "relative",
            required_unless_present = "relative"
        )]
        until: Option<i64>,
        /// A positive duration from now: seconds, or a number with s, m, h or
        /// d, e.g. `30m`, `2h`, `1d`.
        #[arg(long = "in", value_parser = parse_duration_secs)]
        relative: Option<i64>,
    },
    /// Bring snoozed conversations back before their time.
    Unsnooze {
        #[arg(required = true)]
        ids: Vec<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum AttachmentsCmd {
    /// List the attachments of a message, with their IDs.
    List {
        /// The message: its ID, or words that find it and no other.
        message: String,
        #[arg(long = "include-inline")]
        include_inline: bool,
    },
    /// Save one attachment of a message, or all of them with `--all`.
    Download {
        /// The message: its ID, or words that find it and no other.
        message: String,
        /// The attachment to save, by its ID; leave it out with `--all`.
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        attachment: Option<String>,
        #[arg(long = "output-dir")]
        output_dir: Option<PathBuf>,
        /// Save every attachment, inline images too with `--include-inline`.
        #[arg(long)]
        all: bool,
        #[arg(long = "include-inline")]
        include_inline: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum DraftsCmd {
    /// List drafts.
    List {
        #[arg(long, default_value_t = 0)]
        page: u32,
        #[arg(long = "page-size", default_value_t = 25, value_parser = page_size())]
        page_size: u32,
    },
    /// Save a new draft with its attachments (does not send). A delivery
    /// time, an expiration and a message password are `messages send`
    /// options; a draft keeps none of them.
    Save(DraftArgs),
    /// Replace an existing draft's content, adding any `--attach` files to
    /// the attachments it has.
    Edit {
        /// Draft message ID.
        id: String,
        #[command(flatten)]
        args: DraftArgs,
    },
    /// Permanently delete drafts. Lists them and asks first; outside a
    /// terminal `--yes` is required. Other messages are refused.
    Delete {
        /// Draft message IDs.
        #[arg(required = true)]
        ids: Vec<String>,
        /// Delete without asking.
        #[arg(long)]
        yes: bool,
    },
}

/// On/off toggle.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum OnOff {
    On,
    Off,
}

impl OnOff {
    pub fn as_bool(self) -> bool {
        matches!(self, OnOff::On)
    }
}

#[derive(Debug, Subcommand)]
pub enum SettingsCmd {
    /// Print the mail settings as Proton sends them, in JSON.
    Get,
    /// Turn the signing of outgoing mail on or off.
    Sign {
        #[arg(value_enum)]
        value: OnOff,
    },
    /// Turn on or off whether outgoing mail carries your public key.
    AttachPublicKey {
        #[arg(value_enum)]
        value: OnOff,
    },
}

#[derive(Debug, Subcommand)]
pub enum AddressesCmd {
    /// List addresses.
    List,
    /// Change the display name or signature of one of your addresses.
    #[command(group = clap::ArgGroup::new("change").required(true).multiple(true))]
    Update {
        id: String,
        #[arg(long = "display-name", group = "change")]
        display_name: Option<String>,
        #[arg(long, group = "change")]
        signature: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum ContactsCmd {
    /// List contacts.
    List {
        #[arg(long, default_value_t = 0)]
        page: u32,
        #[arg(long = "page-size", default_value_t = 50, value_parser = page_size())]
        page_size: u32,
    },
    /// List the email addresses your contacts hold, or only those matching
    /// `--email`.
    Emails {
        #[arg(long)]
        email: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum FiltersCmd {
    /// List filters.
    List,
    /// Create a filter named `--name` from a Sieve script; `--sieve -` reads
    /// the script from standard input.
    Create {
        #[arg(long)]
        name: String,
        /// The Sieve script itself, or `-` to read it from standard input.
        #[arg(long)]
        sieve: String,
    },
    /// Check that Proton accepts a Sieve script, creating nothing.
    Check {
        /// The Sieve script itself, or `-` to read it from standard input.
        #[arg(long)]
        sieve: String,
    },
    /// Delete a filter.
    Delete { id: String },
    /// Enable a filter.
    Enable { id: String },
    /// Disable a filter.
    Disable { id: String },
}

#[derive(Debug, Subcommand)]
pub enum LabelsCmd {
    /// List your labels, or your folders with `--folders`.
    List {
        /// Show your folders instead.
        #[arg(long)]
        folders: bool,
    },
    /// Make a new label, or a folder with `--folder`.
    Create {
        #[arg(long)]
        name: String,
        #[arg(long, default_value = "#8080FF")]
        color: String,
        /// Make it a folder, not a label.
        #[arg(long)]
        folder: bool,
        #[arg(long)]
        parent: Option<String>,
    },
    /// Delete labels or folders, given by their IDs.
    Delete {
        #[arg(required = true)]
        ids: Vec<String>,
    },
    /// Change the name, color or parent of a label or folder.
    #[command(group = clap::ArgGroup::new("change").required(true).multiple(true))]
    Update {
        id: String,
        #[arg(long, group = "change")]
        name: Option<String>,
        #[arg(long, group = "change")]
        color: Option<String>,
        #[arg(long, group = "change")]
        parent: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, Parser};

    impl Cli {
        /// The command parsed, for tests that always give one.
        fn cmd(self) -> Command {
            self.command.expect("a command")
        }
    }

    /// `args` parsed; tests give only command lines that are valid.
    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(args).unwrap_or_else(|error| panic!("{args:?}: {error}"))
    }

    #[test]
    fn every_command_is_defined_as_clap_expects() {
        // Clap checks a command's definition when parsing reaches it; this
        // checks every one at once.
        Cli::command().debug_assert();
    }

    #[test]
    fn help_never_shows_secrets_read_from_the_environment() {
        let mut command = Cli::command();
        let login = command.find_subcommand_mut("login").unwrap();
        for id in ["totp", "mailbox_password", "captcha_token"] {
            let arg = login
                .get_arguments()
                .find(|arg| arg.get_id() == id)
                .unwrap();
            assert!(arg.get_env().is_some(), "{id} is read from the environment");
            assert!(
                arg.is_hide_env_values_set(),
                "help would print the value of {id}"
            );
        }
    }

    #[test]
    fn the_interface_marks_read_unless_asked_to_keep_unread() {
        let Command::Tui(args) = parse(&["ruston-cli", "tui"]).cmd() else {
            panic!("not the interface");
        };
        assert!(!args.keep_unread);
        let Command::Tui(args) = parse(&["ruston-cli", "tui", "--keep-unread"]).cmd() else {
            panic!("not the interface");
        };
        assert!(args.keep_unread);
    }

    #[test]
    fn sign_in_options_belong_to_login_alone() {
        // Elsewhere they would be read and ignored.
        for args in [
            &["ruston-cli", "messages", "list", "--mailbox-password", "x"][..],
            &[
                "ruston-cli",
                "--api-url",
                "https://mail.example/api",
                "whoami",
            ],
            &["ruston-cli", "logout", "--captcha-chrome"],
        ] {
            assert!(Cli::try_parse_from(args).is_err(), "{args:?} was taken");
        }
        let Command::Login(login) = parse(&[
            "ruston-cli",
            "login",
            "--api-url",
            "https://mail.example/api",
            "--client",
            "web",
            "--captcha-chrome",
        ])
        .cmd() else {
            panic!("not a login");
        };
        assert_eq!(login.api_url.as_deref(), Some("https://mail.example/api"));
        assert!(matches!(login.client, Some(ClientPreset::Web)));
        assert!(login.captcha_chrome);
    }

    #[test]
    fn shared_session_profile_is_default_but_can_be_overridden() {
        assert_eq!(
            parse(&["ruston-cli", "whoami"]).profile,
            ruston_core::DEFAULT_SESSION_PROFILE
        );
        assert_eq!(
            parse(&["ruston-cli", "--profile", "default", "whoami"]).profile,
            "default"
        );
    }

    #[test]
    fn profile_must_be_one_path_component() {
        for profile in [
            "",
            ".",
            "..",
            "../settings",
            r"..\settings",
            "folder/work",
            r"folder\work",
            "C:settings",
            "/tmp/work",
        ] {
            assert!(
                Cli::try_parse_from(["ruston-cli", "--profile", profile, "whoami"]).is_err(),
                "accepted invalid profile {profile:?}"
            );
        }
        assert!(Cli::try_parse_from(["ruston-cli", "--profile", "work.mail", "whoami"]).is_ok());
    }

    #[test]
    fn watch_requires_positive_interval() {
        assert!(Cli::try_parse_from(["ruston-cli", "watch", "--interval", "0"]).is_err());
        assert!(Cli::try_parse_from(["ruston-cli", "watch", "--interval", "1"]).is_ok());
    }

    #[test]
    fn search_dates_are_refused_while_parsing() {
        // Found before any session is resumed.
        for command in ["messages", "conversations"] {
            for flag in ["--after", "--before"] {
                for date in ["15/01/2024", "2024-02-30", "yesterday"] {
                    let error = Cli::try_parse_from(["ruston-cli", command, "search", flag, date])
                        .unwrap_err();
                    assert_eq!(
                        error.kind(),
                        clap::error::ErrorKind::ValueValidation,
                        "{command} {flag} {date}"
                    );
                }
            }
            assert!(
                Cli::try_parse_from([
                    "ruston-cli",
                    command,
                    "search",
                    "--after",
                    "2024-01-01",
                    "--before",
                    "2024-01-31",
                ])
                .is_ok()
            );
        }
    }

    #[test]
    fn a_message_takes_each_recipient_and_file_given() {
        let Command::Messages {
            cmd: MessagesCmd::Send(send),
        } = parse(&[
            "ruston-cli",
            "messages",
            "send",
            "--to",
            "ana@example.test",
            "--cc",
            "cy@example.test",
            "--to",
            "bo@example.test",
            "--bcc",
            "di@example.test",
            "--subject",
            "Plans",
            "--body",
            "See the files.",
            "--attach",
            "plan.pdf",
            "--attach",
            "map.png",
        ])
        .cmd()
        else {
            panic!("not a send");
        };
        // A repeated option keeps its values in order, other options between
        // them or not.
        let message = send.message;
        assert_eq!(message.to, ["ana@example.test", "bo@example.test"]);
        assert_eq!(message.cc, ["cy@example.test"]);
        assert_eq!(message.bcc, ["di@example.test"]);
        assert_eq!(
            message.attach,
            [PathBuf::from("plan.pdf"), PathBuf::from("map.png")]
        );
    }

    #[test]
    fn the_body_option_may_only_say_where_the_text_is() {
        let send = |extra: &[&str]| {
            let start = [
                "ruston-cli",
                "messages",
                "send",
                "--to",
                "ana@example.test",
                "--subject",
                "Hi",
            ];
            let Command::Messages {
                cmd: MessagesCmd::Send(args),
            } = parse(&[&start[..], extra].concat()).cmd()
            else {
                panic!("not a send");
            };
            args.message
        };
        // `-` stays as typed: the handler reads standard input for it, and
        // the message then carries the text read.
        let piped = send(&["--body", "-"]);
        assert_eq!(piped.body, "-");
        assert!(!piped.html, "plain text unless --html");
        let message = piped.into_send_options("Read from standard input".into());
        assert_eq!(message.body, "Read from standard input");
        assert_eq!(message.to, ["ana@example.test"]);
        assert_eq!(message.subject, "Hi");
        assert!(send(&["--body", "<p>Hi</p>", "--html"]).html);
    }

    #[test]
    fn a_message_password_can_be_asked_for_instead_of_given() {
        let eo_password = |extra: &[&str]| {
            let mut args = vec![
                "ruston-cli",
                "messages",
                "send",
                "--to",
                "a@b.com",
                "--subject",
                "hi",
                "--body",
                "hello",
            ];
            args.extend(extra);
            let Command::Messages {
                cmd: MessagesCmd::Send(a),
            } = parse(&args).cmd()
            else {
                panic!("wrong command");
            };
            a.eo_password
        };
        assert_eq!(eo_password(&[]), None);
        assert_eq!(
            eo_password(&["--eo-password", "s3cret"]),
            Some("s3cret".into())
        );
        // Without a value it is asked for, even with an option after it.
        assert_eq!(eo_password(&["--eo-password"]), Some(String::new()));
        assert_eq!(
            eo_password(&["--eo-password", "--eo-hint", "our pet"]),
            Some(String::new())
        );
    }

    #[test]
    fn drafts_take_attachments_but_no_sending_options() {
        let draft = [
            "ruston-cli",
            "drafts",
            "save",
            "--to",
            "a@b.com",
            "--subject",
            "hi",
            "--body",
            "hello",
            "--attach",
            "/tmp/x.pdf",
        ];
        let Command::Drafts {
            cmd: DraftsCmd::Save(a),
        } = parse(&draft).cmd()
        else {
            panic!("wrong command");
        };
        assert_eq!(a.attach.len(), 1);
        // Only sending can schedule, expire or password-protect a message.
        for option in [
            ["--send-at", "1900000000"],
            ["--expires", "3600"],
            ["--eo-password", "secret"],
            ["--eo-hint", "hint"],
        ] {
            for command in ["save", "edit"] {
                let mut args = draft.to_vec();
                args[2] = command;
                if command == "edit" {
                    args.insert(3, "draft1");
                }
                args.extend(option);
                assert!(
                    Cli::try_parse_from(&args).is_err(),
                    "drafts {command} took {}",
                    option[0]
                );
            }
            let mut send = draft.to_vec();
            send[1] = "messages";
            send[2] = "send";
            send.extend(option);
            assert!(
                Cli::try_parse_from(&send).is_ok(),
                "send refused {}",
                option[0]
            );
        }
    }

    #[test]
    fn a_message_shows_as_text_unless_another_format_is_named() {
        for (named, expected) in [
            (&[][..], "text"),
            (&["--format", "html"][..], "html"),
            (&["--format", "raw"][..], "raw"),
        ] {
            let args = [&["ruston-cli", "messages", "read", "msg1"][..], named].concat();
            let Command::Messages {
                cmd: MessagesCmd::Read { format, .. },
            } = parse(&args).cmd()
            else {
                panic!("not a read");
            };
            let name = format.to_possible_value().expect("every format has a name");
            assert_eq!(name.get_name(), expected, "{args:?}");
        }
        let unknown = ["ruston-cli", "messages", "read", "msg1", "--format", "pdf"];
        assert!(Cli::try_parse_from(unknown).is_err());
    }

    #[test]
    fn reading_can_show_the_body_alone_or_save_it_to_a_file() {
        let read = |extra: &[&str]| {
            let start = ["ruston-cli", "messages", "read", "invoice march"];
            let Command::Messages {
                cmd:
                    MessagesCmd::Read {
                        reference,
                        body_only,
                        output,
                        ..
                    },
            } = parse(&[&start[..], extra].concat()).cmd()
            else {
                panic!("not a read");
            };
            // Words that find the message are one reference.
            assert_eq!(reference, "invoice march");
            (body_only, output)
        };
        assert_eq!(read(&[]), (false, None));
        assert_eq!(
            read(&["--body-only", "--output", "body.txt"]),
            (true, Some(PathBuf::from("body.txt")))
        );
    }

    #[test]
    fn a_move_names_the_folder_it_goes_to() {
        for group in ["messages", "conversations"] {
            let error = Cli::try_parse_from(["ruston-cli", group, "move", "id1"]).unwrap_err();
            assert_eq!(
                error.kind(),
                clap::error::ErrorKind::MissingRequiredArgument,
                "{group}"
            );
        }
        let Command::Messages {
            cmd: MessagesCmd::Move { references, dest },
        } = parse(&[
            "ruston-cli",
            "messages",
            "move",
            "id1",
            "id2",
            "--dest",
            "Taxes/2024",
        ])
        .cmd()
        else {
            panic!("not a move");
        };
        assert_eq!(references, ["id1", "id2"]);
        assert_eq!(dest, "Taxes/2024");
    }

    #[test]
    fn permanent_deletes_take_only_trash_or_spam_and_an_optional_yes() {
        for folder in ["inbox", "all", "archive", "qBIcv1_Wv5X4hLpEo0Tz9A=="] {
            assert!(
                Cli::try_parse_from(["ruston-cli", "messages", "empty", folder]).is_err(),
                "emptied {folder}"
            );
        }
        let Command::Messages {
            cmd: MessagesCmd::Empty { folder, yes },
        } = parse(&["ruston-cli", "messages", "empty", "spam", "--yes"]).cmd()
        else {
            panic!("wrong command");
        };
        assert!(matches!(folder, EmptyFolder::Spam));
        assert!(yes);
        let Command::Messages {
            cmd: MessagesCmd::Delete { references, yes },
        } = parse(&["ruston-cli", "messages", "delete", "id1"]).cmd()
        else {
            panic!("wrong command");
        };
        assert_eq!(references, vec!["id1"]);
        assert!(!yes, "deleting asks unless told not to");
        // Deleting drafts names them and asks the same way.
        assert!(Cli::try_parse_from(["ruston-cli", "drafts", "delete"]).is_err());
        let Command::Drafts {
            cmd: DraftsCmd::Delete { ids, yes },
        } = parse(&["ruston-cli", "drafts", "delete", "draft1"]).cmd()
        else {
            panic!("wrong command");
        };
        assert_eq!(ids, vec!["draft1"]);
        assert!(!yes, "deleting asks unless told not to");
    }

    #[test]
    fn marking_names_the_state_before_the_messages() {
        for (typed, read) in [("read", true), ("unread", false)] {
            let Command::Messages {
                cmd: MessagesCmd::Mark { state, references },
            } = parse(&["ruston-cli", "messages", "mark", typed, "id1", "id2"]).cmd()
            else {
                panic!("not a mark");
            };
            assert_eq!(state.as_bool(), read, "{typed}");
            assert_eq!(references, ["id1", "id2"]);
        }
        // An ID where the state goes is refused, not taken for a message.
        assert!(Cli::try_parse_from(["ruston-cli", "messages", "mark", "id1", "read"]).is_err());
    }

    #[test]
    fn global_options_go_before_or_after_the_command() {
        let global = ["--json", "--profile", "work"];
        let listing = [
            "messages",
            "list",
            "--folder",
            "archive",
            "--page-size",
            "10",
            "--unread",
        ];
        for args in [
            [&["ruston-cli"][..], &global, &listing].concat(),
            [&["ruston-cli"][..], &listing, &global].concat(),
        ] {
            let cli = parse(&args);
            assert!(cli.json, "{args:?}");
            assert_eq!(cli.profile, "work", "{args:?}");
            let Command::Messages {
                cmd:
                    MessagesCmd::List {
                        folder,
                        page_size,
                        unread,
                        ..
                    },
            } = cli.cmd()
            else {
                panic!("not a listing");
            };
            assert_eq!(folder, "archive");
            assert_eq!(page_size, 10);
            assert!(unread);
        }
    }

    #[test]
    fn conversations_are_marked_in_all_mail_unless_a_folder_is_named() {
        for (named, expected) in [(&[][..], "all"), (&["--folder", "inbox"][..], "inbox")] {
            let mark = ["ruston-cli", "conversations", "mark", "unread"];
            let args = [&mark[..], named, &["conv1"]].concat();
            let Command::Conversations {
                cmd: ConversationsCmd::Mark { state, folder, ids },
            } = parse(&args).cmd()
            else {
                panic!("not a mark");
            };
            assert!(!state.as_bool());
            assert_eq!(folder, expected, "{args:?}");
            assert_eq!(ids, ["conv1"]);
        }
    }

    #[test]
    fn an_attachment_download_names_what_to_fetch_before_anything_runs() {
        // Found while parsing: no session is resumed and no folder is made.
        let error =
            Cli::try_parse_from(["ruston-cli", "attachments", "download", "msg"]).unwrap_err();
        assert_eq!(
            error.kind(),
            clap::error::ErrorKind::MissingRequiredArgument
        );
        for args in [
            &["ruston-cli", "attachments", "download", "msg", "att"][..],
            &["ruston-cli", "attachments", "download", "msg", "--all"],
        ] {
            assert!(Cli::try_parse_from(args).is_ok(), "{args:?} was refused");
        }
        // An ID beside --all would be ignored, so the two are refused.
        let both = Cli::try_parse_from([
            "ruston-cli",
            "attachments",
            "download",
            "msg",
            "att",
            "--all",
        ])
        .unwrap_err();
        assert_eq!(both.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn commands_that_act_on_messages_need_at_least_one() {
        for args in [
            &["ruston-cli", "messages", "spam"][..],
            &["ruston-cli", "messages", "ham"],
            &["ruston-cli", "messages", "label", "add", "label-1"],
        ] {
            let error = Cli::try_parse_from(args).unwrap_err();
            assert_eq!(
                error.kind(),
                clap::error::ErrorKind::MissingRequiredArgument,
                "{args:?}"
            );
        }
        for args in [
            &["ruston-cli", "messages", "spam", "m1"][..],
            &["ruston-cli", "messages", "ham", "m1"],
            &["ruston-cli", "messages", "label", "add", "label-1", "m1"],
        ] {
            assert!(Cli::try_parse_from(args).is_ok(), "{args:?} was refused");
        }
    }

    #[test]
    fn an_update_must_change_something() {
        for args in [
            &["ruston-cli", "labels", "update", "label-1"][..],
            &["ruston-cli", "addresses", "update", "address-1"],
        ] {
            let error = Cli::try_parse_from(args).unwrap_err();
            assert_eq!(
                error.kind(),
                clap::error::ErrorKind::MissingRequiredArgument,
                "{args:?}"
            );
        }
        for args in [
            &[
                "ruston-cli",
                "labels",
                "update",
                "label-1",
                "--color",
                "#FF0000",
            ][..],
            &[
                "ruston-cli",
                "labels",
                "update",
                "label-1",
                "--name",
                "Taxes",
                "--parent",
                "f",
            ],
            &[
                "ruston-cli",
                "addresses",
                "update",
                "address-1",
                "--signature",
                "Ana",
            ],
        ] {
            assert!(Cli::try_parse_from(args).is_ok(), "{args:?} was refused");
        }
    }

    #[test]
    fn a_snooze_takes_one_end_still_to_come() {
        let snooze = |extra: &[&str]| {
            let mut args = vec!["ruston-cli", "conversations", "snooze", "c1"];
            args.extend_from_slice(extra);
            Cli::try_parse_from(args)
        };
        // 2100-01-01, and a moment long gone.
        assert!(snooze(&["--until", "4102444800"]).is_ok());
        assert!(snooze(&["--in", "2h"]).is_ok());
        for refused in [
            &[][..],
            &["--until", "4102444800", "--in", "2h"],
            &["--until", "1"],
            &["--until", "soon"],
            &["--in", "0"],
        ] {
            assert!(snooze(refused).is_err(), "{refused:?} was accepted");
        }
    }

    #[test]
    fn snooze_durations_are_positive_and_never_wrap() {
        for (duration, seconds) in [("30m", 1800), ("1d", 86_400), ("+5m", 300), ("45", 45)] {
            assert_eq!(
                parse_duration_secs(duration).unwrap(),
                seconds,
                "{duration}"
            );
        }
        for duration in [
            "-5m",
            "0",
            "0d",
            "106751991167301d",
            "9223372036854775807h",
            "5w",
        ] {
            assert!(parse_duration_secs(duration).is_err(), "{duration}");
        }
    }

    #[test]
    fn page_sizes_stay_within_what_paging_can_follow() {
        for command in [
            &["ruston-cli", "messages", "list"][..],
            &["ruston-cli", "conversations", "list"],
            &["ruston-cli", "drafts", "list"],
            &["ruston-cli", "contacts", "list"],
            &["ruston-cli", "index"],
            &["ruston-cli", "sync"],
        ] {
            let with = |size: &str| {
                let mut args = command.to_vec();
                args.extend_from_slice(&["--page-size", size]);
                Cli::try_parse_from(args)
            };
            assert!(with("1").is_ok(), "{command:?}");
            assert!(with("100").is_ok(), "{command:?}");
            for refused in ["0", "101", "1000"] {
                assert!(with(refused).is_err(), "{command:?} took {refused}");
            }
            assert!(Cli::try_parse_from(command).is_ok(), "{command:?} default");
        }
    }

    #[test]
    fn credentials_go_over_https_or_to_this_machine() {
        for accepted in [
            "https://mail.proton.me/api",
            "http://127.0.0.1:8080",
            "http://localhost:8080/api",
            "http://[::1]:8080",
        ] {
            assert!(parse_api_url(accepted).is_ok(), "{accepted} was refused");
        }
        for refused in [
            "http://mail.proton.me/api",
            "http://10.0.0.2",
            "ftp://mail.proton.me",
            "mail.proton.me",
        ] {
            assert!(parse_api_url(refused).is_err(), "{refused} was accepted");
        }
    }

    #[test]
    fn help_explains_every_exit_status() {
        let help = Cli::command().render_help().to_string();
        for status in ["0  ", "1  ", "2  ", "3  ", "4  ", "5  ", "6  "] {
            assert!(help.contains(status), "status {status}is not explained");
        }
    }

    #[test]
    fn index_leaves_trash_and_spam_out_unless_asked() {
        use ruston_core::model::enums::{label_ids, resolve_folder};

        let folder = |args: &[&str]| {
            let Command::Index { folder, .. } = parse(args).cmd() else {
                panic!("wrong command");
            };
            resolve_folder(&folder)
        };
        // Mail moved to Trash or Spam is not decrypted onto disk by default.
        assert_eq!(folder(&["ruston-cli", "index"]), label_ids::ALMOST_ALL_MAIL);
        assert_eq!(
            folder(&["ruston-cli", "index", "--folder", "all"]),
            label_ids::ALL_MAIL
        );
    }

    #[test]
    fn a_new_label_has_a_default_color_and_no_parent() {
        let Command::Labels {
            cmd:
                LabelsCmd::Create {
                    name,
                    color,
                    folder,
                    parent,
                },
        } = parse(&["ruston-cli", "labels", "create", "--name", "Receipts"]).cmd()
        else {
            panic!("not a create");
        };
        assert_eq!(name, "Receipts");
        assert_eq!(color, "#8080FF");
        assert!(!folder, "a label unless --folder");
        assert_eq!(parent, None);

        let Command::Labels {
            cmd:
                LabelsCmd::Create {
                    color,
                    folder,
                    parent,
                    ..
                },
        } = parse(&[
            "ruston-cli",
            "labels",
            "create",
            "--name",
            "2024",
            "--folder",
            "--parent",
            "folder-taxes",
            "--color",
            "#3CBB3A",
        ])
        .cmd()
        else {
            panic!("not a create");
        };
        assert_eq!(color, "#3CBB3A");
        assert!(folder);
        assert_eq!(parent.as_deref(), Some("folder-taxes"));
    }

    #[test]
    fn demo_flag_parsed() {
        let cli = parse(&["ruston-cli", "--demo", "whoami"]);
        assert!(cli.demo);
        let ctx: Ctx = (&cli).into();
        assert!(ctx.demo);
    }
}
