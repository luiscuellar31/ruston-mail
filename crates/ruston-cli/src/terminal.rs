//! CLI output boundary: readable text is safe even when piped into a terminal.

use std::fmt::{self, Write as _};
use std::io::{self, IsTerminal};
use tracing::{Event, Subscriber};
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields, format::Writer};
use tracing_subscriber::registry::LookupSpan;

/// Filter the complete formatted output, including every interpolated field.
pub(crate) struct SafeArgs<'a>(pub(crate) fmt::Arguments<'a>);

impl fmt::Display for SafeArgs<'_> {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::write(
            &mut ControlFilter {
                output,
                json: false,
            },
            self.0,
        )
    }
}

/// Escape controls left literal by JSON serialization without changing values.
pub(crate) struct JsonText<'a>(pub(crate) &'a str);

impl fmt::Display for JsonText<'_> {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        ControlFilter { output, json: true }.write_str(self.0)
    }
}

struct ControlFilter<'a> {
    output: &'a mut dyn fmt::Write,
    json: bool,
}

impl fmt::Write for ControlFilter<'_> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let mut start = 0;
        for (index, c) in text.char_indices() {
            if c.is_control() && !matches!(c, '\n' | '\t') {
                self.output.write_str(&text[start..index])?;
                if self.json {
                    std::write!(self.output, "\\u{:04x}", u32::from(c))?;
                }
                start = index + c.len_utf8();
            }
        }
        self.output.write_str(&text[start..])
    }
}

macro_rules! print {
    ($($arg:tt)*) => {
        std::print!("{}", $crate::terminal::SafeArgs(format_args!($($arg)*)))
    };
}

macro_rules! println {
    () => { std::println!() };
    ($($arg:tt)*) => {
        std::println!("{}", $crate::terminal::SafeArgs(format_args!($($arg)*)))
    };
}

macro_rules! eprint {
    ($($arg:tt)*) => {
        std::eprint!("{}", $crate::terminal::SafeArgs(format_args!($($arg)*)))
    };
}

macro_rules! eprintln {
    ($($arg:tt)*) => {
        std::eprintln!("{}", $crate::terminal::SafeArgs(format_args!($($arg)*)))
    };
}

macro_rules! write {
    ($output:expr, $($arg:tt)*) => {
        std::write!($output, "{}", $crate::terminal::SafeArgs(format_args!($($arg)*)))
    };
}

macro_rules! writeln {
    ($output:expr, $($arg:tt)*) => {
        std::writeln!($output, "{}", $crate::terminal::SafeArgs(format_args!($($arg)*)))
    };
}

pub(crate) use {eprint, eprintln, print, println, write, writeln};

/// Preserve explicit body formats only when stdout is not a terminal.
pub(crate) fn print_body(body: &str, explicit_format: bool) {
    let stdout = io::stdout();
    let is_terminal = stdout.is_terminal();
    write_body(&mut stdout.lock(), body, explicit_format, is_terminal)
        .expect("failed printing message body to stdout");
}

fn write_body(
    output: &mut impl io::Write,
    body: &str,
    explicit_format: bool,
    is_terminal: bool,
) -> io::Result<()> {
    if explicit_format && !is_terminal {
        output.write_all(body.as_bytes())?;
    } else {
        std::write!(output, "{}", SafeArgs(format_args!("{body}")))?;
    }
    if !body.ends_with('\n') {
        output.write_all(b"\n")?;
    }
    Ok(())
}

/// Filter diagnostics after formatting, including display fields and span data.
struct SafeEvents;

impl<S, N> FormatEvent<S, N> for SafeEvents
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        context: &FmtContext<'_, S, N>,
        mut output: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let mut filter = ControlFilter {
            output: &mut output,
            json: false,
        };
        tracing_subscriber::fmt::format()
            .with_ansi(false)
            .format_event(context, Writer::new(&mut filter), event)
    }
}

pub(crate) fn init_tracing(verbose: u8) {
    use tracing_subscriber::EnvFilter;

    let filter = if std::env::var("RUST_LOG").is_ok() {
        EnvFilter::from_default_env()
    } else {
        let levels = match verbose {
            0 => "warn",
            1 => "info,ruston_core=debug,ruston_cli=debug",
            2 => "info,ruston_core=trace,ruston_cli=debug",
            _ => "trace",
        };
        EnvFilter::new(levels)
    };
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .event_format(SafeEvents)
        .with_writer(io::stderr)
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    const ATTACK: &str = "before\x1b]52;c;Y2xpcGJvYXJk\x07\x1b[2J\x1b]0;title\x1b\\\u{9d}52;c;payload\u{9c}\u{9b}2J\rafter\x08\0\x7f";

    fn assert_safe(text: &str) {
        assert!(
            text.chars()
                .all(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        );
    }

    #[test]
    fn filters_formatted_fields_and_all_controls_except_layout() {
        let output = format!(
            "{}",
            SafeArgs(format_args!(
                "Subject: {ATTACK}\n\tFrom: {}",
                "José <a@example.test>"
            ))
        );
        assert_safe(&output);
        assert!(output.contains("José <a@example.test>"));
        assert!(output.contains("\n\tFrom:"));
        for c in (0..=0x9f)
            .filter_map(char::from_u32)
            .filter(|c| c.is_control())
        {
            let output = format!("{}", SafeArgs(format_args!("a{c}b")));
            assert_eq!(
                output,
                if matches!(c, '\n' | '\t') {
                    format!("a{c}b")
                } else {
                    "ab".into()
                }
            );
        }
    }

    #[test]
    fn explicit_body_formats_preserve_data_only_outside_a_terminal() {
        let body = format!("{ATTACK}\n");
        for (explicit, terminal) in [(false, false), (false, true), (true, true), (true, false)] {
            let mut output = Vec::new();
            write_body(&mut output, &body, explicit, terminal).unwrap();
            let text = String::from_utf8(output).unwrap();
            if explicit && !terminal {
                assert_eq!(text, body);
            } else {
                assert_safe(&text);
                assert!(text.ends_with('\n'));
            }
        }
    }

    #[test]
    fn json_output_is_safe_and_round_trips_original_values() {
        let value = serde_json::json!({"body": ATTACK, "subject": "Olá\u{85}\x7f\n\t"});
        let serialized = serde_json::to_string_pretty(&value).unwrap();
        let displayed = format!("{}", JsonText(&serialized));
        assert_safe(&displayed);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&displayed).unwrap(),
            value
        );
    }

    #[test]
    fn diagnostics_filter_message_display_fields_and_span_fields() {
        use std::sync::{Arc, Mutex};

        struct Capture(Arc<Mutex<Vec<u8>>>);
        impl io::Write for Capture {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let captured = Arc::new(Mutex::new(Vec::new()));
        let writer = captured.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .event_format(SafeEvents)
            .with_writer(move || Capture(writer.clone()))
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("account", address = %ATTACK);
            let _entered = span.enter();
            tracing::warn!(remote_id = %ATTACK, "error: {ATTACK}");
        });
        let text = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
        assert!(text.contains("remote_id"));
        assert!(text.contains("address"));
        assert_safe(&text);
    }
}
