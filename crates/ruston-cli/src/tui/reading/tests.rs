use super::*;
use ruston_core::{Error, FullMessage, Recipient, UnreadableMessage, Verdict};

fn opened(sender: &str, body: &str) -> ConversationMessage {
    ConversationMessage::Opened(FullMessage {
        meta: meta(sender),
        body: body.into(),
        mime_type: "text/plain".into(),
        verdict: Verdict::Verified,
        attachments: Vec::new(),
        reply_tos: Vec::new(),
    })
}

fn meta(sender: &str) -> MessageMetadata {
    MessageMetadata {
        sender: Recipient::new(sender),
        to_list: vec![Recipient::new("me@example.test")],
        time: 1_714_000_000,
        ..MessageMetadata::default()
    }
}

#[test]
fn each_message_shows_its_headers_verdict_and_body_safely() {
    let conversation = Conversation {
        subject: "Plans\x1b[2J\nfor May".into(),
        ..Conversation::default()
    };
    let messages = [
        ConversationMessage::Opened(FullMessage {
            meta: meta("alex@example.test"),
            body: "Hi,\r\n\tsee you\x1b]52;c;payload\x07 there".into(),
            mime_type: "text/plain".into(),
            verdict: Verdict::Verified,
            attachments: Vec::new(),
            reply_tos: Vec::new(),
        }),
        ConversationMessage::Unreadable(UnreadableMessage {
            meta: meta("sam@example.test"),
            reply_tos: Vec::new(),
            error: Error::Other("no key opens it".into()),
        }),
    ];

    let reading = Reading::new(&conversation, &messages, &[]);
    assert_eq!(reading.subject, "Plans[2J for May");
    let text = reading.lines.join("\n");
    assert!(
        text.chars().all(|c| !c.is_control() || c == '\n'),
        "{text:?}"
    );
    for expected in [
        "From:    alex@example.test",
        "To:      me@example.test",
        "Date:    2024-04-24 23:06:40 UTC",
        "Verdict: verified",
        "Hi,\n    see you",
        SEPARATOR,
        "From:    sam@example.test",
        "Verdict: not decrypted",
        "(This message could not be decrypted: no key opens it)",
    ] {
        assert!(text.contains(expected), "{expected:?} missing:\n{text}");
    }
    assert!(reading.quotes.is_empty());
}

#[test]
fn an_answer_answers_the_last_message_that_is_not_a_draft() {
    let own = [AddressInfo {
        id: "addr-2".into(),
        email: "alias@example.test".into(),
    }];
    let mut first = meta("alex@example.test");
    first.id = "m1".into();
    first.subject = "Plans\x1b[2J".into();
    first.address_id = "addr-2".into();
    first.cc_list = vec![
        Recipient::new("sam@example.test"),
        Recipient::new("alias@example.test"),
    ];
    let answered = ConversationMessage::Opened(FullMessage {
        meta: first,
        body: "Hi".into(),
        mime_type: "text/plain".into(),
        verdict: Verdict::Verified,
        attachments: Vec::new(),
        reply_tos: vec![Recipient::new("list@example.test")],
    });
    let mut draft = meta("alias@example.test");
    draft.id = "m2".into();
    draft.label_ids = vec![label_ids::DRAFTS.into()];
    let draft = ConversationMessage::Unreadable(UnreadableMessage {
        meta: draft,
        reply_tos: Vec::new(),
        error: Error::Other("a draft".into()),
    });
    let reading = Reading::new(&Conversation::default(), &[answered, draft], &own);
    let answer = reading.answer.unwrap();
    assert_eq!(answer.message_id, "m1");
    // From the address it reached, to whom it asks replies to go, and
    // for Reply all, everyone else but the account.
    assert_eq!(answer.from.as_deref(), Some("alias@example.test"));
    assert_eq!(answer.to, ["list@example.test"]);
    assert_eq!(answer.cc, ["me@example.test", "sam@example.test"]);
    assert_eq!(answer.redirected_from.as_deref(), Some("alex@example.test"));
    assert_eq!(answer.subject, "Plans[2J");
    assert!(answer.opened);
    // A conversation of drafts alone answers nothing.
    let mut only = meta("alias@example.test");
    only.label_ids = vec![label_ids::ALL_DRAFTS.into()];
    let only = ConversationMessage::Unreadable(UnreadableMessage {
        meta: only,
        reply_tos: Vec::new(),
        error: Error::Other("a draft".into()),
    });
    assert_eq!(
        Reading::new(&Conversation::default(), &[only], &own).answer,
        None
    );
}

#[test]
fn quoted_passages_of_three_lines_or_more_fold_unless_all_is_quoted() {
    let messages = [
        opened(
            "alex@example.test",
            "Thanks!\n\nOn Monday, Sam wrote:\n> Shall we meet?\n>\n\n> Tuesday works.\n\n\
                 Below:\n> one\n> two\n\nAnd:\n> x\n> y\n> z\n\nBye",
        ),
        opened("sam@example.test", "> all\n> of it\n\n> quoted"),
    ];
    let reading = Reading::new(&Conversation::default(), &messages, &[]);
    assert!(reading.links.is_empty());
    let at = |text: &str| reading.lines.iter().position(|line| line == text).unwrap();
    let (first, second) = (at("> Shall we meet?"), at("> x"));
    // The blank line between two quoted ones belongs to the passage; the
    // two lines quoted below it are too few to fold.
    assert_eq!(reading.quotes, [first..first + 4, second..second + 3]);
    assert_eq!(reading.lines[first + 3], "> Tuesday works.");
    assert_eq!(reading.lines[first + 4], "");
}

/// A message from `sender` saying `body`, not read yet.
fn unread(sender: &str, body: &str) -> ConversationMessage {
    let mut message = opened(sender, body);
    if let ConversationMessage::Opened(full) = &mut message {
        full.meta.unread = 1;
    }
    message
}

#[test]
fn messages_read_but_the_newest_fold_and_the_first_unread_is_landed_on() {
    let unreadable = ConversationMessage::Unreadable(UnreadableMessage {
        meta: meta("Sam <sam@example.test>"),
        reply_tos: Vec::new(),
        error: Error::Other("no key opens it".into()),
    });
    let messages = [
        opened("alex@example.test", "One\n\ttwo\x1b[2J  three"),
        unreadable,
        unread("alex@example.test", "Not read yet"),
        opened("sam@example.test", "Read, but between"),
        opened("alex@example.test", "The newest, read"),
    ];
    let reading = Reading::new(&Conversation::default(), &messages, &[]);
    let at = |text: &str| reading.lines.iter().position(|line| line == text).unwrap();
    let [first, second, not_read, between, newest] = &reading.messages[..] else {
        panic!("{:?}", reading.messages);
    };
    assert_eq!(
        first.line,
        "▸ 2024-04-24 23:06 · alex@example.test · verified · One two[2J three"
    );
    assert_eq!(
        second.line,
        "▸ 2024-04-24 23:06 · Sam <sam@example.test> · not decrypted"
    );
    // Each takes its lines from its headers on, the separators between
    // them apart.
    assert_eq!(first.lines.start, 0);
    assert_eq!(
        reading.lines[first.lines.start],
        "From:    alex@example.test"
    );
    for pair in reading.messages.windows(2) {
        let between = &reading.lines[pair[0].lines.end..pair[1].lines.start];
        assert_eq!(between, ["", SEPARATOR, ""]);
    }
    assert_eq!(newest.lines.end, reading.lines.len());
    assert!(not_read.unread && !between.unread);
    assert!(not_read.lines.contains(&at("Not read yet")));
    assert!(between.lines.contains(&at("Read, but between")));
    // The newest and those not read yet stay open.
    let folds: Vec<bool> = (0..5).map(|index| reading.folds(index)).collect();
    assert_eq!(folds, [true, true, false, true, false]);
    assert_eq!(reading.landing(), Some(2));
}

#[test]
fn a_conversation_read_lands_on_its_newest_and_a_single_message_nowhere() {
    let conversation = Conversation::default();
    let read = [
        opened("alex@example.test", "A"),
        opened("sam@example.test", "B"),
    ];
    assert_eq!(Reading::new(&conversation, &read, &[]).landing(), Some(1));
    let single = [unread("alex@example.test", "Only")];
    let reading = Reading::new(&conversation, &single, &[]);
    assert_eq!(reading.landing(), None);
    assert!(!reading.folds(0));
}

#[test]
fn links_are_the_web_addresses_of_the_bodies_as_they_open() {
    let text = "See https://example.test/a, the [report](https://example.test/r%281%29) \
                    and (https://example.test/b). Also http://xn--bcher-kva.example/ and \
                    https://bücher.example/x, mailto:me@example.test, javascript:alert(1), \
                    httpd, **https://example.test/c**, \
                    [https://example.test/d](https://example.test/d) and \
                    https://en.wikipedia.org/wiki/Rust_(programming_language) too.";
    assert_eq!(
        web_links(text),
        [
            "https://example.test/a",
            "https://example.test/r%281%29",
            "https://example.test/b",
            "http://xn--bcher-kva.example/",
            "https://xn--bcher-kva.example/x",
            "https://example.test/c",
            "https://example.test/d",
            "https://example.test/d",
            "https://en.wikipedia.org/wiki/Rust_(programming_language)",
        ]
    );
}

#[test]
fn a_long_run_of_brackets_after_an_address_costs_its_length() {
    // Counting the brackets again at each one taken off took over a
    // second for 40,000 of them, and the square of that for more.
    let run = 400_000;
    let text = format!("https://example.test/a(b){} end", ")".repeat(run));
    assert_eq!(web_links(&text), ["https://example.test/a(b)"]);
    let mixed = format!("https://example.test/a{}", ").".repeat(run));
    assert_eq!(web_links(&mixed), ["https://example.test/a"]);
}

#[test]
fn an_html_link_lists_where_it_goes_as_well_as_what_it_says() {
    let html = "<p>Pay at <a href=\"https://real.example/pay\">https://bank.example</a></p>";
    let mut message = opened("alex@example.test", html);
    if let ConversationMessage::Opened(full) = &mut message {
        full.mime_type = "text/html".into();
    }
    let reading = Reading::new(&Conversation::default(), &[message], &[]);
    let text = reading.lines.join(
        "
",
    );
    assert!(
        text.contains("[https://bank.example](https://real.example/pay)"),
        "{text}"
    );
    assert_eq!(
        reading.links,
        ["https://bank.example/", "https://real.example/pay"]
    );
}

#[test]
fn the_demo_shows_a_quote_to_fold_and_a_link_named_in_words() {
    let (conversation, messages) = crate::demo::DemoSession::new()
        .conversation("conv-demo-03")
        .unwrap();
    let reading = Reading::new(&conversation, &messages, &[]);
    let text = reading.lines.join("\n");
    assert!(
        text.contains("[the shared agenda](https://example.com/offsite/agenda)"),
        "{text}"
    );
    assert_eq!(reading.links, ["https://example.com/offsite/agenda"]);
    assert_eq!(reading.quotes.len(), 1, "{text}");
    let quoted = &reading.lines[reading.quotes[0].clone()];
    assert_eq!(quoted.first().map(String::as_str), Some("> Hi Sam,"));
    assert!(quoted.len() >= 3, "{quoted:?}");
}

#[test]
fn attachments_are_listed_under_their_message_the_parts_of_the_body_last() {
    let mut message = opened("alex@example.test", "See attached.");
    if let ConversationMessage::Opened(full) = &mut message {
        full.meta.id = "m1".into();
        full.attachments = vec![
            ruston_core::Attachment {
                id: "a2".into(),
                name: "logo.png".into(),
                size: 900,
                disposition: Some("inline".into()),
                ..ruston_core::Attachment::default()
            },
            ruston_core::Attachment {
                id: "a1".into(),
                name: "report\u{1b}[2J.pdf".into(),
                size: 24_576,
                ..ruston_core::Attachment::default()
            },
        ];
    }
    let reading = Reading::new(&Conversation::default(), &[message], &[]);
    let file = |id: &str, name: &str, size: u64, inline: bool| ReadingAttachment {
        message_id: "m1".into(),
        id: id.into(),
        name: name.into(),
        size,
        inline,
    };
    // A picture placed within the text can be saved as a file attached.
    assert_eq!(
        reading.attachments,
        [
            file("a1", "report[2J.pdf", 24_576, false),
            file("a2", "logo.png", 900, true),
        ]
    );
    let text = reading.lines.join("\n");
    assert!(
        text.contains(
            "Verdict: verified\nAttached: report[2J.pdf (24 KB)\n\
                 In the message: logo.png (900 bytes)\n"
        ),
        "{text}"
    );
}

#[test]
fn sizes_read_as_the_desktop_shows_them() {
    assert_eq!(size_label(1), "1 byte");
    assert_eq!(size_label(500), "500 bytes");
    assert_eq!(size_label(1_024), "1.0 KB");
    assert_eq!(size_label(24_576), "24 KB");
    assert_eq!(size_label(5 * 1024 * 1024 + 300_000), "5.3 MB");
    assert_eq!(size_label(3 * 1024 * 1024 * 1024), "3.0 GB");
}

#[test]
fn a_conversation_lists_each_link_of_its_bodies_once() {
    let messages = [
        opened("alex@example.test", "Plans: https://example.test/plan"),
        opened(
            "sam@example.test",
            "> Plans: https://example.test/plan\nNotes: https://example.test/notes",
        ),
    ];
    let reading = Reading::new(&Conversation::default(), &messages, &[]);
    assert_eq!(
        reading.links,
        ["https://example.test/plan", "https://example.test/notes"]
    );
}
