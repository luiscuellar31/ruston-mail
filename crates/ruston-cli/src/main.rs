//! ruston-cli — a clap v4 command-line front-end over `ruston-core`.

mod cli;
mod commands;
mod demo;
mod hv;
mod render;
mod terminal;

use clap::Parser;
use cli::{Cli, Command, Ctx};
use ruston_core::Result;

#[tokio::main]
async fn main() {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            if error.use_stderr() {
                terminal::eprint!("{error}");
            } else {
                terminal::print!("{error}");
            }
            std::process::exit(error.exit_code());
        }
    };
    // Logs go to stderr so stdout stays clean for text/JSON output.
    // -v debug, -vv trace(core), -vvv trace(all); RUST_LOG overrides.
    terminal::init_tracing(cli.verbose);

    if let Err(e) = dispatch(cli).await {
        terminal::eprintln!("error: {}", with_causes(&e));
        std::process::exit(e.exit_code());
    }
}

/// The error and each cause beneath it, so a failed request says whether DNS,
/// the connection, TLS or a timeout failed instead of only "error sending
/// request". A cause the text already includes is not repeated.
fn with_causes(error: &ruston_core::Error) -> String {
    let mut text = error.to_string();
    let mut cause = std::error::Error::source(error);
    while let Some(next) = cause {
        let message = next.to_string();
        if !text.contains(&message) {
            text.push_str(": ");
            text.push_str(&message);
        }
        cause = next.source();
    }
    text
}

async fn dispatch(cli: Cli) -> Result<()> {
    let ctx: Ctx = (&cli).into();
    if ctx.demo {
        return demo::dispatch(&ctx, cli.command).await;
    }
    match cli.command {
        Command::Login => commands::auth::login(&ctx).await,
        Command::Logout => commands::auth::logout(&ctx).await,
        Command::Whoami => commands::auth::whoami(&ctx).await,
        Command::Messages { cmd } => commands::messages::run(&ctx, cmd).await,
        Command::Conversations { cmd } => commands::conversations::run(&ctx, cmd).await,
        Command::Attachments { cmd } => commands::attachments::run(&ctx, cmd).await,
        Command::Drafts { cmd } => commands::drafts::run(&ctx, cmd).await,
        Command::Filters { cmd } => commands::filters::run(&ctx, cmd).await,
        Command::Contacts { cmd } => commands::contacts::run(&ctx, cmd).await,
        Command::Addresses { cmd } => commands::contacts::addresses(&ctx, cmd).await,
        Command::Settings { cmd } => commands::contacts::settings(&ctx, cmd).await,
        Command::Counts { conversations } => commands::contacts::counts(&ctx, conversations).await,
        Command::Export { folder, out, max } => commands::export::run(&ctx, folder, out, max).await,
        Command::Watch { interval, folder } => commands::watch::run(&ctx, interval, folder).await,
        Command::Index {
            folder,
            max_pages,
            page_size,
        } => commands::search::index(&ctx, folder, max_pages, page_size).await,
        Command::Search { query, limit } => commands::search::search(&ctx, query, limit).await,
        Command::Sync {
            backfill,
            max_pages,
            page_size,
        } => commands::sync::run(&ctx, backfill, max_pages, page_size).await,
        Command::Labels { cmd } => commands::labels::run(&ctx, cmd).await,
    }
}

#[cfg(test)]
mod tests {
    use super::cli::Cli;
    use clap::Parser;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(args).expect("args should parse")
    }

    #[tokio::test]
    async fn a_failed_request_says_why_without_saying_where() {
        use ruston_core::{Doer, HttpClient, Request};

        // A port nothing listens on: the connection is refused at once.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let error = HttpClient::new(format!("http://127.0.0.1:{port}"), "test")
            .do_raw(Request::get("/core/v4/keys/all").query("Email", "private@example.test"))
            .await
            .unwrap_err();
        let text = super::with_causes(&error);

        assert!(
            text.starts_with("http error: error sending request: "),
            "{text}"
        );
        assert_eq!(text.matches("error sending request").count(), 1, "{text}");
        assert!(text.contains("tcp connect error"), "{text}");
        for private in [
            "127.0.0.1",
            &port.to_string(),
            "keys/all",
            "private@",
            "Email",
        ] {
            assert!(!text.contains(private), "{text}");
        }
    }

    #[tokio::test]
    async fn demo_dispatch_whoami() {
        let cli = parse(&["ruston-cli", "--demo", "whoami"]);
        assert!(super::dispatch(cli).await.is_ok());
    }

    #[tokio::test]
    async fn demo_dispatch_messages_list_and_read() {
        let cli = parse(&["ruston-cli", "--demo", "messages", "list"]);
        assert!(super::dispatch(cli).await.is_ok());

        let cli = parse(&["ruston-cli", "--demo", "messages", "read", "msg-demo-01"]);
        assert!(super::dispatch(cli).await.is_ok());
    }

    #[tokio::test]
    async fn demo_dispatch_json_mode() {
        let cli = parse(&["ruston-cli", "--demo", "--json", "counts"]);
        assert!(super::dispatch(cli).await.is_ok());
    }
}
