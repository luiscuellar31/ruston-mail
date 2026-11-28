//! Where the desktop's diagnostics and fatal errors go. On Windows, Ruston is
//! a window app without a console of its own: standard error reaches nobody
//! unless the caller hands one down (`cargo run`, a redirect), so a failure
//! that would only be printed is also shown in a window.

use std::fmt::Display;

#[cfg(target_os = "windows")]
mod windows;

/// Lets HTTP diagnostics reach the terminal Ruston was started from. A
/// redirected standard error keeps its destination.
#[cfg(target_os = "windows")]
pub fn show_debug_output() {
    windows::attach_parent_console();
}

/// Standard error already reaches the terminal Ruston was started from.
#[cfg(not(target_os = "windows"))]
pub fn show_debug_output() {}

/// Runs the app, saying in a window when it stops with a panic. The panic
/// still ends the process.
#[cfg(target_os = "windows")]
pub fn report_panics<T>(run: impl FnOnce() -> T) -> T {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        windows::remember_panic(info);
        hook(info);
    }));
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)) {
        Ok(value) => value,
        Err(panic) => {
            windows::show_error(&format!(
                "Ruston Mail stopped because of an internal error.\n\n{}",
                windows::panic_description(panic.as_ref())
            ));
            std::panic::resume_unwind(panic)
        }
    }
}

/// Runs the app; a panic is printed, as standard error reaches someone.
#[cfg(not(target_os = "windows"))]
pub fn report_panics<T>(run: impl FnOnce() -> T) -> T {
    run()
}

/// Reports why Ruston could not start, or could not hand a link to the
/// window already open.
pub fn report_failure(error: &dyn Display) {
    eprintln!("Error: {error}");
    show_failure(&format!("Ruston Mail could not start.\n\n{error}"));
}

/// Shows `text` in a window, where standard error reaches nobody.
#[cfg(target_os = "windows")]
fn show_failure(text: &str) {
    windows::show_error(text);
}

/// Standard error, which `report_failure` wrote to, is enough here.
#[cfg(not(target_os = "windows"))]
fn show_failure(_text: &str) {}
