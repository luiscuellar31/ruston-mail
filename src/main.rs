// A window app on Windows: launching it never opens a console, whose closing
// would end Ruston without asking about a written message.
#![cfg_attr(all(target_os = "windows", not(test)), windows_subsystem = "windows")]

mod app;
mod diagnostics;
mod downloads;
mod file_manager;
mod launch;
mod mail;
mod mailto;
mod runtime;
mod settings;
mod ui;

use std::process::ExitCode;

use tracing_subscriber::EnvFilter;

/// Set to any value to print Proton HTTP diagnostics to stderr.
const DEBUG_HTTP_ENV: &str = "RUSTON_DEBUG_HTTP";

/// Only ruston-core's HTTP target: method, path, body kind, status, size and
/// timing. Other ruston-core targets log usernames, session UIDs and
/// addresses, so the filter is fixed and `RUST_LOG` is deliberately ignored.
const DEBUG_HTTP_FILTER: &str = "ruston_core::http=debug";

fn main() -> ExitCode {
    if std::env::var_os(DEBUG_HTTP_ENV).is_some() {
        diagnostics::show_debug_output();
        let _ = tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::new(DEBUG_HTTP_FILTER))
            .with_writer(std::io::stderr)
            .try_init();
    }

    match diagnostics::report_panics(start) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            diagnostics::report_failure(&error);
            ExitCode::FAILURE
        }
    }
}

fn start() -> Result<(), Box<dyn std::error::Error>> {
    let demo =
        std::env::var(ruston_core::DEMO_ENV).is_ok_and(|value| ruston_core::demo_requested(&value));
    let urls = launch::arguments(std::env::args_os().skip(1))?;
    match launch::Instance::start(demo, urls)? {
        launch::Start::Forwarded => Ok(()),
        launch::Start::Primary(instance) => Ok(ui::run(demo, instance)?),
    }
}
