use super::*;

#[test]
fn each_conversation_carries_the_labels_of_its_messages() {
    let DemoMailbox {
        messages,
        conversations,
    } = fixtures();
    for conversation in &conversations {
        let own: Vec<&MessageMetadata> = messages
            .iter()
            .map(|message| &message.meta)
            .filter(|meta| meta.conversation_id == conversation.id)
            .collect();
        let mut carried: Vec<&str> = own
            .iter()
            .flat_map(|meta| meta.label_ids.iter().map(String::as_str))
            .collect();
        carried.sort_unstable();
        carried.dedup();
        let mut labelled: Vec<&str> = conversation
            .labels
            .iter()
            .map(|label| label.id.as_str())
            .collect();
        labelled.sort_unstable();
        assert_eq!(labelled, carried, "{}", conversation.id);
        for label in &conversation.labels {
            let with: Vec<_> = own
                .iter()
                .filter(|meta| meta.label_ids.contains(&label.id))
                .collect();
            let unread = with.iter().filter(|meta| meta.unread > 0).count();
            let at = format!("{} in {}", conversation.id, label.id);
            assert_eq!(label.context_num_messages, Some(with.len() as i64), "{at}");
            assert_eq!(label.context_num_unread, Some(unread as i64), "{at}");
        }
    }
}

#[test]
fn a_search_narrows_by_place_days_and_people_as_the_real_one_does() {
    assert_eq!(search_label(Some("all")).unwrap(), None);
    assert_eq!(search_label(None).unwrap(), None);
    assert_eq!(
        search_label(Some("archive")).unwrap().as_deref(),
        Some(label_ids::ARCHIVE)
    );
    assert!(search_label(Some("No such folder")).is_err());

    // 2024-04-23, all of it in UTC.
    let day = search_window(Some("2024-04-23"), Some("2024-04-23")).unwrap();
    assert!(within(day, 1_713_830_400) && within(day, 1_713_916_799));
    assert!(!within(day, 1_713_830_399) && !within(day, 1_713_916_800));
    assert!(within((None, None), 0));

    let alex = Recipient {
        name: "Alex Reed".into(),
        address: "alex.reed@example.com".into(),
        ..Recipient::default()
    };
    assert!(someone_matches([&alex], "REED") && someone_matches([&alex], "alex.reed@"));
    assert!(!someone_matches([&alex], "sam"));
}

#[test]
fn a_session_starts_with_the_counts_the_demo_gives() {
    let counts = DemoSession::new().counts();
    for expected in demo_counts(true) {
        let counted = counts
            .iter()
            .find(|count| count.label_id == expected.label_id)
            .unwrap();
        assert_eq!(
            (counted.total, counted.unread),
            (expected.total, expected.unread),
            "{}",
            expected.label_id
        );
    }
}

#[test]
fn a_session_keeps_what_it_marks_read_and_counts_it() {
    let session = DemoSession::new();
    let unread = |label: &str| {
        session
            .counts()
            .iter()
            .find(|count| count.label_id == label)
            .map_or(0, |count| count.unread)
    };
    assert_eq!(
        (unread(label_ids::INBOX), unread(label_ids::STARRED)),
        (2, 1)
    );
    session.change("conv-demo-01", label_ids::INBOX, &Change::Read(true));
    assert_eq!(
        (unread(label_ids::INBOX), unread(label_ids::STARRED)),
        (1, 0)
    );
    assert_eq!(unread("label-demo-work"), 1);
    let (_, inbox) = session.conversations(label_ids::INBOX, 0, 50);
    let first = inbox.iter().find(|c| c.id == "conv-demo-01").unwrap();
    assert_eq!(first.num_unread, 0);
    // A new session starts from the fixtures again.
    assert_eq!(
        DemoSession::new()
            .counts()
            .iter()
            .find(|count| count.label_id == label_ids::INBOX)
            .map(|count| count.unread),
        Some(2)
    );
}

#[test]
fn a_session_marks_a_conversations_messages_read_or_unread_as_proton_would() {
    let session = DemoSession::new();
    let id = "conv-demo-01";
    let unread = || -> Vec<i64> {
        let (_, messages) = session.conversation(id).unwrap();
        messages
            .iter()
            .map(|message| match message {
                ConversationMessage::Opened(full) => full.meta.unread,
                ConversationMessage::Unreadable(unreadable) => unreadable.meta.unread,
            })
            .collect()
    };
    // Alex's review, not read yet, then the reply sent to it.
    assert_eq!(unread(), [1, 0]);
    session.change(id, label_ids::INBOX, &Change::Read(true));
    assert_eq!(unread(), [0, 0]);
    // Unread again, its newest in the Inbox is, not the reply in Sent.
    session.change(id, label_ids::INBOX, &Change::Read(false));
    assert_eq!(unread(), [1, 0]);
    // Unread already, it stays as it is.
    session.change(id, label_ids::SENT, &Change::Read(false));
    assert_eq!(unread(), [1, 0]);
    // Where none of its messages is, as in All mail here, its newest is.
    session.change(id, label_ids::INBOX, &Change::Read(true));
    session.change(id, label_ids::ALL_MAIL, &Change::Read(false));
    assert_eq!(unread(), [0, 1]);
}

#[test]
fn a_session_stars_marks_unread_and_moves_as_proton_would() {
    let session = DemoSession::new();
    let count = |label: &str| {
        session
            .counts()
            .iter()
            .find(|count| count.label_id == label)
            .map_or((0, 0), |count| (count.total, count.unread))
    };
    let listed = |label: &str| {
        let (_, page) = session.conversations(label, 0, 50);
        page.into_iter().map(|c| c.id).collect::<Vec<_>>()
    };
    // Offsite planning, read and in the Inbox only.
    let id = "conv-demo-03";
    session.change(id, label_ids::INBOX, &Change::Star(true));
    session.change(id, label_ids::INBOX, &Change::Star(true));
    assert_eq!(count(label_ids::STARRED), (2, 1));
    session.change(id, label_ids::INBOX, &Change::Read(false));
    assert_eq!(count(label_ids::INBOX), (4, 3));
    session.change(
        id,
        label_ids::INBOX,
        &Change::Move(label_ids::ARCHIVE.into()),
    );
    assert!(!listed(label_ids::INBOX).contains(&id.to_string()));
    assert!(listed(label_ids::ARCHIVE).contains(&id.to_string()));
    // Still starred, and unread wherever it is now.
    assert!(listed(label_ids::STARRED).contains(&id.to_string()));
    assert_eq!(count(label_ids::ARCHIVE), (2, 1));
    session.change(id, label_ids::ARCHIVE, &Change::Star(false));
    assert_eq!(count(label_ids::STARRED), (1, 1));
    // The reply in Sent stays there when its conversation is trashed.
    let first = "conv-demo-01";
    session.change(
        first,
        label_ids::INBOX,
        &Change::Move(label_ids::TRASH.into()),
    );
    assert!(listed(label_ids::TRASH).contains(&first.to_string()));
    assert!(listed(label_ids::SENT).contains(&first.to_string()));
    assert!(!listed(label_ids::INBOX).contains(&first.to_string()));
}

#[test]
fn a_session_moves_a_conversations_messages_with_it() {
    let session = DemoSession::new();
    // Alex's review, in the Inbox, starred and labelled Work, then the
    // reply sent to it.
    let id = "conv-demo-01";
    session.change(
        id,
        label_ids::INBOX,
        &Change::Move(label_ids::ARCHIVE.into()),
    );
    let (_, messages) = session.conversation(id).unwrap();
    let places: Vec<Vec<String>> = messages
        .iter()
        .map(|message| match message {
            ConversationMessage::Opened(full) => full.meta.label_ids.clone(),
            ConversationMessage::Unreadable(unreadable) => unreadable.meta.label_ids.clone(),
        })
        .collect();
    assert_eq!(
        places,
        [
            vec![label_ids::STARRED, "label-demo-work", label_ids::ARCHIVE],
            vec![label_ids::SENT, "label-demo-work", label_ids::ARCHIVE],
        ]
    );
}

#[test]
fn a_session_deletes_forever_and_empties_what_was_thrown_away() {
    let session = DemoSession::new();
    let listed = |label: &str| {
        let (_, page) = session.conversations(label, 0, 50);
        page.into_iter().map(|c| c.id).collect::<Vec<_>>()
    };
    // Garden fence, in Trash alone.
    session.change("conv-demo-06", label_ids::TRASH, &Change::Delete);
    assert!(listed(label_ids::TRASH).is_empty());
    assert!(!listed(label_ids::ALL_MAIL).contains(&"conv-demo-06".to_owned()));
    // Emptying Spam takes all that is in it, wherever else it was
    // labelled, and nothing else.
    for id in ["conv-demo-02", "conv-demo-03"] {
        session.change(id, label_ids::INBOX, &Change::Move(label_ids::SPAM.into()));
    }
    session.empty(label_ids::SPAM);
    assert!(listed(label_ids::SPAM).is_empty());
    assert_eq!(listed(label_ids::INBOX), ["conv-demo-01", "conv-demo-05"]);
    assert_eq!(listed("label-demo-work"), ["conv-demo-01"]);
    let all = session
        .counts()
        .into_iter()
        .find(|count| count.label_id == label_ids::ALL_MAIL)
        .map(|count| count.total);
    // The three deleted of seven, the draft's among those left.
    assert_eq!(all, Some(4));
}

#[test]
fn a_session_finds_conversations_by_their_subject_wherever_they_are() {
    let session = DemoSession::new();
    let search = |words: &str| {
        let opts = SearchOpts {
            keyword: Some(words.into()),
            ..SearchOpts::default()
        };
        session.search(&opts, 0, 50).unwrap()
    };
    let found = |words: &str| {
        let (total, page) = search(words);
        (total, page.into_iter().map(|c| c.id).collect::<Vec<_>>())
    };
    assert_eq!(found("OFFSITE"), (1, vec!["conv-demo-03".to_owned()]));
    // In Trash too, and newest first.
    let (total, ids) = found("e");
    assert!(
        total > 1 && ids.contains(&"conv-demo-06".to_owned()),
        "{ids:?}"
    );
    assert_eq!(found("nothing like this"), (0, Vec::new()));
    // A change made in the session shows in what it finds.
    session.change(
        "conv-demo-03",
        label_ids::INBOX,
        &Change::Move(label_ids::TRASH.into()),
    );
    let (_, page) = search("offsite");
    assert!(page[0].labels.iter().any(|l| l.id == label_ids::TRASH));
}

#[test]
fn a_session_search_narrows_by_people_days_and_place() {
    let session = DemoSession::new();
    let found = |opts: SearchOpts| -> Vec<String> {
        let (_, page) = session.search(&opts, 0, 50).unwrap();
        page.into_iter().map(|c| c.id).collect()
    };
    let from = SearchOpts {
        from: Some("RILEY".into()),
        ..SearchOpts::default()
    };
    assert_eq!(found(from), ["conv-demo-02"]);
    // The Archive's of 2024-04-22 is left out.
    let inbox_days = SearchOpts {
        after: Some("2024-04-21".into()),
        before: Some("2024-04-23".into()),
        folder: Some(label_ids::INBOX.into()),
        ..SearchOpts::default()
    };
    assert_eq!(found(inbox_days), ["conv-demo-03", "conv-demo-05"]);
    // A label by its ID; All mail holds every conversation.
    let work = SearchOpts {
        folder: Some("label-demo-work".into()),
        ..SearchOpts::default()
    };
    assert_eq!(found(work), ["conv-demo-01", "conv-demo-02"]);
    let to = SearchOpts {
        to: Some(DEMO_EMAIL.into()),
        folder: Some(label_ids::ALL_MAIL.into()),
        ..SearchOpts::default()
    };
    assert_eq!(found(to).len(), 6);
    let backwards = SearchOpts {
        after: Some("2024-04-23".into()),
        before: Some("2024-04-21".into()),
        ..SearchOpts::default()
    };
    assert!(session.search(&backwards, 0, 50).is_err());
}

#[test]
fn a_session_labels_and_unlabels_as_proton_would() {
    let session = DemoSession::new();
    let listed = |label: &str| {
        let (_, page) = session.conversations(label, 0, 50);
        page.into_iter().map(|c| c.id).collect::<Vec<_>>()
    };
    let id = "conv-demo-03";
    session.change(
        id,
        label_ids::INBOX,
        &Change::Label("label-demo-personal".into()),
    );
    session.change(
        id,
        label_ids::INBOX,
        &Change::Label("label-demo-personal".into()),
    );
    assert_eq!(
        listed("label-demo-personal"),
        ["conv-demo-03", "conv-demo-05"]
    );
    // A label stays when the conversation moves.
    session.change(id, label_ids::INBOX, &Change::Move(label_ids::SPAM.into()));
    assert!(listed("label-demo-personal").contains(&id.to_string()));
    assert!(listed(label_ids::SPAM).contains(&id.to_string()));
    session.change(
        id,
        label_ids::SPAM,
        &Change::Unlabel("label-demo-personal".into()),
    );
    assert_eq!(listed("label-demo-personal"), ["conv-demo-05"]);
}

#[test]
fn the_conversation_counts_agree_with_the_conversations() {
    let conversations = fixtures().conversations;
    let unread_in = |conversation: &Conversation, id: &str| {
        conversation
            .labels
            .iter()
            .any(|label| label.id == id && label.context_num_unread.unwrap_or(0) > 0)
    };
    let counts = demo_counts(true);
    for count in &counts {
        let (total, unread) = if count.label_id == label_ids::ALL_MAIL {
            let unread = conversations.iter().filter(|c| c.num_unread > 0).count();
            (conversations.len(), unread)
        } else {
            let id = count.label_id.as_str();
            let listed = conversations
                .iter()
                .filter(|c| c.labels.iter().any(|label| label.id == id))
                .count();
            let unread = conversations.iter().filter(|c| unread_in(c, id)).count();
            (listed, unread)
        };
        assert_eq!(count.total, total as i64, "{}", count.label_id);
        assert_eq!(count.unread, unread as i64, "{}", count.label_id);
    }
    // Every place with unread mail says how much, as Proton's counts do.
    for conversation in &conversations {
        for label in &conversation.labels {
            if unread_in(conversation, &label.id) {
                let counted = counts.iter().any(|count| count.label_id == label.id);
                assert!(counted, "{} has no count", label.id);
            }
        }
    }
}
