//! Exercise the actual CLI output boundary without contacting an account.

use ruston_core::mail::attachments::is_invisible_mark;
use std::process::{Command, Output};

const ATTACK: &str =
    "mail\x1b]52;c;Y2xpcGJvYXJk\x07\u{9d}title\u{9c}\u{9b}2J\r\x08\u{202e}fdp.scr\u{2066}\0";

/// What every demo command says on standard error.
const DEMO_NOTICE: &str =
    "note: offline demo (--demo or RUSTON_DEMO); nothing is sent to or changed on Proton\n";

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ruston-cli"))
        .args(args)
        .env_remove("RUST_LOG")
        .output()
        .unwrap()
}

fn assert_safe(bytes: &[u8]) -> String {
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    let unsafe_character = text
        .chars()
        .find(|&c| (c.is_control() && !matches!(c, '\n' | '\t')) || is_invisible_mark(c));
    if let Some(c) = unsafe_character {
        panic!("{c:?} in {text:?}");
    }
    text
}

/// Runs the CLI and expects it to succeed, showing what it said if not.
fn run_ok(args: &[&str]) -> Output {
    let output = run(args);
    assert!(
        output.status.success(),
        "{args:?} ended with {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

/// The attack as a process argument, which cannot contain NUL; writers'
/// unit tests cover that.
fn attack_argument() -> &'static str {
    ATTACK.trim_end_matches('\0')
}

#[test]
fn a_folder_or_label_of_ones_own_is_named_as_people_see_it() {
    let output = run_ok(&["--demo", "--json", "messages", "list", "--folder", "WORK"]);
    let listed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let messages = listed["messages"].as_array().unwrap();
    assert!(!messages.is_empty(), "{listed}");
    for message in messages {
        let labels = message["LabelIDs"].as_array().unwrap();
        assert!(labels.contains(&"label-demo-work".into()), "{message}");
    }

    let unknown = run(&["--demo", "messages", "list", "--folder", "Nowhere"]);
    assert!(!unknown.status.success());
    let error = String::from_utf8(unknown.stderr).unwrap();
    assert!(error.contains("folder or label not found"), "{error}");
}

#[test]
fn settings_emptying_and_a_body_written_to_a_file_say_what_happened() {
    let json = |args: &[&str]| -> serde_json::Value {
        let output = run_ok(&[&["--demo", "--json"][..], args].concat());
        serde_json::from_slice(&output.stdout).unwrap()
    };
    assert_eq!(
        json(&["settings", "sign", "on"]),
        serde_json::json!({"status": "ok", "setting": "sign", "value": true, "demo": true})
    );
    assert_eq!(
        json(&["settings", "attach-public-key", "off"]),
        serde_json::json!({
            "status": "ok", "setting": "attach_public_key", "value": false, "demo": true
        })
    );
    assert_eq!(
        json(&["messages", "empty", "spam", "--yes"]),
        serde_json::json!({"status": "ok", "action": "empty", "folder": "spam", "demo": true})
    );
    let text = assert_safe(&run_ok(&["--demo", "settings", "sign", "off"]).stdout);
    assert_eq!(text, "Sign outgoing mail: off\n");
    let text = assert_safe(&run_ok(&["--demo", "messages", "empty", "trash", "--yes"]).stdout);
    assert_eq!(text, "Emptied Trash.\n");

    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let file = std::env::temp_dir().join(format!(
        "ruston-cli-body-{}-{unique}.txt",
        std::process::id()
    ));
    let path = file.to_str().unwrap();
    let written = json(&["messages", "read", "msg-demo-01", "--output", path]);
    let bytes = std::fs::metadata(&file).unwrap().len();
    std::fs::remove_file(&file).unwrap();
    assert_eq!(
        written,
        serde_json::json!({"status": "ok", "bytes": bytes, "path": path, "demo": true})
    );
}

#[test]
fn each_action_is_named_after_its_command() {
    for (args, action) in [
        (&["messages", "mark", "read", "msg-demo-01"][..], "mark"),
        (
            &["messages", "move", "msg-demo-01", "--dest", "archive"],
            "move",
        ),
        (
            &["messages", "label", "add", "Work", "msg-demo-01"],
            "label",
        ),
        (
            &["messages", "label", "rm", "Work", "msg-demo-01"],
            "unlabel",
        ),
        (&["messages", "undelete", "msg-demo-01"], "undelete"),
        (&["messages", "unsubscribe", "msg-demo-01"], "unsubscribe"),
        (&["messages", "receipt", "msg-demo-01"], "receipt"),
        (
            &["conversations", "move", "conv-demo-01", "--dest", "inbox"],
            "move",
        ),
        (&["conversations", "mark", "unread", "conv-demo-01"], "mark"),
        (&["conversations", "unsnooze", "conv-demo-01"], "unsnooze"),
        (&["drafts", "delete", "draft-demo-01", "--yes"], "delete"),
        (&["filters", "enable", "filter-demo-01"], "enable"),
        (&["filters", "disable", "filter-demo-01"], "disable"),
        (&["filters", "delete", "filter-demo-01"], "delete"),
        (&["labels", "delete", "label-demo-work"], "delete"),
    ] {
        let output = run_ok(&[&["--demo", "--json"][..], args].concat());
        let reported: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(reported["action"], action, "{args:?}");
    }
}

#[test]
fn a_demo_page_far_past_the_end_lists_nothing() {
    for list in ["messages", "conversations", "drafts", "contacts"] {
        let args = [
            "--demo",
            "--json",
            list,
            "list",
            "--page",
            "4294967295",
            "--page-size",
            "100",
        ];
        let output = run_ok(&args);
        let text = String::from_utf8(output.stdout).unwrap();
        let listed: serde_json::Value = serde_json::from_str(&text).unwrap();
        let rows = listed
            .as_object()
            .and_then(|object| object.values().find_map(serde_json::Value::as_array))
            .unwrap_or_else(|| panic!("{list}: no rows in {text}"));
        assert!(rows.is_empty(), "{list}: {text}");
    }
}

#[test]
fn human_output_filters_untrusted_label_names() {
    let attack = attack_argument();
    let output = run_ok(&["--demo", "labels", "create", "--name", attack]);
    let text = assert_safe(&output.stdout);
    assert!(text.contains("Created"));
    assert!(text.contains("mail"));
    assert_safe(&output.stderr);
}

#[test]
fn json_output_filters_controls_without_changing_label_data() {
    let attack = attack_argument();
    let output = run_ok(&["--demo", "--json", "labels", "create", "--name", attack]);
    let text = assert_safe(&output.stdout);
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["Name"], attack);
    assert_safe(&output.stderr);
}

#[test]
fn a_reader_that_stops_early_ends_the_command_quietly() {
    for args in [
        &["--demo", "messages", "list"][..],
        &["--demo", "--json", "messages", "list"],
        &["--demo", "messages", "read", "1"],
    ] {
        // Like `| head` once it has its lines: nobody reads what follows.
        let (reader, writer) = std::io::pipe().unwrap();
        drop(reader);
        let output = Command::new(env!("CARGO_BIN_EXE_ruston-cli"))
            .args(args)
            .env_remove("RUST_LOG")
            .stdout(writer)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{args:?} ended with {}",
            output.status
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            DEMO_NOTICE,
            "{args:?} wrote more than the demo notice"
        );
    }
}

#[test]
fn a_demo_says_so_on_stderr_whatever_turned_it_on() {
    let send: Vec<&str> = "messages send --to a@example.org --subject Hi --body Hello"
        .split(' ')
        .collect();
    let flag = run_ok(&[&["--demo", "--json"][..], &send].concat());
    // A variable left set from trying the desktop demo turns it on too.
    let variable = Command::new(env!("CARGO_BIN_EXE_ruston-cli"))
        .args(&send)
        .env_remove("RUST_LOG")
        .env("RUSTON_DEMO", "1")
        .output()
        .unwrap();

    for output in [flag, variable] {
        assert!(output.status.success(), "ended with {}", output.status);
        assert_eq!(String::from_utf8_lossy(&output.stderr), DEMO_NOTICE);
    }
}

#[test]
fn runtime_and_argument_errors_filter_controls_on_stderr() {
    let attack = attack_argument();
    for args in [vec!["--demo", "messages", "read", attack], vec![attack]] {
        let output = run(&args);
        assert!(!output.status.success());
        assert_safe(&output.stdout);
        let error = assert_safe(&output.stderr);
        assert!(error.contains("error:"));
    }
}

#[test]
fn a_second_demo_export_into_the_same_folder_resumes() {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let folder =
        std::env::temp_dir().join(format!("ruston-cli-export-{}-{unique}", std::process::id()));
    let out = folder.to_str().unwrap();
    let args = ["--demo", "export", "--out", out, "--max", "2"];

    let first = run(&args);
    let second = run(&args);
    std::fs::remove_dir_all(&folder).unwrap();

    for output in [&first, &second] {
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let text = assert_safe(&second.stdout);
    assert!(text.contains("Exported 0 demo message(s)"), "{text}");
    assert!(
        text.contains("2 message(s) were already exported there"),
        "{text}"
    );
}

#[test]
fn a_demo_download_never_replaces_a_file_already_there() {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let folder = std::env::temp_dir().join(format!(
        "ruston-cli-download-{}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("translation_proposal.pdf"), b"mine").unwrap();

    let output = run_ok(&[
        "--demo",
        "attachments",
        "download",
        "msg-demo-03",
        "att-demo-01",
        "--output-dir",
        folder.to_str().unwrap(),
    ]);
    let kept = std::fs::read(folder.join("translation_proposal.pdf")).unwrap();
    let saved = std::fs::read(folder.join("translation_proposal (1).pdf"));
    std::fs::remove_dir_all(&folder).unwrap();

    assert_eq!(kept, b"mine");
    assert!(saved.unwrap().starts_with(b"%PDF"));
    // The demo reports a download as the real command does.
    let text = assert_safe(&output.stdout);
    assert!(text.contains("Downloaded 1 file(s):"), "{text}");
    assert!(text.contains("translation_proposal (1).pdf"), "{text}");
}

#[test]
fn a_saved_draft_is_reported_as_saved_not_sent() {
    let args = [
        "--demo",
        "drafts",
        "save",
        "--to",
        "ana@example.org",
        "--subject",
        "Plans",
        "--body",
        "Hi",
    ];
    let human = run_ok(&args);
    assert_eq!(
        assert_safe(&human.stdout),
        "Draft saved (id draft-demo-saved-01)\n"
    );

    let json = run_ok(&[&["--json"][..], &args].concat());
    let value: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(
        value,
        serde_json::json!({"status": "ok", "id": "draft-demo-saved-01", "demo": true})
    );
}

#[test]
fn demo_json_says_so_and_lists_come_as_objects() {
    for (args, list) in [
        (&["labels", "list"][..], "labels"),
        (&["labels", "list", "--folders"], "folders"),
        (&["counts"], "counts"),
        (&["addresses", "list"], "addresses"),
        (&["filters", "list"], "filters"),
        (&["contacts", "emails"], "emails"),
        (&["attachments", "list", "msg-demo-03"], "attachments"),
        (&["messages", "list"], "messages"),
    ] {
        let output = run_ok(&[&["--demo", "--json"][..], args].concat());
        let shown: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(shown[list].is_array(), "{args:?}: {shown}");
        assert_eq!(shown["demo"], true, "{args:?}");
    }
    // A message read in full says so too.
    let output = run_ok(&["--demo", "--json", "messages", "read", "msg-demo-01"]);
    let shown: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(shown["demo"], true);
}

#[test]
fn labels_and_folders_are_listed_apart() {
    let labels = run_ok(&["--demo", "labels", "list"]);
    let labels = assert_safe(&labels.stdout);
    assert!(labels.starts_with("2 label(s)\n"), "{labels}");
    assert!(labels.contains(" Work ") && !labels.contains(" Projects "));

    let folders = run_ok(&["--demo", "labels", "list", "--folders"]);
    let folders = assert_safe(&folders.stdout);
    assert!(folders.starts_with("1 folder(s)\n"), "{folders}");
    assert!(folders.contains(" Projects ") && !folders.contains(" Work "));
}

#[test]
fn reading_a_message_or_a_thread_names_its_verdict_and_ends_each_body_once() {
    let message = run_ok(&["--demo", "messages", "read", "msg-demo-01"]);
    let message = assert_safe(&message.stdout);
    let thread = run_ok(&["--demo", "conversations", "read", "conv-demo-01"]);
    let thread = assert_safe(&thread.stdout);

    assert!(message.contains("\nVerdict: verified\n"), "{message}");
    assert!(message.contains("\n\nHi Demo,\n"), "{message}");
    assert!(message.ends_with("Thanks,\nAlex\n"), "{message}");
    assert_eq!(
        thread.matches("\nVerdict: verified\n").count(),
        2,
        "{thread}"
    );
    assert!(
        thread.contains("Thanks,\nAlex\n\n--- message 2/2 ---\n"),
        "{thread}"
    );
    assert!(thread.ends_with("Thanks,\nDemo\n"), "{thread}");
}
