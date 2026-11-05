//! The handlers that carry out commands on the account, a module for each
//! area, and what several of them share.

pub mod attachments;
pub mod auth;
pub mod contacts;
pub mod conversations;
pub mod drafts;
pub mod export;
pub mod filters;
pub mod labels;
pub mod messages;
pub mod prompt;
pub mod search;
pub mod sync;
pub mod watch;

use crate::cli::{Command, Ctx};
use futures::{StreamExt, TryStreamExt};
use ruston_core::{Client, Result, SearchOpts};

/// Runs `command` on the account: each command goes to the handler of its
/// area. The offline demo answers commands through its own dispatch.
pub async fn run(ctx: &Ctx, command: Command) -> Result<()> {
    match command {
        Command::Login(args) => auth::login(ctx, args).await,
        Command::Logout => auth::logout(ctx).await,
        Command::Whoami => auth::whoami(ctx).await,
        Command::Tui(args) => crate::tui::run(ctx, &args).await,
        Command::Messages { cmd } => messages::run(ctx, cmd).await,
        Command::Conversations { cmd } => conversations::run(ctx, cmd).await,
        Command::Attachments { cmd } => attachments::run(ctx, cmd).await,
        Command::Drafts { cmd } => drafts::run(ctx, cmd).await,
        Command::Filters { cmd } => filters::run(ctx, cmd).await,
        Command::Labels { cmd } => labels::run(ctx, cmd).await,
        Command::Contacts { cmd } => contacts::run(ctx, cmd).await,
        Command::Addresses { cmd } => contacts::addresses(ctx, cmd).await,
        Command::Settings { cmd } => contacts::settings(ctx, cmd).await,
        Command::Counts { conversations } => contacts::counts(ctx, conversations).await,
        Command::Export { folder, out, max } => export::run(ctx, folder, out, max).await,
        Command::Watch { interval, folder } => watch::run(ctx, interval, folder).await,
        Command::Sync {
            backfill,
            max_pages,
            page_size,
        } => sync::run(ctx, backfill, max_pages, page_size).await,
        Command::Index {
            folder,
            max_pages,
            page_size,
        } => search::index(ctx, folder, max_pages, page_size).await,
        Command::Search { query, limit } => search::search(ctx, query, limit).await,
    }
}

/// `opts` with the folder it searches named by its label ID, so a folder
/// of the account's own can be named as people see it.
pub async fn search_options(client: &Client, opts: SearchOpts) -> Result<SearchOpts> {
    let folder = match &opts.folder {
        Some(folder) => Some(client.folder_id(folder).await?),
        None => None,
    };
    Ok(SearchOpts { folder, ..opts })
}

/// The text a body option stands for: `-` means all of standard input, and
/// anything else is the text itself.
pub fn read_body(body: &str) -> std::io::Result<String> {
    match body {
        "-" => std::io::read_to_string(std::io::stdin()),
        text => Ok(text.to_owned()),
    }
}

/// The message ID each reference names, in order: an ID names itself, and
/// search text the one message it finds. They are looked up one at a time,
/// and the first that fails stops the rest.
pub async fn resolve_all(client: &Client, refs: &[String]) -> Result<Vec<String>> {
    futures::stream::iter(refs)
        .then(|reference| client.resolve_ref(reference))
        .try_collect()
        .await
}
