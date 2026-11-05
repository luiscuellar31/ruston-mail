//! End-to-end state-machine tests through `App::update`, a module for each
//! part of the app. The fixtures here are those several modules share.

mod actions;
mod attachments;
mod composer;
mod folders;
mod inspections;
mod keyboard;
mod links;
mod mailto;
mod notifications;
mod reader;
mod refresh;
mod search;
mod session;
mod settings;
mod sign_in;

use super::*;
use crate::mail::demo;
use crate::mail::{INSPECTION_CONCURRENCY, SendError};
use crate::settings::{ComposePlacement, Reading};
use secrecy::{ExposeSecret, SecretString};

use crate::mail::MailFolder;
use crate::test_support::{NEVER_ISSUED, OPENING_COUNTS, OPENING_PAGE, sys};

const NOW: i64 = 1_789_000_000;

/// What a refresh asks for: the open folder's page and its counts.
const PAGE_AND_COUNTS: usize = 2;

fn authenticated_app() -> App {
    let mut app = App::new(Settings::default());
    app.auth_state = AuthState::Authenticated { email: None };
    app.mailbox = Some(Mailbox::open(Folder::INBOX, 50, OPENING_PAGE, OPENING_COUNTS).0);
    app.last_request = OPENING_COUNTS;
    app
}

/// Delivers the demo page a request would produce, as the async task would.
fn deliver_demo_page(app: &mut App, request: RequestId, page: u32) {
    let folder = app.mailbox().unwrap().folder();
    let result = demo_service(app).list_conversations(folder, page, demo::PAGE_SIZE, NOW);
    let _ = app.update(Message::ConversationsLoaded(request, result));
}

fn deliver_latest_demo_page(app: &mut App, page: u32) {
    let request = pending_list_request(app);
    deliver_demo_page(app, request, page);
}

fn demo_service(app: &App) -> demo::DemoMailbox {
    match app.backend.as_ref() {
        Some(MailBackend::Demo(service)) => service.clone(),
        _ => panic!("expected demo backend"),
    }
}

/// The list request the open folder or search waits for, as its status
/// says, so a test need not know the order requests were numbered in.
fn pending_list_request(app: &App) -> RequestId {
    app.mailbox()
        .and_then(|mailbox| mailbox.status().request_in_flight())
        .expect("a list request in flight")
}

fn pending_reader_request(app: &App) -> ReaderRequest {
    match app.mailbox().unwrap().reader_state() {
        ReaderState::Loading {
            conversation_id,
            request,
        } => ReaderRequest {
            id: *request,
            conversation_id: conversation_id.clone(),
            kind: crate::mail::SummaryKind::Conversation,
            messages: 1,
        },
        _ => panic!("expected pending reader request"),
    }
}

fn deliver_selected_demo_detail(app: &mut App) {
    let request = pending_reader_request(app);
    let result = demo_service(app).conversation_detail(&request.conversation_id, NOW);
    let _ = app.update(Message::ConversationLoaded(request, result));
}

fn deliver_folders(app: &mut App, result: Result<Vec<Folder>, MailboxError>) {
    let epoch = app.session_epoch;
    let places = result.map(|folders| crate::mail::OwnPlaces {
        folders,
        ..Default::default()
    });
    let _ = app.update(Message::FoldersLoaded(epoch, places));
}

fn deliver_send(app: &mut App, result: Result<(), SendError>) {
    let epoch = app.session_epoch;
    let _ = app.update(Message::Sent(epoch, result));
}

/// Sends a plain key press, as the window would when no field took it.
fn press(app: &mut App, key: Key) {
    let _ = app.update(Message::KeyPressed(KeyPress::plain(key)));
}

/// Sends a command key press (Cmd on macOS, Ctrl on other platforms).
fn press_command(app: &mut App, key: Key) {
    let _ = app.update(Message::KeyPressed(KeyPress::with_command(key)));
}

fn loaded_demo_app() -> App {
    let (mut app, _) = App::boot(true, Settings::default());
    deliver_demo_page(&mut app, OPENING_PAGE, 0);
    let counts = demo_service(&app).counts();
    let _ = app.update(Message::CountsLoaded(OPENING_COUNTS, Ok(counts)));
    app
}

/// Whether the open folder's list has a row for `id`.
fn is_listed(app: &App, id: &str) -> bool {
    app.mailbox()
        .unwrap()
        .conversations()
        .iter()
        .any(|row| row.id == id)
}

fn open_conversation(app: &mut App, id: &str) {
    let _ = app.update(Message::SelectConversation(id.to_owned()));
    deliver_selected_demo_detail(app);
}

/// Opens a message and fills it in.
fn write(app: &mut App, to: &str) {
    let _ = app.update(Message::OpenCompose);
    let _ = app.update(Message::ComposeChanged(ComposeField::To, to.to_owned()));
    let _ = app.update(Message::ComposeChanged(
        ComposeField::Subject,
        "Thursday".to_owned(),
    ));
    let _ = app.update(Message::ComposeChanged(
        ComposeField::Body,
        "See you then.".to_owned(),
    ));
}
