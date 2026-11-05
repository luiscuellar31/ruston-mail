//! Mailbox tests, a module for each part of the mailbox. The fixtures here
//! are those several modules share.

mod actions;
mod folders;
mod listing;
mod ordering;
mod reader;
mod reads;
mod search;
mod splits;

use super::*;
use crate::mail::{MailAddress, MailMessage, MessageBody};
use crate::settings::Reading;
use crate::test_support::{NEVER_ISSUED, OPENING_COUNTS, OPENING_PAGE, sys};

const PAGE_SIZE: u32 = 50;
fn summary(id: &str) -> ConversationSummary {
    ConversationSummary {
        id: id.to_owned(),
        kind: SummaryKind::Conversation,
        subject: None,
        correspondents: None,
        participants: Vec::new(),
        preview: None,
        time: None,
        unread: false,
        starred: false,
        message_count: 1,
        has_attachments: false,
    }
}

fn searchable_summary(
    id: &str,
    name: &str,
    address: &str,
    subject: &str,
    preview: &str,
) -> ConversationSummary {
    ConversationSummary {
        id: id.to_owned(),
        kind: SummaryKind::Conversation,
        subject: Some(subject.to_owned()),
        correspondents: Some(name.to_owned()),
        participants: vec![MailAddress {
            name: Some(name.to_owned()),
            address: address.to_owned(),
        }],
        preview: Some(preview.to_owned()),
        time: None,
        unread: false,
        starred: false,
        message_count: 1,
        has_attachments: false,
    }
}

/// A page of `conversations` out of `total`, with nothing to inspect.
fn page_of(
    conversations: Vec<ConversationSummary>,
    total: u32,
) -> Result<ConversationPage, MailboxError> {
    Ok(ConversationPage {
        conversations,
        total,
        inspect_candidates: Vec::new(),
    })
}

fn page(ids: &[&str], total: u32) -> Result<ConversationPage, MailboxError> {
    page_of(ids.iter().map(|id| summary(id)).collect(), total)
}

fn detail(id: &str, message_ids: &[&str]) -> ConversationDetail {
    ConversationDetail {
        id: id.to_owned(),
        subject: None,
        labels: Vec::new(),
        messages: message_ids
            .iter()
            .enumerate()
            .map(|(time, message_id)| MailMessage {
                id: (*message_id).to_owned(),
                verdict: crate::mail::Verdict::Unverified,
                sender: MailAddress::default(),
                to: Vec::new(),
                cc: Vec::new(),
                bcc: Vec::new(),
                reply_to: Vec::new(),
                reply_all_cc: Vec::new(),
                reply_redirected: false,
                time: Some(time as i64),
                unread: false,
                body: MessageBody::PlainText(String::new()),
                attachments: Vec::new(),
            })
            .collect(),
    }
}

fn ids(mailbox: &Mailbox) -> Vec<&str> {
    mailbox
        .conversations()
        .iter()
        .map(|c| c.id.as_str())
        .collect()
}

fn open() -> (Mailbox, PageRequest) {
    open_with_page_size(PAGE_SIZE)
}

/// The Inbox, opened to list `page_size` rows a page.
fn open_with_page_size(page_size: u32) -> (Mailbox, PageRequest) {
    Mailbox::open(Folder::INBOX, page_size, OPENING_PAGE, OPENING_COUNTS)
}

fn loaded_inbox(ids: &[&str], total: u32) -> Mailbox {
    let (mut mailbox, request) = open();
    mailbox.finish_page(request.id, page(ids, total));
    mailbox
}

fn load_detail(
    mailbox: &mut Mailbox,
    conversation_id: &str,
    conversation: ConversationDetail,
    request: RequestId,
) {
    let request = mailbox
        .start_conversation_load(conversation_id.into(), request)
        .unwrap();
    assert_eq!(
        mailbox.finish_conversation(&request, Ok(conversation)),
        None
    );
}

/// Starts an action on the selected row and expects it to go at once.
fn send(mailbox: &mut Mailbox, action: MailAction, request: RequestId) -> ActionRequest {
    mailbox
        .start_action(action, request)
        .and_then(|start| start.send)
        .expect("the action goes at once")
}

fn searchable_mailbox() -> Mailbox {
    let (mut mailbox, request) = open();
    let conversations = vec![
        searchable_summary(
            "sender",
            "Alice Stone",
            "alice@example.com",
            "Quarterly roadmap",
            "Budget approval is ready",
        ),
        searchable_summary(
            "subject",
            "Bob Chen",
            "bob@example.com",
            "Rust project plan",
            "Milestones for next month",
        ),
        searchable_summary(
            "preview",
            "Carol Diaz",
            "carol@example.com",
            "Release notes",
            "Launch checklist is complete",
        ),
    ];
    let total = conversations.len() as u32;
    mailbox.finish_page(request.id, page_of(conversations, total));
    mailbox
}

fn visible_ids(mailbox: &Mailbox) -> Vec<&str> {
    mailbox
        .visible_conversations()
        .map(|conversation| conversation.id.as_str())
        .collect()
}

fn dated(id: &str, time: i64) -> ConversationSummary {
    ConversationSummary {
        time: Some(time),
        ..summary(id)
    }
}

fn unread(id: &str) -> ConversationSummary {
    ConversationSummary {
        unread: true,
        ..summary(id)
    }
}

fn split_message(id: &str, time: i64) -> ConversationSummary {
    ConversationSummary {
        kind: SummaryKind::Message,
        ..dated(id, time)
    }
}

fn message_row(id: &str) -> ConversationSummary {
    ConversationSummary {
        kind: SummaryKind::Message,
        ..searchable_summary(id, "Bank", "bank@example.com", "Statement", "")
    }
}

fn dated_page(rows: &[(&str, i64)], total: u32) -> Result<ConversationPage, MailboxError> {
    page_of(
        rows.iter().map(|(id, time)| dated(id, *time)).collect(),
        total,
    )
}

/// The date `full_page` gives the row `p<number>`: a second older per row,
/// so a row's place in the folder is its age.
fn page_time(number: u32) -> i64 {
    i64::from(1_000_000 - number)
}

/// A full page of dated rows named `p<n>`, newest first.
fn full_page(start: u32, total: u32) -> Result<ConversationPage, MailboxError> {
    let conversations = (0..PAGE_SIZE)
        .map(|index| {
            let number = start + index;
            dated(&format!("p{number}"), page_time(number))
        })
        .collect();

    page_of(conversations, total)
}

/// An Inbox holding `listed`, showing the results of a server search that
/// returned `found`, with `opened` open in the reader.
fn searched(listed: &[&str], found: &[&str], opened: &str) -> Mailbox {
    let (mut mailbox, request) = open();
    let rows: Vec<ConversationSummary> = listed.iter().map(|id| unread(id)).collect();
    let total = rows.len() as u32;
    mailbox.finish_page(request.id, page_of(rows, total));

    mailbox.set_search_query("report".to_owned());
    // Fixtures issue IDs from 80, clear of the 3, 4 and on that tests issue.
    let search = mailbox.start_search(90).unwrap();
    let results: Vec<ConversationSummary> = found.iter().map(|id| unread(id)).collect();
    let total = results.len() as u32;
    assert_eq!(
        mailbox.finish_search(&search, page_of(results, total)),
        None
    );
    load_detail(&mut mailbox, opened, detail(opened, &["message"]), 91);

    mailbox
}
