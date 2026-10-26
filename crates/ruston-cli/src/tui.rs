//! The terminal interface: folders, conversations and a reader in one screen,
//! drawn with ratatui over crossterm. Its state and keys live in `app`, how
//! it looks in `view`; this module owns the terminal, the session and the
//! event loop, and runs the work `app` hands out.

mod app;
mod reading;
mod view;

use crate::cli::{Ctx, LoginArgs};
use crate::commands;
use crate::mailbox::Mailbox;
use crate::terminal;
use app::{App, Effect, Message};
use clap::{Args, FromArgMatches};
use crossterm::event::EventStream;
use futures::StreamExt;
use reading::Reading;
use ruston_core::{Client, Error, Result};
use std::io::{self, IsTerminal};
use std::sync::Arc;
use tokio::sync::mpsc::{self, UnboundedSender};

/// Whether standard input and output are both a terminal, as the interface
/// needs: it reads keys from one and draws over the other.
pub(crate) fn has_terminal() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

/// Opens the interface and runs it until it is closed. Signing in, when no
/// session is saved, happens first, as text. The terminal is put back as it
/// was however the interface ends: a panic restores it through the hook
/// `ratatui::try_init` installs, before the panic is reported.
pub(crate) async fn run(ctx: &Ctx) -> Result<()> {
    let mailbox = if ctx.demo {
        Mailbox::Demo
    } else {
        Mailbox::Proton(Arc::new(signed_in(ctx).await?))
    };
    let mut app = App::new(mailbox.account());
    let theme = view::Theme::from_env();
    let mut terminal = ratatui::try_init()?;
    let result = run_loop(&mut terminal, &mut app, &theme, &mailbox).await;
    ratatui::restore();
    result
}

/// The profile's saved session, after signing in as `ruston-cli login` does
/// when there is none.
async fn signed_in(ctx: &Ctx) -> Result<Client> {
    match Client::resume(&ctx.profile).await {
        Err(Error::Unauthorized) => {
            terminal::eprintln!("No session is saved for this profile: sign in first.");
            commands::auth::login(ctx, login_options()?).await?;
            Client::resume(&ctx.profile).await
        }
        resumed => resumed,
    }
}

/// What `ruston-cli login` takes when given no options: the secrets that
/// may be in the environment, and nothing else.
fn login_options() -> Result<LoginArgs> {
    let matches = LoginArgs::augment_args(clap::Command::new("login"))
        .try_get_matches_from(["login"])
        .map_err(|error| Error::Other(error.to_string()))?;
    LoginArgs::from_arg_matches(&matches).map_err(|error| Error::Other(error.to_string()))
}

/// Draws, then waits for a key or for an answer to work handed out, until
/// the person quits or the terminal stops sending events. A resize needs no
/// message: the next draw fits the new size.
async fn run_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    theme: &view::Theme,
    mailbox: &Mailbox,
) -> Result<()> {
    let (sender, mut answers) = mpsc::unbounded_channel();
    let mut events = EventStream::new();
    for effect in app.start() {
        spawn(effect, mailbox, &sender);
    }
    while !app.quit {
        terminal.draw(|frame| view::draw(frame, app, theme))?;
        let message = tokio::select! {
            event = events.next() => match event {
                Some(event) => Message::from_event(&event?),
                None => break,
            },
            Some(answer) = answers.recv() => Some(answer),
        };
        if let Some(message) = message {
            for effect in app.update(message) {
                spawn(effect, mailbox, &sender);
            }
        }
    }
    Ok(())
}

/// Runs `effect` in a task of its own, which sends its answer back as a
/// message. Once the interface has closed, nobody waits for it.
fn spawn(effect: Effect, mailbox: &Mailbox, answers: &UnboundedSender<Message>) {
    let (mailbox, answers) = (mailbox.clone(), answers.clone());
    tokio::spawn(async move {
        let answer = match effect {
            Effect::Places => {
                Message::PlacesLoaded(mailbox.places().await.map_err(|error| error.to_string()))
            }
            Effect::Conversations { request, place } => Message::ConversationsLoaded {
                request,
                result: mailbox
                    .conversations(&place, 0)
                    .await
                    .map_err(|error| error.to_string()),
            },
            // The reading is made here, off the loop: a large HTML body
            // takes a while to turn into text.
            Effect::Conversation { request, id } => Message::ConversationLoaded {
                request,
                result: mailbox
                    .conversation(&id)
                    .await
                    .map(|(conversation, messages)| Reading::new(&conversation, &messages))
                    .map_err(|error| error.to_string()),
            },
        };
        let _ = answers.send(answer);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signing_in_first_takes_what_login_takes_without_options() {
        let options = login_options().unwrap();
        assert_eq!(options.api_url, None);
        assert!(!options.captcha_chrome);
    }
}
