//! Command-line interface definition (clap v4 derive).

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

    /// Emit machine-readable JSON instead of human-readable text.
    #[arg(long, global = true)]
    pub json: bool,

    /// Verbose logging to stderr (-v debug, -vv trace core, -vvv trace all).
    /// `RUST_LOG` overrides. Logs steps of login, crypto, and every HTTP call.
    #[arg(short = 'v', long = "verbose", global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Override the API base URL (login only): https, or plain http to this
    /// machine for a local test server.
    #[arg(long, global = true, value_parser = parse_api_url)]
    pub api_url: Option<String>,

    /// Override the app-version header (login only).
    #[arg(long, global = true)]
    pub app_version: Option<String>,

    // Secrets read from the environment: help names the variable, never its value.
    /// TOTP two-factor code (prompted for when required if omitted).
    #[arg(long, global = true, env = "PROTON_TOTP", hide_env_values = true)]
    pub totp: Option<String>,

    /// Separate mailbox password (two-password accounts). On a terminal it is
    /// asked for, hidden, when the account has one and it is not given.
    #[arg(
        long,
        global = true,
        env = "PROTON_MAILBOX_PASSWORD",
        hide_env_values = true
    )]
    pub mailbox_password: Option<String>,

    /// Pre-solved human-verification (CAPTCHA) token, skips the interactive flow.
    #[arg(long, global = true, env = "PROTON_HV_TOKEN", hide_env_values = true)]
    pub captcha_token: Option<String>,

    /// Present as an official client (sets app-version + User-Agent unless overridden).
    #[arg(long, global = true, value_enum)]
    pub client: Option<ClientPreset>,

    /// Override the User-Agent header (login only; default `ruston-cli/<version> (<os>)`).
    #[arg(long, global = true)]
    pub user_agent: Option<String>,

    /// Solve the CAPTCHA on verify.proton.me in an isolated Chrome window, or
    /// Chromium, Brave or Edge (throwaway profile); press Enter once verified
    /// and the window is closed.
    #[arg(long, global = true)]
    pub captcha_chrome: bool,

    /// Run in offline demo mode using fictional data without contacting Proton.
    #[arg(long, global = true)]
    pub demo: bool,

    #[command(subcommand)]
    pub command: Command,
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

/// A Unix time in seconds that has not passed yet.
fn parse_future_time(value: &str) -> std::result::Result<i64, String> {
    let time: i64 = value
        .parse()
        .map_err(|_| "expected a Unix time in seconds".to_owned())?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_secs()).unwrap_or(i64::MAX)
        });
    if time <= now {
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

/// Official-client presets for `--client`.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ClientPreset {
    Web,
    Ios,
    Android,
}

impl ClientPreset {
    /// A representative `x-pm-appversion` value (override with `--app-version`).
    pub fn app_version(self) -> &'static str {
        match self {
            ClientPreset::Web => "web-mail@5.0.99.0",
            ClientPreset::Ios => "ios-mail@5.4.0",
            ClientPreset::Android => "android-mail@4.5.0",
        }
    }
    /// A representative `User-Agent` (override with `--user-agent`).
    pub fn user_agent(self) -> &'static str {
        match self {
            ClientPreset::Web => {
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36"
            }
            ClientPreset::Ios => "ProtonMail/5.4.0 (iOS/17.4; iPhone)",
            ClientPreset::Android => "ProtonMail/4.5.0 (Android/14; OnePlus)",
        }
    }
}

/// Global options threaded through to command handlers.
pub struct Ctx {
    pub profile: String,
    pub json: bool,
    pub demo: bool,
    pub api_url: Option<String>,
    pub app_version: Option<String>,
    pub totp: Option<String>,
    pub mailbox_password: Option<String>,
    pub captcha_token: Option<String>,
    pub client: Option<ClientPreset>,
    pub user_agent: Option<String>,
    pub captcha_chrome: bool,
}

impl From<&Cli> for Ctx {
    fn from(c: &Cli) -> Self {
        let env_demo = std::env::var(ruston_core::DEMO_ENV)
            .is_ok_and(|value| ruston_core::demo_requested(&value));

        Ctx {
            profile: c.profile.clone(),
            json: c.json,
            demo: c.demo || env_demo,
            api_url: c.api_url.clone(),
            app_version: c.app_version.clone(),
            totp: c.totp.clone(),
            mailbox_password: c.mailbox_password.clone(),
            captcha_token: c.captcha_token.clone(),
            client: c.client,
            user_agent: c.user_agent.clone(),
            captcha_chrome: c.captcha_chrome,
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Log in to Proton Mail (prompts for credentials unless given via env).
    Login,
    /// Revoke the server session and clear local state. A session that can no
    /// longer be resumed is still cleared from this device.
    Logout,
    /// Print the primary email address of the active session.
    Whoami,
    /// Message operations.
    Messages {
        #[command(subcommand)]
        cmd: MessagesCmd,
    },
    /// Conversation (thread) operations.
    Conversations {
        #[command(subcommand)]
        cmd: ConversationsCmd,
    },
    /// Attachment operations.
    Attachments {
        #[command(subcommand)]
        cmd: AttachmentsCmd,
    },
    /// Draft operations (create/edit/delete without sending).
    Drafts {
        #[command(subcommand)]
        cmd: DraftsCmd,
    },
    /// Server-side Sieve filter operations.
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
    /// Mail settings (read + a few toggles).
    Settings {
        #[command(subcommand)]
        cmd: SettingsCmd,
    },
    /// Per-folder message (or conversation) counts.
    Counts {
        /// Count conversations instead of messages.
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
    /// Continuously sync the cache (event stream) until interrupted. Network
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
    /// Full-text search the local index (message bodies; private + offline).
    Search {
        /// Query words (AND-ed together).
        query: String,
        #[arg(long, default_value_t = 25)]
        limit: u32,
    },
    /// Sync the local cache from the event stream (optionally backfill a folder).
    Sync {
        /// Also backfill this folder into the cache (e.g. `inbox`).
        #[arg(long)]
        backfill: Option<String>,
        #[arg(long = "max-pages", default_value_t = 4)]
        max_pages: u32,
        #[arg(long = "page-size", default_value_t = 50, value_parser = page_size())]
        page_size: u32,
    },
    /// Label and folder operations.
    Labels {
        #[command(subcommand)]
        cmd: LabelsCmd,
    },
}

/// Shared search options (messages + conversations).
#[derive(Debug, Args)]
pub struct SearchArgs {
    /// Free-text keyword.
    #[arg(long)]
    pub keyword: Option<String>,
    /// Match the sender address.
    #[arg(long)]
    pub from: Option<String>,
    /// Match a recipient address.
    #[arg(long)]
    pub to: Option<String>,
    /// Match the subject.
    #[arg(long)]
    pub subject: Option<String>,
    /// Only results on/after this date (YYYY-MM-DD, a day in UTC).
    #[arg(long, value_parser = parse_search_date)]
    pub after: Option<String>,
    /// Only results on/before this date (YYYY-MM-DD, a day in UTC), including it.
    #[arg(long, value_parser = parse_search_date)]
    pub before: Option<String>,
    /// Restrict to a folder or label: a system folder, or one of yours by
    /// its name, path or ID.
    #[arg(long)]
    pub folder: Option<String>,
    /// Only unread results.
    #[arg(long)]
    pub unread: bool,
    /// Maximum number of results.
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
    /// Primary recipient(s) (repeatable).
    #[arg(long = "to", required = true)]
    pub to: Vec<String>,
    /// Carbon-copy recipient(s) (repeatable).
    #[arg(long = "cc")]
    pub cc: Vec<String>,
    /// Blind carbon-copy recipient(s) (repeatable).
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
    /// Message body, or `-` to read from stdin.
    #[arg(long)]
    pub body: String,
    /// Treat the body as HTML. `messages send` may ask on a terminal when the
    /// body looks like HTML.
    #[arg(long)]
    pub html: bool,
    /// Attach a file (repeatable).
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
    /// Schedule delivery at this Unix timestamp.
    #[arg(long = "send-at")]
    pub send_at: Option<i64>,
    /// Self-destruct after this many seconds.
    #[arg(long = "expires")]
    pub expires: Option<i64>,
    /// Encrypted-outside: password-protect the message for keyless external recipients.
    /// Without a value, the password is asked for twice, hidden, on the terminal.
    #[arg(long = "eo-password", num_args = 0..=1, default_missing_value = "")]
    pub eo_password: Option<String>,
    /// Password hint shown to encrypted-outside recipients.
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

/// Apply or remove a label.
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
    /// List messages in a folder.
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
        /// Read from the local cache instead of the API (offline).
        #[arg(long)]
        cached: bool,
    },
    /// Search messages.
    Search(SearchArgs),
    /// Read a single message.
    Read {
        /// Message reference (ID or free-text).
        reference: String,
        #[arg(long, value_enum, default_value_t = ReadFormat::Text)]
        format: ReadFormat,
        /// Print only the body.
        #[arg(long = "body-only")]
        body_only: bool,
        /// Write the body to a file instead of stdout.
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Compose and send a message.
    Send(SendArgs),
    /// Reply to a message.
    Reply {
        reference: String,
        /// Reply to all recipients.
        #[arg(long)]
        all: bool,
        /// Sender address (defaults to the address the original message reached).
        #[arg(long)]
        from: Option<String>,
        /// Body text, or `-` for stdin.
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
    /// Cancel a scheduled send.
    CancelSend { reference: String },
    /// Report messages as spam (move to Spam).
    Spam {
        #[arg(required = true)]
        references: Vec<String>,
    },
    /// Report messages as not-spam (move to Inbox).
    Ham {
        #[arg(required = true)]
        references: Vec<String>,
    },
    /// One-click unsubscribe from a mailing list.
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
    /// Apply or remove a label on messages.
    Label {
        #[arg(value_enum)]
        action: LabelAction,
        /// The label, by its name, path or ID.
        label_id: String,
        /// Message references.
        #[arg(required = true)]
        references: Vec<String>,
    },
    /// Restore permanently-deleted messages (by ID).
    Undelete {
        #[arg(required = true)]
        ids: Vec<String>,
    },
    /// Send a read receipt for a message.
    Receipt { reference: String },
    /// Move messages to a folder.
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
    /// List conversations in a folder.
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
    /// Read a conversation (all messages).
    Read {
        /// Conversation ID.
        id: String,
    },
    /// Move conversations to a folder.
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
        /// Label context for the mark operation.
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
    /// Snooze conversations until a Unix timestamp (--until) or a duration (--in 1h/2d).
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
    /// Unsnooze conversations.
    Unsnooze {
        #[arg(required = true)]
        ids: Vec<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum AttachmentsCmd {
    /// List a message's attachments.
    List {
        /// Message reference (ID or free-text).
        message: String,
        #[arg(long = "include-inline")]
        include_inline: bool,
    },
    /// Download attachment(s) from a message.
    Download {
        /// Message reference (ID or free-text).
        message: String,
        /// Specific attachment ID (omit with --all).
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        attachment: Option<String>,
        #[arg(long = "output-dir")]
        output_dir: Option<PathBuf>,
        /// Download all attachments.
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
    /// Show mail settings (raw JSON).
    Get,
    /// Toggle signing outgoing mail.
    Sign {
        #[arg(value_enum)]
        value: OnOff,
    },
    /// Toggle attaching your public key to outgoing mail.
    AttachPublicKey {
        #[arg(value_enum)]
        value: OnOff,
    },
}

#[derive(Debug, Subcommand)]
pub enum AddressesCmd {
    /// List addresses.
    List,
    /// Update an address (display name / signature).
    Update {
        id: String,
        #[arg(long = "display-name")]
        display_name: Option<String>,
        #[arg(long)]
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
    /// List contact email addresses (optionally filter by an email).
    Emails {
        #[arg(long)]
        email: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum FiltersCmd {
    /// List filters.
    List,
    /// Create a filter from a Sieve script (`--sieve -` reads stdin).
    Create {
        #[arg(long)]
        name: String,
        /// Sieve script text, or `-` for stdin.
        #[arg(long)]
        sieve: String,
    },
    /// Validate a Sieve script without creating a filter.
    Check {
        /// Sieve script text, or `-` for stdin.
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
    /// List labels (or folders with --folders).
    List {
        /// List folders instead of labels.
        #[arg(long)]
        folders: bool,
    },
    /// Create a label or folder.
    Create {
        #[arg(long)]
        name: String,
        #[arg(long, default_value = "#8080FF")]
        color: String,
        /// Create a folder instead of a label.
        #[arg(long)]
        folder: bool,
        #[arg(long)]
        parent: Option<String>,
    },
    /// Delete labels/folders by ID.
    Delete {
        #[arg(required = true)]
        ids: Vec<String>,
    },
    /// Rename / recolor / reparent a label or folder.
    Update {
        id: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        color: Option<String>,
        #[arg(long)]
        parent: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, Parser};

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(args).expect("args should parse")
    }

    #[test]
    fn verify_clap_command() {
        // Guards against derive macro misconfiguration.
        Cli::command().debug_assert();
    }

    #[test]
    fn help_never_shows_secrets_read_from_the_environment() {
        let command = Cli::command();
        for id in ["totp", "mailbox_password", "captcha_token"] {
            let arg = command
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
    fn send_with_stdin_body() {
        let cli = parse(&[
            "ruston-cli",
            "messages",
            "send",
            "--to",
            "a@b.com",
            "--subject",
            "x",
            "--body",
            "-",
        ]);
        let Command::Messages {
            cmd: MessagesCmd::Send(a),
        } = cli.command
        else {
            panic!("wrong command");
        };
        assert_eq!(a.message.to, vec!["a@b.com"]);
        assert_eq!(a.message.subject, "x");
        assert_eq!(a.message.body, "-");
        assert!(!a.message.html);
    }

    #[test]
    fn send_multiple_recipients_and_attachments() {
        let cli = parse(&[
            "ruston-cli",
            "messages",
            "send",
            "--to",
            "a@b.com",
            "--to",
            "c@d.com",
            "--cc",
            "e@f.com",
            "--subject",
            "hi",
            "--body",
            "hello",
            "--html",
            "--attach",
            "/tmp/x.pdf",
            "--attach",
            "/tmp/y.png",
        ]);
        let Command::Messages {
            cmd: MessagesCmd::Send(a),
        } = cli.command
        else {
            panic!("wrong command");
        };
        assert_eq!(a.message.to.len(), 2);
        assert_eq!(a.message.cc, vec!["e@f.com"]);
        assert!(a.message.html);
        assert_eq!(a.message.attach.len(), 2);
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
            } = parse(&args).command
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
        } = parse(&draft).command
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
    fn read_with_html_format() {
        let cli = parse(&["ruston-cli", "messages", "read", "REF", "--format", "html"]);
        let Command::Messages {
            cmd:
                MessagesCmd::Read {
                    reference,
                    format,
                    body_only,
                    ..
                },
        } = cli.command
        else {
            panic!("wrong command");
        };
        assert_eq!(reference, "REF");
        assert!(matches!(format, ReadFormat::Html));
        assert!(!body_only);
    }

    #[test]
    fn read_defaults_to_text() {
        let cli = parse(&["ruston-cli", "messages", "read", "abc"]);
        let Command::Messages {
            cmd: MessagesCmd::Read { format, .. },
        } = cli.command
        else {
            panic!("wrong command");
        };
        assert!(matches!(format, ReadFormat::Text));
    }

    #[test]
    fn move_requires_dest() {
        let cli = parse(&["ruston-cli", "messages", "move", "REF", "--dest", "archive"]);
        let Command::Messages {
            cmd: MessagesCmd::Move { references, dest },
        } = cli.command
        else {
            panic!("wrong command");
        };
        assert_eq!(references, vec!["REF"]);
        assert_eq!(dest, "archive");
        // Missing --dest is a parse error.
        assert!(Cli::try_parse_from(["ruston-cli", "messages", "move", "REF"]).is_err());
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
        } = parse(&["ruston-cli", "messages", "empty", "spam", "--yes"]).command
        else {
            panic!("wrong command");
        };
        assert!(matches!(folder, EmptyFolder::Spam));
        assert!(yes);
        let Command::Messages {
            cmd: MessagesCmd::Delete { references, yes },
        } = parse(&["ruston-cli", "messages", "delete", "id1"]).command
        else {
            panic!("wrong command");
        };
        assert_eq!(references, vec!["id1"]);
        assert!(!yes, "deleting asks unless told not to");
        // Deleting drafts names them and asks the same way.
        assert!(Cli::try_parse_from(["ruston-cli", "drafts", "delete"]).is_err());
        let Command::Drafts {
            cmd: DraftsCmd::Delete { ids, yes },
        } = parse(&["ruston-cli", "drafts", "delete", "draft1"]).command
        else {
            panic!("wrong command");
        };
        assert_eq!(ids, vec!["draft1"]);
        assert!(!yes, "deleting asks unless told not to");
    }

    #[test]
    fn mark_state_then_refs() {
        let cli = parse(&["ruston-cli", "messages", "mark", "read", "id1", "id2"]);
        let Command::Messages {
            cmd: MessagesCmd::Mark { state, references },
        } = cli.command
        else {
            panic!("wrong command");
        };
        assert!(matches!(state, ReadState::Read));
        assert!(state.as_bool());
        assert_eq!(references, vec!["id1", "id2"]);
    }

    #[test]
    fn global_flags_parse() {
        let cli = parse(&[
            "ruston-cli",
            "--json",
            "--profile",
            "work",
            "messages",
            "list",
            "--folder",
            "archive",
            "--page-size",
            "10",
            "--unread",
        ]);
        assert!(cli.json);
        assert_eq!(cli.profile, "work");
        let Command::Messages {
            cmd:
                MessagesCmd::List {
                    folder,
                    page_size,
                    unread,
                    ..
                },
        } = cli.command
        else {
            panic!("wrong command");
        };
        assert_eq!(folder, "archive");
        assert_eq!(page_size, 10);
        assert!(unread);
    }

    #[test]
    fn conversations_mark_default_folder() {
        let cli = parse(&["ruston-cli", "conversations", "mark", "unread", "conv1"]);
        let Command::Conversations {
            cmd: ConversationsCmd::Mark { state, folder, ids },
        } = cli.command
        else {
            panic!("wrong command");
        };
        assert!(matches!(state, ReadState::Unread));
        assert_eq!(folder, "all");
        assert_eq!(ids, vec!["conv1"]);
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
            &["ruston-cli", "messages", "undelete"],
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
            &["ruston-cli", "messages", "undelete", "m1"],
            &["ruston-cli", "messages", "label", "add", "label-1", "m1"],
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
            let Command::Index { folder, .. } = parse(args).command else {
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
    fn labels_create_defaults() {
        let cli = parse(&["ruston-cli", "labels", "create", "--name", "Work"]);
        let Command::Labels {
            cmd:
                LabelsCmd::Create {
                    name,
                    color,
                    folder,
                    parent,
                },
        } = cli.command
        else {
            panic!("wrong command");
        };
        assert_eq!(name, "Work");
        assert_eq!(color, "#8080FF");
        assert!(!folder);
        assert!(parent.is_none());
    }

    #[test]
    fn demo_flag_parsed() {
        let cli = parse(&["ruston-cli", "--demo", "whoami"]);
        assert!(cli.demo);
        let ctx: Ctx = (&cli).into();
        assert!(ctx.demo);
    }
}
