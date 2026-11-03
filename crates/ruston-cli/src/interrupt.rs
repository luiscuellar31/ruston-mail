//! Ctrl+C in the commands that do not open the terminal interface, which
//! takes it as a key instead.

use crate::terminal;
use std::time::Duration;

/// How long Ctrl+C waits before saying what it waits for.
const QUIET_WAIT: Duration = Duration::from_millis(250);

/// The status Windows ends a console program with on Ctrl+C.
#[cfg(windows)]
const STATUS_CONTROL_C_EXIT: u32 = 0xC000_013A;

/// Ends the program on Ctrl+C as the terminal would, once no token refresh
/// of `profile`'s session is under way: by then Proton has spent the old
/// refresh token, and ending before the new one is saved would sign the
/// profile out. A second Ctrl+C ends it at once.
pub(crate) async fn end_on_ctrl_c(profile: String) {
    // Without a handler, Ctrl+C keeps its default and ends the program.
    if tokio::signal::ctrl_c().await.is_err() {
        return;
    }
    let settled = async {
        let wait = ruston_core::Client::settle_session(&profile);
        tokio::pin!(wait);
        if tokio::time::timeout(QUIET_WAIT, &mut wait).await.is_err() {
            terminal::eprintln!(
                "finishing the session's token refresh before quitting (Ctrl-C again to quit now)…"
            );
            // A wait that gives up ends the program all the same.
            let _ = wait.await;
        }
    };
    tokio::select! {
        () = settled => {}
        _ = tokio::signal::ctrl_c() => {}
    }
    end_interrupted();
}

/// Ends the program as Ctrl+C's default would, so a shell or script that
/// runs it sees it interrupted.
fn end_interrupted() -> ! {
    #[cfg(unix)]
    {
        let _ = signal_hook::low_level::emulate_default_handler(signal_hook::consts::SIGINT);
        // Not reached: SIGINT's default action ends the program. 130 is how
        // a shell reports it.
        std::process::exit(130);
    }
    #[cfg(windows)]
    std::process::exit(STATUS_CONTROL_C_EXIT as i32);
}
