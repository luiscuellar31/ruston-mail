//! `ruston-cli`: the Ruston Mail account from a terminal, a command at a time
//! or in the terminal interface, with `ruston-core` doing the work as it does
//! for the desktop app.

mod cli;
mod commands;
mod demo;
mod hv;
mod interrupt;
mod mailbox;
mod render;
mod terminal;
mod tui;

use clap::{CommandFactory, Parser};
use cli::{Cli, Command, Ctx, TuiArgs};
use ruston_core::Result;

/// The exit status of a usage error, as clap uses it.
const USAGE_ERROR: i32 = 2;

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
    let opens_interface = matches!(cli.command, None | Some(Command::Tui(_)));
    if opens_interface && !tui::has_terminal() {
        // Without a command and away from a terminal, as in a script, the
        // help is a usage error, as it was before the interface existed.
        if cli.command.is_none() {
            terminal::eprint!("{}", Cli::command().render_help());
        } else {
            terminal::eprintln!(
                "error: the terminal interface needs a terminal; run a command such as `ruston-cli messages list` instead"
            );
        }
        std::process::exit(USAGE_ERROR);
    }
    if !opens_interface {
        // Standard error takes the log, so standard output carries only what
        // the command prints, as text or JSON. The interface draws over the
        // terminal, so it logs nothing.
        terminal::init_tracing(cli.verbose);
        // The interface reads Ctrl+C as a key; the demo has no session.
        let ctx = Ctx::from(&cli);
        if !ctx.demo {
            tokio::spawn(interrupt::end_on_ctrl_c(ctx.profile));
        }
    }

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

/// Hands the parsed command to the offline demo, or to the handlers that work
/// with the account.
async fn dispatch(cli: Cli) -> Result<()> {
    let ctx: Ctx = (&cli).into();
    // `main` has made sure a terminal is there to open the interface in.
    let command = cli
        .command
        .unwrap_or_else(|| Command::Tui(TuiArgs::default()));
    if ctx.demo {
        demo::dispatch(&ctx, command).await
    } else {
        commands::run(&ctx, command).await
    }
}

#[cfg(test)]
mod tests {
    use super::cli::Cli;
    use clap::Parser;

    /// Runs `args`, the command line after `ruston-cli --demo`, as `main`
    /// would.
    async fn run_demo(args: &[&str]) -> ruston_core::Result<()> {
        let line = ["ruston-cli", "--demo"].iter().chain(args);
        super::dispatch(Cli::try_parse_from(line).expect("a valid command line")).await
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
        assert!(run_demo(&["whoami"]).await.is_ok());
    }

    #[tokio::test]
    async fn demo_dispatch_messages_list_and_read() {
        assert!(run_demo(&["messages", "list"]).await.is_ok());
        assert!(run_demo(&["messages", "read", "msg-demo-01"]).await.is_ok());
    }

    #[tokio::test]
    async fn demo_dispatch_json_mode() {
        assert!(run_demo(&["--json", "counts"]).await.is_ok());
    }
}
