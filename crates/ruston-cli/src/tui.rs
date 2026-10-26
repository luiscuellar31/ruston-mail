//! The terminal interface: folders, conversations and a reader in one screen,
//! drawn with ratatui over crossterm. Its state and keys live in `app`, how
//! it looks in `view`; this module owns the terminal, the session and the
//! event loop, and runs the work `app` hands out.

mod app;
mod command;
mod reading;
mod view;

use crate::cli::{Ctx, LoginArgs, TuiArgs};
use crate::commands;
use crate::mailbox::{Change, Mailbox};
use crate::terminal;
use app::{App, Effect, Message};
use clap::{Args, FromArgMatches};
use crossterm::event::EventStream;
use futures::{FutureExt, StreamExt};
use reading::Reading;
use ruston_core::{Client, Error, Result};
use std::io::{self, IsTerminal};
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::thread::ThreadId;
use std::time::Duration;
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio::task::{AbortHandle, JoinHandle};

/// How long quitting waits at most for what is still on its way.
const QUIT_WAIT: Duration = Duration::from_secs(3);

/// Whether standard input and output are both a terminal, as the interface
/// needs: it reads keys from one and draws over the other.
pub(crate) fn has_terminal() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

/// Opens the interface and runs it until it is closed. Signing in, when no
/// session is saved, happens first, as text. The terminal is put back as it
/// was however the interface ends: a panic of its loop restores it through
/// the hook `ratatui::try_init` installs, before the panic is reported.
pub(crate) async fn run(ctx: &Ctx, args: &TuiArgs) -> Result<()> {
    let mailbox = if ctx.demo {
        Mailbox::demo()
    } else {
        Mailbox::Proton(Arc::new(signed_in(ctx).await?))
    };
    let mut app = App::new(mailbox.account());
    app.marks_read = !args.keep_unread;
    let theme = view::Theme::from_env();
    let mut terminal = ratatui::try_init()?;
    report_panics_of(std::thread::current().id());
    let result = run_loop(&mut terminal, &mut app, &theme, &mailbox).await;
    ratatui::restore();
    // Back to the default hook, which reports any panic as usual.
    drop(std::panic::take_hook());
    result
}

/// Reports only the panics of the interface's loop, on `loop_thread`,
/// through the hook `ratatui::try_init` installs, which puts the terminal
/// back first. A task that panics answers with a message instead, and the
/// interface goes on with its screen, which a report would draw over.
fn report_panics_of(loop_thread: ThreadId) {
    let report = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().id() == loop_thread {
            report(info);
        }
    }));
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
    let mut tasks = Tasks::default();
    for effect in app.start() {
        tasks.run(effect, mailbox, &sender);
    }
    while !app.quit {
        terminal.draw(|frame| view::draw(frame, app, theme))?;
        let message = tokio::select! {
            event = events.next() => match event {
                Some(event) => {
                    let event = event?;
                    #[cfg(unix)]
                    if suspends(&event) {
                        suspend(terminal)?;
                        continue;
                    }
                    app.message_for(&event)
                }
                None => break,
            },
            Some(answer) = answers.recv() => Some(answer),
        };
        if let Some(message) = message {
            for effect in app.update(message) {
                tasks.run(effect, mailbox, &sender);
            }
        }
    }
    if tasks.busy() {
        app.notice = Some("Finishing what is on its way…".to_owned());
        terminal.draw(|frame| view::draw(frame, app, theme))?;
    }
    tasks.finish(mailbox, QUIT_WAIT).await;
    Ok(())
}

/// Opens `url` in the default browser, as signing in opens a challenge,
/// with nothing to read from the terminal the interface holds, and waits for
/// the opener on a thread of its own, so it leaves nothing behind.
fn open_link(url: &str) -> io::Result<()> {
    let mut opener = crate::hv::browser_command(url)?
        .stdin(std::process::Stdio::null())
        .spawn()?;
    std::thread::spawn(move || opener.wait());
    Ok(())
}

/// Whether `event` is Ctrl+Z, which suspends the interface where the shell
/// has job control, as it suspends any program there. In raw mode the
/// terminal sends it as a key instead of stopping the process.
#[cfg(unix)]
fn suspends(event: &crossterm::event::Event) -> bool {
    use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
    matches!(event, Event::Key(key) if key.kind != KeyEventKind::Release
        && key.code == KeyCode::Char('z')
        && key.modifiers.contains(KeyModifiers::CONTROL))
}

/// Puts the terminal back as the shell had it and stops the process, as
/// Ctrl+Z does to any program. Once the shell resumes it, it takes the
/// terminal again through a new `Terminal`, which draws everything anew:
/// clearing the old one would ask the terminal where its cursor is, an
/// answer the event stream's reader can hold up until the next key.
#[cfg(unix)]
fn suspend(terminal: &mut ratatui::DefaultTerminal) -> Result<()> {
    use crossterm::terminal::{Clear, ClearType, EnterAlternateScreen};
    terminal.show_cursor()?;
    ratatui::try_restore()?;
    signal_hook::low_level::raise(signal_hook::consts::SIGTSTP)?;
    crossterm::terminal::enable_raw_mode()?;
    crossterm::execute!(io::stdout(), EnterAlternateScreen, Clear(ClearType::All))?;
    *terminal = ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(io::stdout()))?;
    Ok(())
}

/// The task still running each kind of effect, if any.
#[derive(Default)]
struct Tasks {
    places: Option<AbortHandle>,
    list: Option<AbortHandle>,
    conversation: Option<AbortHandle>,
    /// The changes and links on their way, which quitting waits for.
    writes: Vec<JoinHandle<()>>,
}

impl Tasks {
    /// Runs `effect` as [`spawn`] does. A read aborts the task still running
    /// one of its kind, whose answer would come too late or be overtaken;
    /// that is safe, and a token refresh that one of them starts finishes on
    /// a task of its own. A change to a conversation changes the account,
    /// and opening a link starts a browser, so each always runs to its end:
    /// stopped halfway, nobody would know whether it happened.
    fn run(&mut self, effect: Effect, mailbox: &Mailbox, answers: &UnboundedSender<Message>) {
        let running = match effect {
            Effect::Places => &mut self.places,
            Effect::Conversations { .. } => &mut self.list,
            Effect::Conversation { .. } => &mut self.conversation,
            Effect::MarkRead { .. } | Effect::Change { .. } | Effect::OpenLink { .. } => {
                self.writes.retain(|write| !write.is_finished());
                self.writes.push(spawn(effect, mailbox, answers));
                return;
            }
        };
        let task = spawn(effect, mailbox, answers).abort_handle();
        if let Some(before) = running.replace(task) {
            before.abort();
        }
    }

    /// Whether a change or a link is still on its way.
    fn busy(&self) -> bool {
        self.writes.iter().any(|write| !write.is_finished())
    }

    /// Waits, for at most `limit`, for the changes still on their way, then
    /// for a token refresh under way, which ending the program would cut
    /// short: a change half made, or the session lost.
    async fn finish(self, mailbox: &Mailbox, limit: Duration) {
        let finished = async {
            futures::future::join_all(self.writes).await;
            mailbox.settle().await;
        };
        let _ = tokio::time::timeout(limit, finished).await;
    }
}

/// Runs `effect` in a task of its own, which sends its answer back as a
/// message. Quitting waits a little for those that change the account, and
/// for no other.
fn spawn(effect: Effect, mailbox: &Mailbox, answers: &UnboundedSender<Message>) -> JoinHandle<()> {
    let (mailbox, answers) = (mailbox.clone(), answers.clone());
    tokio::spawn(async move {
        let answer = guarded(answer(effect, &mailbox)).await;
        let _ = answers.send(answer);
    })
}

/// The message `work` answers with, or, should it panic, one that says
/// what went wrong: a bug then stops one task, not the interface.
async fn guarded(work: impl Future<Output = Message>) -> Message {
    AssertUnwindSafe(work)
        .catch_unwind()
        .await
        .unwrap_or_else(|panic| {
            let said = panic
                .downcast_ref::<&str>()
                .map(|text| (*text).to_owned())
                .or_else(|| panic.downcast_ref::<String>().cloned());
            Message::Failed(said.unwrap_or_else(|| "a task stopped".to_owned()))
        })
}

/// What `effect` comes to once done, as the message that answers it.
async fn answer(effect: Effect, mailbox: &Mailbox) -> Message {
    match effect {
        Effect::Places => {
            Message::PlacesLoaded(mailbox.places().await.map_err(|error| error.to_string()))
        }
        Effect::Conversations {
            request,
            place,
            page,
        } => Message::ConversationsLoaded {
            request,
            page,
            result: mailbox
                .conversations(&place, page)
                .await
                .map_err(|error| error.to_string()),
        },
        Effect::MarkRead { id, place } => {
            let result = mailbox.change(&id, &place, &Change::Read(true)).await;
            Message::MarkedRead {
                id,
                result: result.map_err(|error| error.to_string()),
            }
        }
        Effect::Change { id, place, change } => {
            let result = mailbox.change(&id, &place, &change).await;
            Message::Changed {
                change,
                result: result.map_err(|error| error.to_string()),
            }
        }
        Effect::OpenLink { url } => {
            Message::LinkOpened(open_link(&url).map_err(|error| error.to_string()))
        }
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_new_effect_aborts_the_task_still_running_one_of_its_kind() {
        let (sender, mut answers) = mpsc::unbounded_channel();
        let mut tasks = Tasks::default();
        let mailbox = Mailbox::demo();
        // The test's runtime runs none of them until it waits.
        for request in [1, 2] {
            let id = "no-such-conversation".to_owned();
            tasks.run(Effect::Conversation { request, id }, &mailbox, &sender);
        }
        tasks.run(Effect::Places, &mailbox, &sender);
        drop(sender);
        let mut seen = Vec::new();
        while let Some(answer) = answers.recv().await {
            seen.push(match answer {
                Message::ConversationLoaded { request, .. } => format!("conversation {request}"),
                Message::PlacesLoaded(_) => "places".to_owned(),
                other => panic!("{other:?}"),
            });
        }
        seen.sort();
        assert_eq!(seen, ["conversation 2", "places"]);
    }

    async fn panics(text: &str) -> Message {
        panic!("{text}")
    }

    #[tokio::test]
    async fn a_task_that_panics_answers_with_what_went_wrong() {
        let answer = guarded(panics("the renderer broke")).await;
        assert!(
            matches!(&answer, Message::Failed(said) if said == "the renderer broke"),
            "{answer:?}"
        );
        let answer = guarded(async { Message::Quit }).await;
        assert!(matches!(answer, Message::Quit), "{answer:?}");
    }

    #[tokio::test]
    async fn quitting_waits_a_while_for_the_changes_on_their_way() {
        let (sender, mut answers) = mpsc::unbounded_channel();
        let mut tasks = Tasks::default();
        let mailbox = Mailbox::demo();
        let change = Effect::Change {
            id: "conv-demo-01".into(),
            place: "0".into(),
            change: Change::Star(false),
        };
        tasks.run(change, &mailbox, &sender);
        // The test's runtime has not run it yet.
        assert!(tasks.busy());
        assert!(answers.try_recv().is_err());
        tasks.finish(&mailbox, Duration::from_secs(5)).await;
        assert!(matches!(
            answers.try_recv(),
            Ok(Message::Changed { result: Ok(()), .. })
        ));

        // One that never ends holds quitting up only so long.
        let mut stuck = Tasks::default();
        stuck
            .writes
            .push(tokio::spawn(std::future::pending::<()>()));
        let started = std::time::Instant::now();
        stuck.finish(&mailbox, Duration::from_millis(100)).await;
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test]
    async fn changes_run_to_their_end_however_often_they_are_asked() {
        let (sender, mut answers) = mpsc::unbounded_channel();
        let mut tasks = Tasks::default();
        let mailbox = Mailbox::demo();
        for id in ["a", "b"] {
            let effect = Effect::MarkRead {
                id: id.into(),
                place: "0".into(),
            };
            tasks.run(effect, &mailbox, &sender);
        }
        for star in [true, false] {
            let effect = Effect::Change {
                id: "c".into(),
                place: "0".into(),
                change: Change::Star(star),
            };
            tasks.run(effect, &mailbox, &sender);
        }
        drop(sender);
        let mut done = Vec::new();
        while let Some(answer) = answers.recv().await {
            match answer {
                Message::MarkedRead { id, result } => {
                    assert!(result.is_ok());
                    done.push(id);
                }
                Message::Changed { change, result } => {
                    assert!(result.is_ok());
                    done.push(format!("{change:?}"));
                }
                other => panic!("{other:?}"),
            }
        }
        done.sort();
        assert_eq!(done, ["Star(false)", "Star(true)", "a", "b"]);
    }

    #[cfg(unix)]
    #[test]
    fn ctrl_z_suspends_the_interface_and_asks_nothing_of_it() {
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
        let ctrl_z = Event::Key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL));
        assert!(suspends(&ctrl_z));
        assert!(Message::from_event(&ctrl_z).is_none());
        let z = Event::Key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE));
        assert!(!suspends(&z));
    }

    #[test]
    fn signing_in_first_takes_what_login_takes_without_options() {
        let options = login_options().unwrap();
        assert_eq!(options.api_url, None);
        assert!(!options.captcha_chrome);
    }
}
