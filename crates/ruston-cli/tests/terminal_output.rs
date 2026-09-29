//! Exercise the actual CLI output boundary without contacting an account.

use std::process::{Command, Output};

const ATTACK: &str = "mail\x1b]52;c;Y2xpcGJvYXJk\x07\u{9d}title\u{9c}\u{9b}2J\r\x08\0";

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ruston-cli"))
        .args(args)
        .env_remove("RUST_LOG")
        .output()
        .unwrap()
}

fn assert_safe(bytes: &[u8]) -> String {
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(
        text.chars()
            .all(|c| !c.is_control() || matches!(c, '\n' | '\t'))
    );
    text
}

#[test]
fn human_output_filters_untrusted_label_names() {
    // Process arguments cannot contain NUL; it is covered by writer unit tests.
    let attack = ATTACK.trim_end_matches('\0');
    let output = run(&["--demo", "labels", "create", "--name", attack]);
    assert!(output.status.success());
    let text = assert_safe(&output.stdout);
    assert!(text.contains("Created"));
    assert!(text.contains("mail"));
    assert_safe(&output.stderr);
}

#[test]
fn json_output_filters_controls_without_changing_label_data() {
    let attack = ATTACK.trim_end_matches('\0');
    let output = run(&["--demo", "--json", "labels", "create", "--name", attack]);
    assert!(output.status.success());
    let text = assert_safe(&output.stdout);
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["Name"], attack);
    assert_safe(&output.stderr);
}

#[test]
fn runtime_and_argument_errors_filter_controls_on_stderr() {
    let attack = ATTACK.trim_end_matches('\0');
    for args in [vec!["--demo", "messages", "read", attack], vec![attack]] {
        let output = run(&args);
        assert!(!output.status.success());
        assert_safe(&output.stdout);
        let error = assert_safe(&output.stderr);
        assert!(error.contains("error:"));
    }
}
