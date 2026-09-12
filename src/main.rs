mod app;
mod downloads;
mod mail;
mod settings;
mod ui;

use std::ffi::OsStr;

use tracing_subscriber::EnvFilter;

/// Set to any value to print Proton HTTP diagnostics to stderr.
const DEBUG_HTTP_ENV: &str = "RUSTON_DEBUG_HTTP";

/// Only proton-core's HTTP target: method, path, body kind, status, size and
/// timing. Other proton-core targets log usernames, session UIDs and
/// addresses, so the filter is fixed and `RUST_LOG` is deliberately ignored.
const DEBUG_HTTP_FILTER: &str = "proton_core::http=debug";

/// Set to `1` to open a local fictional mailbox instead of Proton.
const DEMO_ENV: &str = "RUSTON_DEMO";

fn main() -> iced::Result {
    if std::env::var_os(DEBUG_HTTP_ENV).is_some() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::new(DEBUG_HTTP_FILTER))
            .with_writer(std::io::stderr)
            .try_init();
    }

    ui::run(demo_requested(std::env::var_os(DEMO_ENV).as_deref()))
}

fn demo_requested(value: Option<&OsStr>) -> bool {
    value == Some(OsStr::new("1"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_requires_explicit_one() {
        assert!(demo_requested(Some(OsStr::new("1"))));

        assert!(!demo_requested(None));
        for value in ["", "0", "true", "yes", " 1", "01"] {
            assert!(!demo_requested(Some(OsStr::new(value))), "{value:?}");
        }
    }
}
