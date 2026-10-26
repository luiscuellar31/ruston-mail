//! The terminal interface: folders, conversations and a reader in one screen,
//! drawn with ratatui over crossterm. Its state and keys live in `app`, how
//! it looks in `view`; this module owns the terminal and the event loop.

mod app;
mod view;

use crate::cli::Ctx;
use app::{App, Message};
use crossterm::event::EventStream;
use futures::StreamExt;
use ruston_core::Result;
use std::io::{self, IsTerminal};

/// Whether standard input and output are both a terminal, as the interface
/// needs: it reads keys from one and draws over the other.
pub(crate) fn has_terminal() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

/// Opens the interface and runs it until it is closed. The terminal is put
/// back as it was however the interface ends: a panic restores it through
/// the hook `ratatui::try_init` installs, before the panic is reported.
pub(crate) async fn run(ctx: &Ctx) -> Result<()> {
    let account = if ctx.demo {
        "demo".to_owned()
    } else {
        ctx.profile.clone()
    };
    let mut app = App::new(account);
    let theme = view::Theme::from_env();
    let mut terminal = ratatui::try_init()?;
    let result = run_loop(&mut terminal, &mut app, &theme).await;
    ratatui::restore();
    result
}

/// Draws, then waits for the next terminal event, until the person quits or
/// the terminal stops sending events. A resize needs no message: the next
/// draw fits the new size.
async fn run_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    theme: &view::Theme,
) -> Result<()> {
    let mut events = EventStream::new();
    while !app.quit {
        terminal.draw(|frame| view::draw(frame, app, theme))?;
        let Some(event) = events.next().await else {
            break;
        };
        if let Some(message) = Message::from_event(&event?) {
            app.update(message);
        }
    }
    Ok(())
}
