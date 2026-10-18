//! Exercise the actual CLI output boundary without contacting an account.

use ruston_core::mail::attachments::is_invisible_mark;
use std::process::{Command, Output};

const ATTACK: &str =
    "mail\x1b]52;c;Y2xpcGJvYXJk\x07\u{9d}title\u{9c}\u{9b}2J\r\x08\u{202e}fdp.scr\u{2066}\0";

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
        assert!(
            output.stderr.is_empty(),
            "{args:?} wrote {}",
            String::from_utf8_lossy(&output.stderr)
        );
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
        serde_json::json!({"status": "ok", "id": "draft-demo-saved-01"})
    );
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
