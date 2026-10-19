//! Where the desktop's diagnostics and fatal errors go. On Windows, Ruston is
//! a window app without a console of its own: standard error reaches nobody
//! unless the caller hands one down (`cargo run`, a redirect), so a failure
//! that would only be printed is also shown in a window.

use std::fmt::Display;

#[cfg(target_os = "windows")]
mod windows;

/// Lets HTTP diagnostics reach the terminal Ruston was started from. A
/// redirected standard error keeps its destination.
pub fn show_debug_output() {
    #[cfg(target_os = "windows")]
    windows::attach_parent_console();
}

/// Runs the app, saying in a window when it stops with a panic. The panic
/// still ends the process.
pub fn report_panics<T>(run: impl FnOnce() -> T) -> T {
    #[cfg(target_os = "windows")]
    {
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
    #[cfg(not(target_os = "windows"))]
    run()
}

/// Reports why Ruston could not start, or could not hand a link to the
/// window already open.
pub fn report_failure(error: &dyn Display) {
    eprintln!("Error: {error}");
    #[cfg(target_os = "windows")]
    windows::show_error(&format!("Ruston Mail could not start.\n\n{error}"));
}
