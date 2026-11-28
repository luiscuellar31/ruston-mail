use super::*;

fn label(id: &str, path: &str) -> Label {
    Label {
        id: id.into(),
        name: path.rsplit('/').next().unwrap().into(),
        path: path.into(),
        ..Label::default()
    }
}

#[test]
fn system_folders_come_first_then_folders_under_their_parents_then_labels() {
    let folders = vec![
        label("f-work", "Work"),
        label("f-taxes-2024", "Taxes/2024"),
        label("f-taxes", "Taxes"),
    ];
    let labels = vec![label("l-later", "Later"), label("l-family", "Family")];
    let counts = [
        LabelCount {
            label_id: label_ids::INBOX.into(),
            total: 9,
            unread: 3,
        },
        LabelCount {
            label_id: "f-taxes-2024".into(),
            total: 2,
            unread: 1,
        },
    ];

    let places = places(folders, labels, &counts);
    let names: Vec<(&str, usize, i64)> = places
        .iter()
        .map(|place| (place.name.as_str(), place.depth, place.unread))
        .collect();
    assert_eq!(
        names,
        [
            ("Inbox", 0, 3),
            ("Drafts", 0, 0),
            ("Sent", 0, 0),
            ("Starred", 0, 0),
            ("Archive", 0, 0),
            ("Spam", 0, 0),
            ("Trash", 0, 0),
            ("Taxes", 0, 0),
            ("2024", 1, 1),
            ("Work", 0, 0),
            ("Later", 0, 0),
            ("Family", 0, 0),
        ]
    );
    assert_eq!(places[7].kind, PlaceKind::Folder);
    assert_eq!(places[10].kind, PlaceKind::Label);
    // What `in:` finds a folder by.
    let paths: Vec<&str> = places.iter().map(|place| place.path.as_str()).collect();
    assert_eq!(paths[..2], ["Inbox", "Drafts"]);
    assert_eq!(
        paths[7..],
        ["Taxes", "Taxes/2024", "Work", "Later", "Family"]
    );
}

#[test]
fn a_folder_whose_name_sorts_before_a_slash_leaves_children_under_their_parent() {
    let folders = vec![
        label("f-taxes-old", "Taxes-old"),
        label("f-taxes-2024", "Taxes/2024"),
        label("f-taxes-two", "Taxes 2"),
        label("f-taxes", "Taxes"),
    ];

    let places = places(folders, Vec::new(), &[]);
    let folders: Vec<(&str, usize)> = places
        .iter()
        .filter(|place| place.kind == PlaceKind::Folder)
        .map(|place| (place.path.as_str(), place.depth))
        .collect();
    assert_eq!(
        folders,
        [
            ("Taxes", 0),
            ("Taxes/2024", 1),
            ("Taxes 2", 0),
            ("Taxes-old", 0),
        ]
    );
}

#[tokio::test]
async fn an_attachment_saves_under_a_free_name_never_over_a_file() {
    let folder = std::env::temp_dir().join(format!(
        "ruston-tui-save-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&folder).unwrap();
    let mailbox = Mailbox::demo();
    let first = mailbox
        .save_attachment("msg-demo-03", "att-demo-01", &folder)
        .await
        .unwrap();
    assert_eq!(first, folder.join("translation_proposal.pdf"));
    let second = mailbox
        .save_attachment("msg-demo-03", "att-demo-01", &folder)
        .await
        .unwrap();
    assert_eq!(second, folder.join("translation_proposal (1).pdf"));
    let saved = std::fs::read_to_string(&first).unwrap();
    assert!(saved.contains("translation_proposal.pdf"), "{saved}");
    assert!(
        mailbox
            .save_attachment("msg-demo-03", "nope", &folder)
            .await
            .is_err()
    );
    std::fs::remove_dir_all(folder).unwrap();
}

#[tokio::test]
async fn the_demo_lists_its_places_and_conversations_as_its_commands_do() {
    let mailbox = Mailbox::demo();
    let places = mailbox.places().await.unwrap();
    let inbox = &places[0];
    assert_eq!(inbox.id, label_ids::INBOX);
    let counted = demo::demo_counts(true)
        .into_iter()
        .find(|count| count.label_id == label_ids::INBOX)
        .unwrap();
    assert_eq!(inbox.unread, counted.unread);
    assert!(
        places
            .iter()
            .any(|place| place.name == "Projects" && place.kind == PlaceKind::Folder)
    );
    assert!(
        places
            .iter()
            .any(|place| place.name == "Work" && place.kind == PlaceKind::Label)
    );

    let (total, page) = mailbox.conversations(label_ids::INBOX, 0).await.unwrap();
    assert!(
        total > 0 && page.len() as u32 == total,
        "{total} {}",
        page.len()
    );
    assert!(page.windows(2).all(|pair| pair[0].time >= pair[1].time));
    let (_, past_the_end) = mailbox.conversations(label_ids::INBOX, 9).await.unwrap();
    assert!(past_the_end.is_empty());
    let agenda = SearchOpts {
        keyword: Some("AGENDA".into()),
        ..SearchOpts::default()
    };
    let (found, matching) = mailbox.search(&agenda, 0).await.unwrap();
    assert_eq!(found, 1);
    assert_eq!(matching[0].subject, "Offsite planning and agenda");
    // The demo's local search looks in bodies too.
    let glossary = SearchOpts {
        keyword: Some("glossary".into()),
        ..SearchOpts::default()
    };
    let (found, matching) = mailbox.search_local(&glossary).await.unwrap();
    assert_eq!(found, 1);
    assert_eq!(matching[0].id, "conv-demo-02");

    let (conversation, messages) = mailbox.conversation(&page[0].id).await.unwrap();
    assert_eq!(conversation.id, page[0].id);
    assert!(!messages.is_empty());
    assert!(
        messages
            .iter()
            .all(|message| matches!(message, ConversationMessage::Opened(_)))
    );
    assert!(mailbox.conversation("no-such-conversation").await.is_err());
    mailbox
        .change(&[page[0].id.clone()], label_ids::INBOX, &Change::Read(true))
        .await
        .unwrap();
}

#[tokio::test]
async fn the_filters_of_a_local_search_narrow_what_its_words_find() {
    // The demo's index finds two proposals: Riley's, with Alex in copy,
    // in the Inbox on 2024-04-24, and Drew's, archived on 2024-04-22.
    let proposals = async |opts: SearchOpts| -> Vec<String> {
        let opts = SearchOpts {
            keyword: Some("proposal".into()),
            ..opts
        };
        let (found, conversations) = Mailbox::demo().search_local(&opts).await.unwrap();
        assert_eq!(found as usize, conversations.len());
        conversations.into_iter().map(|found| found.id).collect()
    };
    let (riley, drew) = ("conv-demo-02", "conv-demo-04");
    assert_eq!(proposals(SearchOpts::default()).await, [riley, drew]);
    for (opts, found) in [
        (
            SearchOpts {
                from: Some("RILEY".into()),
                ..SearchOpts::default()
            },
            riley,
        ),
        (
            SearchOpts {
                to: Some("alex.reed@".into()),
                ..SearchOpts::default()
            },
            riley,
        ),
        (
            SearchOpts {
                before: Some("2024-04-23".into()),
                ..SearchOpts::default()
            },
            drew,
        ),
        (
            SearchOpts {
                after: Some("2024-04-24".into()),
                ..SearchOpts::default()
            },
            riley,
        ),
        (
            SearchOpts {
                folder: Some(label_ids::ARCHIVE.into()),
                ..SearchOpts::default()
            },
            drew,
        ),
    ] {
        assert_eq!(proposals(opts.clone()).await, [found], "{opts:?}");
    }
    // All mail holds them both.
    let everywhere = SearchOpts {
        folder: Some(label_ids::ALL_MAIL.into()),
        ..SearchOpts::default()
    };
    assert_eq!(proposals(everywhere).await, [riley, drew]);
}

#[tokio::test]
async fn the_demo_keeps_what_it_sends_in_sent_and_protects_proton_mail_alone() {
    let mailbox = Mailbox::demo();
    assert_eq!(mailbox.sender(), demo::DEMO_EMAIL);
    let addresses = ["sam@proton.me".to_owned(), "alex@example.test".to_owned()];
    assert_eq!(
        mailbox.protection(&addresses).await,
        [
            (addresses[0].clone(), Ok(RecipientProtection::EndToEnd)),
            (addresses[1].clone(), Ok(RecipientProtection::Unencrypted)),
        ]
    );
    let outgoing = Outgoing {
        kind: OutgoingKind::New,
        to: vec![addresses[0].clone()],
        cc: Vec::new(),
        bcc: vec![addresses[1].clone()],
        subject: "Plans".into(),
        body: "See you there.".into(),
        attachments: Vec::new(),
    };
    mailbox.send(&outgoing).await.unwrap();
    // The newest, so the first in Sent.
    let (_, sent) = mailbox.conversations(label_ids::SENT, 0).await.unwrap();
    assert_eq!(sent[0].subject, "Plans");
    let (_, messages) = mailbox.conversation(&sent[0].id).await.unwrap();
    let [ConversationMessage::Opened(message)] = &messages[..] else {
        panic!("{messages:?}");
    };
    assert_eq!(message.body, "See you there.");
    assert_eq!(message.meta.sender.address, demo::DEMO_EMAIL);
    assert_eq!(message.meta.bcc_list[0].address, addresses[1]);
}

#[tokio::test]
async fn a_demo_answer_joins_its_conversation_with_core_s_subject_and_quote() {
    let mailbox = Mailbox::demo();
    let answer = |kind, body: &str| Outgoing {
        kind,
        to: vec!["sam.chen@example.com".into()],
        cc: Vec::new(),
        bcc: Vec::new(),
        subject: String::new(),
        body: body.into(),
        attachments: Vec::new(),
    };
    let reply = OutgoingKind::Reply {
        message_id: "msg-demo-04".into(),
        everyone: false,
        quote: Quote::Original,
    };
    mailbox.send(&answer(reply, "Count me in.")).await.unwrap();
    let (conversation, messages) = mailbox.conversation("conv-demo-03").await.unwrap();
    assert_eq!(conversation.num_messages, 2);
    let Some(ConversationMessage::Opened(sent)) = messages.last() else {
        panic!("{messages:?}");
    };
    assert_eq!(sent.meta.subject, "Re: Offsite planning and agenda");
    assert!(
        sent.body
            .starts_with("Count me in.\n\n------- Original Message -------\nOn Sam Chen <"),
        "{}",
        sent.body
    );
    assert!(sent.body.contains("\n> "), "{}", sent.body);
    // Sent lists the conversation it answered.
    let (_, listed) = mailbox.conversations(label_ids::SENT, 0).await.unwrap();
    assert_eq!(listed[0].id, "conv-demo-03");

    let forward = OutgoingKind::Forward {
        message_id: "msg-demo-04".into(),
        quote: Quote::Omitted,
        leave_out: Vec::new(),
    };
    mailbox.send(&answer(forward, "FYI")).await.unwrap();
    let (_, messages) = mailbox.conversation("conv-demo-03").await.unwrap();
    let Some(ConversationMessage::Opened(sent)) = messages.last() else {
        panic!("{messages:?}");
    };
    assert_eq!(sent.meta.subject, "Fw: Offsite planning and agenda");
    assert_eq!(sent.body, "FYI");
}

#[tokio::test]
async fn the_demo_sends_files_and_forwards_the_original_s_but_those_left_out() {
    let mailbox = Mailbox::demo();
    let file =
        std::env::temp_dir().join(format!("ruston-tui-demo-file-{}.txt", std::process::id()));
    std::fs::write(&file, b"twelve bytes").unwrap();
    let forward = |leave_out: Vec<String>| Outgoing {
        kind: OutgoingKind::Forward {
            message_id: "msg-demo-03".into(),
            quote: Quote::Omitted,
            leave_out,
        },
        to: vec!["sam.chen@example.com".into()],
        cc: Vec::new(),
        bcc: Vec::new(),
        subject: String::new(),
        body: "FYI".into(),
        attachments: vec![file.clone()],
    };
    let names = |messages: &[ConversationMessage]| -> Vec<(String, u64)> {
        let Some(ConversationMessage::Opened(sent)) = messages.last() else {
            panic!("{messages:?}");
        };
        sent.attachments
            .iter()
            .map(|attachment| (attachment.name.clone(), attachment.size))
            .collect()
    };
    mailbox.send(&forward(Vec::new())).await.unwrap();
    let (_, messages) = mailbox.conversation("conv-demo-02").await.unwrap();
    let file_name = file.file_name().unwrap().to_string_lossy().into_owned();
    let attached = names(&messages);
    assert_eq!(attached[0].0, "translation_proposal.pdf");
    assert_eq!(attached[1], (file_name.clone(), 12));
    // Left out, the original's goes no more; what was sent saves.
    mailbox
        .send(&forward(vec!["att-demo-01".into()]))
        .await
        .unwrap();
    let (_, messages) = mailbox.conversation("conv-demo-02").await.unwrap();
    assert_eq!(names(&messages), [(file_name, 12)]);
    std::fs::remove_file(file).unwrap();
}

#[tokio::test]
async fn mail_is_deleted_forever_from_trash_and_spam_alone() {
    let mailbox = Mailbox::demo();
    let ids = |(_, page): (u32, Vec<Conversation>)| -> Vec<String> {
        page.into_iter()
            .map(|conversation| conversation.id)
            .collect()
    };
    let inbox = ids(mailbox.conversations(label_ids::INBOX, 0).await.unwrap());
    assert!(
        mailbox
            .change(&inbox, label_ids::INBOX, &Change::Delete)
            .await
            .is_err()
    );
    assert!(mailbox.empty(label_ids::INBOX).await.is_err());
    assert!(mailbox.empty(label_ids::ALL_MAIL).await.is_err());
    assert_eq!(
        ids(mailbox.conversations(label_ids::INBOX, 0).await.unwrap()),
        inbox
    );

    let trash = ids(mailbox.conversations(label_ids::TRASH, 0).await.unwrap());
    assert!(!trash.is_empty());
    mailbox
        .change(&trash, label_ids::TRASH, &Change::Delete)
        .await
        .unwrap();
    assert!(ids(mailbox.conversations(label_ids::TRASH, 0).await.unwrap()).is_empty());
    mailbox
        .change(
            &inbox[..1],
            label_ids::INBOX,
            &Change::Move(label_ids::SPAM.into()),
        )
        .await
        .unwrap();
    mailbox.empty(label_ids::SPAM).await.unwrap();
    assert!(ids(mailbox.conversations(label_ids::SPAM, 0).await.unwrap()).is_empty());
    assert_eq!(
        ids(mailbox.conversations(label_ids::INBOX, 0).await.unwrap()),
        inbox[1..]
    );
}
