use super::*;
use crate::test_support::sys;

#[test]
fn demo_protection_depends_only_on_the_address() {
    use crate::mail::Protection;

    for address in ["ada@proton.me", "ada@PM.ME", "ada@protonmail.com"] {
        assert_eq!(protection(address), Protection::EndToEnd, "{address}");
    }
    for address in ["plain@example.com", "ada@proton.me.example.com"] {
        assert_eq!(protection(address), Protection::Unencrypted, "{address}");
    }
}

pub(super) const NOW: i64 = 1_789_000_000;

#[test]
fn search_reaches_every_folder() {
    let mailbox = DemoMailbox::new();
    let inbox = mailbox
        .list_conversations(&sys(MailFolder::Inbox), 0, PAGE_SIZE, NOW)
        .unwrap();
    // Taken from the data rather than written here, so the test survives
    // any rewrite of the fictional mail.
    let row = inbox
        .conversations
        .iter()
        .find(|row| row.subject.is_some())
        .expect("the demo inbox has mail with subjects");
    let id = row.id.clone();
    let word = row
        .subject
        .as_deref()
        .unwrap()
        .split_whitespace()
        .find(|word| word.len() >= 4)
        .expect("a subject has a word to search for")
        .to_owned();

    let found = mailbox.search(&word, PAGE_SIZE, NOW).unwrap();
    assert!(found.conversations.iter().any(|found| found.id == id));

    // The row keeps turning up once it has left the Inbox: a search that
    // stopped at the open folder would not be a search.
    assert!(mailbox.move_to(&id, &sys(MailFolder::Archive)));
    let found = mailbox.search(&word, PAGE_SIZE, NOW).unwrap();
    assert!(found.conversations.iter().any(|found| found.id == id));

    // An empty query matches nothing, and the limit is honoured.
    assert!(
        mailbox
            .search("   ", PAGE_SIZE, NOW)
            .unwrap()
            .conversations
            .is_empty()
    );
    assert!(mailbox.search(&word, 1, NOW).unwrap().conversations.len() <= 1);
}

fn page(folder: MailFolder, page: u32) -> ConversationPage {
    DemoMailbox::new()
        .list_conversations(&sys(folder), page, PAGE_SIZE, NOW)
        .unwrap()
}

fn detail(index: usize) -> ConversationDetail {
    DemoMailbox::new()
        .conversation_detail(&format!("demo-{index}"), NOW)
        .unwrap()
}

#[test]
fn counts_are_deterministic() {
    let counts = DemoMailbox::new().counts();

    assert_eq!(counts, DemoMailbox::new().counts());
    assert_eq!(counts.unread(&sys(MailFolder::Inbox)), Some(6));
    assert_eq!(counts.unread(&sys(MailFolder::Starred)), Some(1));
    assert_eq!(counts.unread(&sys(MailFolder::Archive)), Some(1));
    assert_eq!(counts.unread(&sys(MailFolder::Sent)), Some(0));
    assert_eq!(counts.unread(&sys(MailFolder::Trash)), Some(0));
}

#[test]
fn first_inbox_page_is_deterministic_and_newest_first() {
    let first = page(MailFolder::Inbox, 0);

    assert_eq!(first.total, 35);
    assert_eq!(first.conversations.len(), 10);
    assert_eq!(first.conversations[0].id, "demo-0");
    assert_eq!(first.conversations[0].time, Some(NOW - 12 * MINUTE));
    assert!(
        first
            .conversations
            .windows(2)
            .all(|pair| pair[0].time >= pair[1].time)
    );
    assert_eq!(
        first.conversations,
        page(MailFolder::Inbox, 0).conversations
    );
}

#[test]
fn pages_continue_and_end() {
    let second = page(MailFolder::Inbox, 1);
    let last = page(MailFolder::Inbox, 3);

    assert_eq!(second.conversations[0].id, "demo-9");
    assert_eq!(last.conversations.len(), 5);
    assert!(page(MailFolder::Inbox, 4).conversations.is_empty());
}

#[test]
fn trash_starts_empty_and_spam_is_available() {
    let trash = page(MailFolder::Trash, 0);
    let spam = page(MailFolder::Spam, 0);

    assert_eq!(trash.total, 0);
    assert!(trash.conversations.is_empty());
    assert_eq!(spam.total, 2);
    assert_eq!(spam.conversations.len(), 2);
}

#[test]
fn starred_collects_starred_conversations_from_other_folders() {
    let starred = page(MailFolder::Starred, 0);

    assert_eq!(starred.total, 5);
    assert!(starred.conversations.iter().all(|c| c.starred));
}

#[test]
fn fixtures_cover_edge_cases() {
    let inbox: Vec<_> = (0..4)
        .flat_map(|index| page(MailFolder::Inbox, index).conversations)
        .collect();

    assert!(inbox.iter().any(|c| c.subject.is_none()));
    assert!(inbox.iter().any(|c| c.correspondents.is_none()));
    assert!(inbox.iter().any(|c| c.message_count > 1));
    assert!(inbox.iter().any(|c| c.unread) && inbox.iter().any(|c| !c.unread));
}

#[test]
fn every_conversation_uses_its_fixture_bodies() {
    for (index, fixture) in fixtures().iter().enumerate() {
        let detail = detail(index);

        assert_eq!(detail.id, format!("demo-{index}"));
        assert_eq!(detail.messages.len(), fixture.message_count());
        for (message, expected) in detail.messages.iter().zip(&fixture.bodies) {
            assert!(!expected.trim().is_empty());
            if fixture.html {
                // An HTML fixture reaches the reader as structure. No
                // markup survives into what the reader shows or copies.
                assert!(matches!(message.body, MessageBody::Rich(_)));
                assert!(!message.body.plain_text().contains('<'));
            } else {
                assert_eq!(message.body, MessageBody::PlainText((*expected).to_owned()));
            }
        }
        assert!(
            detail
                .messages
                .windows(2)
                .all(|pair| pair[0].time < pair[1].time)
        );
        assert_eq!(
            detail,
            DemoMailbox::new()
                .conversation_detail(&detail.id, NOW)
                .unwrap()
        );
    }
    let mailbox = DemoMailbox::new();
    assert_eq!(
        mailbox.conversation_detail("demo-999", NOW),
        Err(MailboxError::Unavailable)
    );
    assert_eq!(
        mailbox.conversation_detail("other", NOW),
        Err(MailboxError::Unavailable)
    );
}

#[test]
fn newest_message_matches_the_list_time() {
    let detail = detail(0);

    assert_eq!(
        detail.messages.last().unwrap().time,
        Some(NOW - 12 * MINUTE)
    );
}

fn outgoing(subject: &str) -> Outgoing {
    Outgoing {
        kind: OutgoingKind::New,
        to: vec!["alex@example.com".into()],
        cc: Vec::new(),
        bcc: Vec::new(),
        subject: subject.to_owned(),
        body: "Hi Alex,\n\nSee you Thursday.".into(),
        format: crate::mail::BodyFormat::PlainText,
        attachments: Vec::new(),
    }
}

#[test]
fn a_sent_message_lands_in_the_fictional_sent_folder() {
    let mailbox = DemoMailbox::new();
    let before = state_page(&mailbox, MailFolder::Sent).total;

    mailbox.send_now(&outgoing("About Thursday")).unwrap();

    let sent = state_page(&mailbox, MailFolder::Sent);
    assert_eq!(sent.total, before + 1);
    let row = sent
        .conversations
        .iter()
        .find(|row| row.subject.as_deref() == Some("About Thursday"))
        .expect("the message is in Sent");
    assert_eq!(row.correspondents.as_deref(), Some("alex@example.com"));
}

#[test]
fn the_fictional_mailbox_stops_accepting_after_a_few() {
    // A cap is what makes it safe to leave sending switched on without an
    // account: a loop in the sending path cannot grow the mailbox.
    let mailbox = DemoMailbox::new();
    for index in 0..SEND_LIMIT {
        mailbox
            .send_now(&outgoing(&format!("Message {index}")))
            .expect("within the limit");
    }

    assert_eq!(
        mailbox.send_now(&outgoing("One too many")),
        Err(SendError::DemoLimitReached)
    );
    // And it says so in a way someone can act on.
    assert!(SendError::DemoLimitReached.message().contains("Restart"));
}

#[test]
fn a_reply_goes_back_to_the_sender_with_the_thread_under_it() {
    // The fictional mailbox mirrors what Proton does with an answer, so
    // the demo shows the same thing the real one would.
    let mailbox = DemoMailbox::new();
    let answered = mailbox.conversation_detail("demo-0", NOW).unwrap();
    let parent = &answered.messages[0];

    mailbox
        .send_now(&Outgoing {
            kind: OutgoingKind::Reply {
                message_id: parent.id.clone(),
                everyone: false,
            },
            to: Vec::new(),
            subject: String::new(),
            ..outgoing("ignored")
        })
        .unwrap();

    let row = state_page(&mailbox, MailFolder::Sent)
        .conversations
        .into_iter()
        .max_by_key(|row| row.time)
        .expect("the reply is in Sent");
    // Core writes the subject and addresses it; neither was typed here.
    assert_eq!(
        row.subject.as_deref(),
        Some(format!("Re: {}", answered.subject.unwrap()).as_str())
    );
    assert_eq!(
        row.correspondents.as_deref(),
        Some(parent.sender.address.as_str())
    );

    let sent = mailbox.conversation_detail(&row.id, NOW).unwrap();
    let body = sent.messages[0].body.plain_text();
    assert!(
        body.contains("See you Thursday."),
        "the new text is missing"
    );
    assert!(body.contains("> "), "the message answered is not quoted");
}

#[test]
fn a_reply_to_the_demo_user_s_own_message_goes_to_its_recipients() {
    let mailbox = DemoMailbox::new();
    let answered = mailbox.conversation_detail("demo-37", NOW).unwrap();
    assert_eq!(answered.subject.as_deref(), Some("Re: Offsite agenda"));
    let own = &answered.messages[0];
    assert_eq!(own.sender.address, DEMO_USER_ADDRESS);
    let recipients: Vec<String> = own
        .to
        .iter()
        .map(|recipient| recipient.address.clone())
        .collect();
    assert_eq!(own.reply_to, recipients);
    assert!(own.reply_all_cc.is_empty());

    mailbox
        .send_now(&Outgoing {
            kind: OutgoingKind::Reply {
                message_id: own.id.clone(),
                everyone: false,
            },
            to: own.reply_to.clone(),
            ..outgoing("ignored")
        })
        .unwrap();

    let row = state_page(&mailbox, MailFolder::Sent)
        .conversations
        .into_iter()
        .max_by_key(|row| row.time)
        .expect("the reply is in Sent");
    assert_eq!(row.correspondents, Some(recipients.join(", ")));
}

#[test]
fn a_forward_keeps_the_recipients_it_was_given() {
    let mailbox = DemoMailbox::new();
    let answered = mailbox.conversation_detail("demo-0", NOW).unwrap();

    mailbox
        .send_now(&Outgoing {
            kind: OutgoingKind::Forward {
                message_id: answered.messages[0].id.clone(),
            },
            subject: String::new(),
            ..outgoing("ignored")
        })
        .unwrap();

    let row = state_page(&mailbox, MailFolder::Sent)
        .conversations
        .into_iter()
        .max_by_key(|row| row.time)
        .expect("the forward is in Sent");
    assert!(row.subject.as_deref().unwrap().starts_with("Fw: "));
    // Unlike a reply, a forward goes where it was told.
    assert_eq!(row.correspondents.as_deref(), Some("alex@example.com"));
}

#[test]
fn choosing_html_is_what_the_demo_shows_back() {
    let mailbox = DemoMailbox::new();
    mailbox
        .send_now(&Outgoing {
            format: crate::mail::BodyFormat::Html,
            ..outgoing("Formatted")
        })
        .unwrap();

    let id = state_page(&mailbox, MailFolder::Sent)
        .conversations
        .into_iter()
        .find(|row| row.subject.as_deref() == Some("Formatted"))
        .expect("the message is in Sent")
        .id;
    let detail = mailbox.conversation_detail(&id, NOW).unwrap();

    // It comes back as structure, not as the tags that carried it.
    assert!(matches!(
        detail.messages[0].body,
        crate::mail::MessageBody::Rich(_)
    ));
    assert!(!detail.messages[0].body.plain_text().contains('<'));
}

#[test]
fn the_html_conversation_shows_its_text_and_never_its_markup() {
    let fixtures = fixtures();
    let index = fixtures
        .iter()
        .position(|fixture| fixture.html)
        .expect("the fictional mailbox carries one HTML conversation");
    let fixture = &fixtures[index];

    let text = body_text(fixture, &fixture.bodies[0]);
    assert!(text.contains("Reading room booking"));
    assert!(
        !text.contains('<'),
        "markup reached the reader's text: {text}"
    );

    // Searching reads what the mail says, not the tags it is written in.
    assert!(matches_query(fixture, "membership card"));
    assert!(!matches_query(fixture, "blockquote"));

    // Nor do the tags reach the conversation list.
    let preview = state_page(&DemoMailbox::new(), MailFolder::Inbox)
        .conversations
        .into_iter()
        .find(|conversation| conversation.id == format!("demo-{index}"))
        .expect("the HTML conversation is on the first Inbox page")
        .preview
        .expect("it has a preview");
    assert!(!preview.contains('<'), "markup reached the list: {preview}");
}

#[test]
fn summaries_keep_only_a_bounded_preview() {
    let summary = state_page(&DemoMailbox::new(), MailFolder::Inbox)
        .conversations
        .into_iter()
        .find(|conversation| conversation.id == "demo-5")
        .unwrap();
    let preview = summary.preview.unwrap();
    let MessageBody::PlainText(body) = &detail(5).messages[0].body else {
        panic!("demo bodies are plain text");
    };

    assert!(preview.chars().count() <= 200);
    assert!(body.len() > preview.len());
}

#[test]
fn details_cover_reader_edge_cases() {
    let noreply = &detail(1).messages[0];
    assert_eq!(noreply.sender.name, None);
    assert_eq!(noreply.sender.display_name(), Some("noreply@example.org"));

    let unknown = &detail(22).messages[0];
    assert_eq!(unknown.sender.display_name(), None);

    let garden = &detail(6).messages[0];
    assert_eq!(garden.to.len(), GARDEN_VOLUNTEERS.len());

    let long = &detail(5).messages[0];
    assert_eq!(long.body, MessageBody::PlainText(LONG_BODY.to_owned()));

    assert_eq!(detail(3).subject, None);
}

#[test]
fn threads_alternate_with_the_demo_user() {
    let inbox = detail(0);
    let sent = detail(36);

    assert_eq!(
        inbox.messages[2].sender.name.as_deref(),
        Some("Alex Rivera")
    );
    assert_eq!(inbox.messages[1].sender.address, DEMO_USER_ADDRESS);
    assert_eq!(sent.messages[0].sender.address, DEMO_USER_ADDRESS);
}

#[test]
fn fixture_addresses_are_fictional() {
    for (index, _) in fixtures().iter().enumerate() {
        for message in detail(index).messages {
            for address in std::iter::once(&message.sender)
                .chain(&message.to)
                .chain(&message.cc)
                .chain(&message.bcc)
            {
                if let Some((_, domain)) = address.address.rsplit_once('@') {
                    assert!(
                        matches!(domain, "example.com" | "example.org" | "example.net"),
                        "non-example domain in demo data: {domain}"
                    );
                }
            }
        }
    }
}

#[test]
fn read_actions_update_state_and_counts_idempotently() {
    let mailbox = DemoMailbox::new();

    assert_eq!(mailbox.counts().unread(&sys(MailFolder::Inbox)), Some(6));
    assert!(mailbox.set_unread("demo-0", false));
    assert!(!state_page(&mailbox, MailFolder::Inbox).conversations[0].unread);
    assert_eq!(mailbox.counts().unread(&sys(MailFolder::Inbox)), Some(5));

    assert!(mailbox.set_unread("demo-0", false));
    assert_eq!(mailbox.counts().unread(&sys(MailFolder::Inbox)), Some(5));
    assert!(mailbox.set_unread("demo-0", true));
    assert!(mailbox.set_unread("demo-0", true));
    assert_eq!(mailbox.counts().unread(&sys(MailFolder::Inbox)), Some(6));
}

#[test]
fn star_actions_update_visibility_and_counts_idempotently() {
    let mailbox = DemoMailbox::new();

    assert!(mailbox.set_starred("demo-1", true));
    assert!(mailbox.set_starred("demo-1", true));
    let starred = state_page(&mailbox, MailFolder::Starred);
    assert_eq!(starred.total, 6);
    assert!(starred.conversations.iter().any(|c| c.id == "demo-1"));
    assert_eq!(mailbox.counts().unread(&sys(MailFolder::Starred)), Some(2));

    assert!(mailbox.set_starred("demo-1", false));
    assert!(mailbox.set_starred("demo-1", false));
    assert_eq!(state_page(&mailbox, MailFolder::Starred).total, 5);
    assert_eq!(mailbox.counts().unread(&sys(MailFolder::Starred)), Some(1));
}

#[test]
fn archive_moves_conversation_and_preserves_content() {
    let mailbox = DemoMailbox::new();
    let detail = mailbox.conversation_detail("demo-0", NOW).unwrap();

    assert!(mailbox.move_to("demo-0", &sys(MailFolder::Archive)));

    assert!(!contains(&mailbox, MailFolder::Inbox, "demo-0"));
    assert!(contains(&mailbox, MailFolder::Archive, "demo-0"));
    assert_eq!(mailbox.conversation_detail("demo-0", NOW), Ok(detail));
}

#[test]
fn trash_moves_conversation_without_deleting_it() {
    let mailbox = DemoMailbox::new();

    assert!(mailbox.move_to("demo-3", &sys(MailFolder::Trash)));

    assert!(!contains(&mailbox, MailFolder::Inbox, "demo-3"));
    assert!(contains(&mailbox, MailFolder::Trash, "demo-3"));
    assert!(mailbox.conversation_detail("demo-3", NOW).is_ok());
}

#[test]
fn spam_moves_conversation_without_network_behavior() {
    let mailbox = DemoMailbox::new();

    assert!(mailbox.move_to("demo-4", &sys(MailFolder::Spam)));

    assert!(!contains(&mailbox, MailFolder::Inbox, "demo-4"));
    assert!(contains(&mailbox, MailFolder::Spam, "demo-4"));
}

#[test]
fn derived_counts_match_every_folder_after_repeated_actions() {
    let mailbox = DemoMailbox::new();
    assert!(mailbox.set_unread("demo-0", false));
    assert!(mailbox.set_unread("demo-0", false));
    assert!(mailbox.set_starred("demo-1", true));
    assert!(mailbox.set_starred("demo-1", true));
    assert!(mailbox.move_to("demo-2", &sys(MailFolder::Archive)));
    assert!(mailbox.move_to("demo-3", &sys(MailFolder::Trash)));
    assert!(mailbox.move_to("demo-4", &sys(MailFolder::Spam)));

    let counts = mailbox.counts();
    for folder in MailFolder::ALL {
        let unread = state_page(&mailbox, folder)
            .conversations
            .iter()
            .filter(|conversation| conversation.unread)
            .count() as u32;
        assert_eq!(counts.unread(&sys(folder)), Some(unread), "{folder:?}");
    }
}

fn state_page(mailbox: &DemoMailbox, folder: MailFolder) -> ConversationPage {
    mailbox
        .list_conversations(&sys(folder), 0, 1_000, NOW)
        .unwrap()
}

fn contains(mailbox: &DemoMailbox, folder: MailFolder, id: &str) -> bool {
    state_page(mailbox, folder)
        .conversations
        .iter()
        .any(|conversation| conversation.id == id)
}
