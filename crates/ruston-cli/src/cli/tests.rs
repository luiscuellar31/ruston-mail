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
        &["ruston-cli", "whoami", "--captcha-manual"],
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
        "--captcha-manual",
    ])
    .cmd() else {
        panic!("not a login");
    };
    assert_eq!(login.api_url.as_deref(), Some("https://mail.example/api"));
    assert!(matches!(login.client, Some(ClientPreset::Web)));
    assert!(login.captcha_manual);
}

#[test]
fn a_captcha_is_solved_on_protons_verification_page_unless_asked_otherwise() {
    let login = |args: &[&str]| match parse(args).cmd() {
        Command::Login(login) => login,
        _ => panic!("not a login"),
    };
    assert!(!login(&["ruston-cli", "login"]).captcha_manual);
    // Asked for by name before it was the default, it is still taken.
    assert!(!login(&["ruston-cli", "login", "--captcha-chrome"]).captcha_manual);
    assert!(
        Cli::try_parse_from([
            "ruston-cli",
            "login",
            "--captcha-chrome",
            "--captcha-manual"
        ])
        .is_err()
    );
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
                let error =
                    Cli::try_parse_from(["ruston-cli", command, "search", flag, date]).unwrap_err();
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
    let error = Cli::try_parse_from(["ruston-cli", "attachments", "download", "msg"]).unwrap_err();
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
