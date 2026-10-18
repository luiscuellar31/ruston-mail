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
    use super::cli::{
        Cli, Command, ConversationsCmd, DraftsCmd, EmptyFolder, MessagesCmd, ReadFormat, ReadState,
    };
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
        match cli.command {
            Command::Messages {
                cmd: MessagesCmd::Send(a),
            } => {
                assert_eq!(a.message.to, vec!["a@b.com"]);
                assert_eq!(a.message.subject, "x");
                assert_eq!(a.message.body, "-");
                assert!(!a.message.html);
            }
            _ => panic!("wrong command"),
        }
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
        match cli.command {
            Command::Messages {
                cmd: MessagesCmd::Send(a),
            } => {
                assert_eq!(a.message.to.len(), 2);
                assert_eq!(a.message.cc, vec!["e@f.com"]);
                assert!(a.message.html);
                assert_eq!(a.message.attach.len(), 2);
            }
            _ => panic!("wrong command"),
        }
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
        match parse(&draft).command {
            Command::Drafts {
                cmd: DraftsCmd::Save(a),
            } => assert_eq!(a.attach.len(), 1),
            _ => panic!("wrong command"),
        }
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
        match cli.command {
            Command::Messages {
                cmd:
                    MessagesCmd::Read {
                        reference,
                        format,
                        body_only,
                        ..
                    },
            } => {
                assert_eq!(reference, "REF");
                assert!(matches!(format, ReadFormat::Html));
                assert!(!body_only);
            }
            _ => panic!("wrong command"),
        }
    }

    #[test]
    fn read_defaults_to_text() {
        let cli = parse(&["ruston-cli", "messages", "read", "abc"]);
        match cli.command {
            Command::Messages {
                cmd: MessagesCmd::Read { format, .. },
            } => {
                assert!(matches!(format, ReadFormat::Text));
            }
            _ => panic!("wrong command"),
        }
    }

    #[test]
    fn move_requires_dest() {
        let cli = parse(&["ruston-cli", "messages", "move", "REF", "--dest", "archive"]);
        match cli.command {
            Command::Messages {
                cmd: MessagesCmd::Move { references, dest },
            } => {
                assert_eq!(references, vec!["REF"]);
                assert_eq!(dest, "archive");
            }
            _ => panic!("wrong command"),
        }
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
        match parse(&["ruston-cli", "messages", "empty", "spam", "--yes"]).command {
            Command::Messages {
                cmd: MessagesCmd::Empty { folder, yes },
            } => {
                assert!(matches!(folder, EmptyFolder::Spam));
                assert!(yes);
            }
            _ => panic!("wrong command"),
        }
        match parse(&["ruston-cli", "messages", "delete", "id1"]).command {
            Command::Messages {
                cmd: MessagesCmd::Delete { references, yes },
            } => {
                assert_eq!(references, vec!["id1"]);
                assert!(!yes, "deleting asks unless told not to");
            }
            _ => panic!("wrong command"),
        }
        // Deleting drafts names them and asks the same way.
        assert!(Cli::try_parse_from(["ruston-cli", "drafts", "delete"]).is_err());
        match parse(&["ruston-cli", "drafts", "delete", "draft1"]).command {
            Command::Drafts {
                cmd: DraftsCmd::Delete { ids, yes },
            } => {
                assert_eq!(ids, vec!["draft1"]);
                assert!(!yes, "deleting asks unless told not to");
            }
            _ => panic!("wrong command"),
        }
    }

    #[test]
    fn mark_state_then_refs() {
        let cli = parse(&["ruston-cli", "messages", "mark", "read", "id1", "id2"]);
        match cli.command {
            Command::Messages {
                cmd: MessagesCmd::Mark { state, references },
            } => {
                assert!(matches!(state, ReadState::Read));
                assert!(state.as_bool());
                assert_eq!(references, vec!["id1", "id2"]);
            }
            _ => panic!("wrong command"),
        }
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
        match cli.command {
            Command::Messages {
                cmd:
                    MessagesCmd::List {
                        folder,
                        page_size,
                        unread,
                        ..
                    },
            } => {
                assert_eq!(folder, "archive");
                assert_eq!(page_size, 10);
                assert!(unread);
            }
            _ => panic!("wrong command"),
        }
    }

    #[test]
    fn conversations_mark_default_folder() {
        let cli = parse(&["ruston-cli", "conversations", "mark", "unread", "conv1"]);
        match cli.command {
            Command::Conversations {
                cmd: ConversationsCmd::Mark { state, folder, ids },
            } => {
                assert!(matches!(state, ReadState::Unread));
                assert_eq!(folder, "all");
                assert_eq!(ids, vec!["conv1"]);
            }
            _ => panic!("wrong command"),
        }
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
    }

    #[test]
    fn help_explains_every_exit_status() {
        let help = Cli::command().render_help().to_string();
        for status in ["0  ", "1  ", "2  ", "3  ", "4  ", "5  "] {
            assert!(help.contains(status), "status {status}is not explained");
        }
    }

    #[test]
    fn index_leaves_trash_and_spam_out_unless_asked() {
        use ruston_core::model::enums::{label_ids, resolve_folder};

        let folder = |args: &[&str]| match parse(args).command {
            Command::Index { folder, .. } => resolve_folder(&folder),
            _ => panic!("wrong command"),
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
        match cli.command {
            Command::Labels {
                cmd:
                    super::cli::LabelsCmd::Create {
                        name,
                        color,
                        folder,
                        parent,
                    },
            } => {
                assert_eq!(name, "Work");
                assert_eq!(color, "#8080FF");
                assert!(!folder);
                assert!(parent.is_none());
            }
            _ => panic!("wrong command"),
        }
    }

    #[test]
    fn render_time_formats() {
        // 2021-01-01T00:00:00Z
        assert_eq!(
            super::render::fmt_time(1_609_459_200),
            "2021-01-01 00:00:00 UTC"
        );
        assert_eq!(super::render::fmt_time(0), "-");
    }

    #[test]
    fn demo_flag_parsed() {
        let cli = parse(&["ruston-cli", "--demo", "whoami"]);
        assert!(cli.demo);
        let ctx: super::cli::Ctx = (&cli).into();
        assert!(ctx.demo);
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
