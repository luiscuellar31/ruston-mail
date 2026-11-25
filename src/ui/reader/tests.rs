//! Tests of the reader pane: its layout, controls and messages.

use jiff::civil::date;

use super::*;
use crate::mail::ConversationDetail;
use crate::ui::test_support::{text_rect, visible_text_rect};
use ruston_core::html::rich::ListMarker;

/// A message with no pictures fetched, nor parts to fetch them from.
fn no_pictures() -> Pictures<'static> {
    static NONE: std::sync::LazyLock<HashMap<String, InlineImage>> =
        std::sync::LazyLock::new(HashMap::new);
    Pictures {
        images: &NONE,
        message_id: "",
        attachments: &[],
    }
}

#[test]
fn the_reading_column_is_bounded_and_centred() {
    // Narrower than a column: it gives way, and never asks for a gap it
    // cannot have.
    assert_eq!(reading_column_place(400.0), (0.0, 400.0));
    // Exactly a column: no gap to share out.
    assert_eq!(reading_column_place(READING_WIDTH), (0.0, READING_WIDTH));
    // Wider: the column keeps its width and the rest is split evenly, so
    // it sits in the middle instead of against the divider.
    let (gap, width) = reading_column_place(READING_WIDTH + 400.0);
    assert_eq!(width, READING_WIDTH);
    assert_eq!(gap, 200.0);
    // A panel dragged to nothing must not produce a negative gap.
    assert_eq!(reading_column_place(0.0), (0.0, 0.0));
}

#[test]
fn reader_scrollbar_overlays_without_resizing_content() {
    let style = theme::panel_scroll_style();

    assert!(style.floating);
    assert_eq!(style.allocated_width(), 0.0);
    let handle_center = -style.bar_outer_margin - style.floating_width * 0.5;
    assert_eq!(handle_center, theme::PANEL_PADDING as f32 * 0.5);
}

#[test]
fn conversation_actions_share_one_compact_toolbar() {
    // Archived mail gets every control at once: the way back to the
    // Inbox, both menus and a label count.
    let (mailbox, _, _) = loading_reader("Message", Folder::System(MailFolder::Archive), 1);
    let summary = mailbox.selected_summary().unwrap();
    let places = [
        Folder::label("work", "Work"),
        Folder::custom("kZ9", "Invoices"),
    ];
    let context = egui::Context::default();
    let mut height = 0.0;

    context
        .run_ui(egui::RawInput::default(), |ui| {
            ui.set_width(680.0);
            action_toolbar(
                ui,
                summary,
                &mailbox,
                &["work".to_owned()],
                &places,
                true,
                &mut Vec::new(),
            );
            height = ui.min_rect().height();
        })
        .drop_without_applying_deltas();

    assert!(height <= 32.0, "toolbar wrapped to {height} points");
}

#[test]
fn mail_in_archive_spam_or_trash_is_offered_the_way_back_to_the_inbox() {
    let archive = Folder::System(MailFolder::Archive);
    let spam = Folder::System(MailFolder::Spam);
    let trash = Folder::System(MailFolder::Trash);

    assert_eq!(back_to_inbox(Some(&archive)), Some("Move to Inbox"));
    assert_eq!(back_to_inbox(Some(&trash)), Some("Move to Inbox"));
    assert_eq!(back_to_inbox(Some(&spam)), Some("Not spam (move to Inbox)"));
    assert_eq!(back_to_inbox(Some(&Folder::INBOX)), None);
    assert_eq!(
        back_to_inbox(Some(&Folder::custom("kZ9", "Invoices"))),
        None
    );
    // Under a label, or in search results, the row could be anywhere.
    assert_eq!(back_to_inbox(None), None);
}

#[test]
fn the_move_menu_lists_the_inbox_and_the_accounts_folders() {
    let invoices = Folder::custom("kZ9", "Invoices");
    let work = Folder::label("work", "Work");
    let archive = Folder::System(MailFolder::Archive);
    let places = [work.clone(), invoices.clone()];
    let names = |location: Option<&Folder>, view: &Folder| {
        move_destinations(location, view, &places)
            .into_iter()
            .map(Folder::name)
            .collect::<Vec<_>>()
    };

    // A label is given, not moved into, so the Labels menu has it.
    assert_eq!(names(Some(&Folder::INBOX), &Folder::INBOX), ["Invoices"]);
    // The first button already leads back to the Inbox.
    assert_eq!(names(Some(&archive), &archive), ["Invoices"]);
    assert_eq!(names(Some(&invoices), &invoices), ["Inbox"]);
    assert_eq!(names(None, &work), ["Inbox", "Invoices"]);
    // Proton keeps the account's own mail out of the Inbox.
    for own in [MailFolder::Sent, MailFolder::Drafts] {
        assert_eq!(names(None, &Folder::System(own)), ["Invoices"]);
    }
}

/// A click of `button` at `pos`: the pointer moves there, presses and lets go.
fn click_events(pos: egui::Pos2, button: egui::PointerButton) -> Vec<egui::Event> {
    let press = |pressed| egui::Event::PointerButton {
        pos,
        button,
        pressed,
        modifiers: egui::Modifiers::default(),
    };
    vec![egui::Event::PointerMoved(pos), press(true), press(false)]
}

/// One frame's input with a click at `position`.
fn click_at(input: &egui::RawInput, position: egui::Pos2) -> egui::RawInput {
    egui::RawInput {
        events: click_events(position, egui::PointerButton::Primary),
        ..input.clone()
    }
}

#[test]
fn the_first_action_outside_the_inbox_puts_the_conversation_back() {
    for folder in [MailFolder::Archive, MailFolder::Spam, MailFolder::Trash] {
        let (mut mailbox, request, detail) = loading_reader("Message", Folder::System(folder), 1);
        mailbox.finish_conversation(&request, Ok(detail));
        let context = egui::Context::default();
        theme::install(&context);
        let mut state = UiState::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(540.0, 950.0),
            )),
            ..Default::default()
        };
        let (shapes, _) = reader_frame(
            &context,
            &mailbox,
            &[],
            Reading::default(),
            &mut state,
            input.clone(),
        );
        let first = buttons(&shapes)[0].center();

        let (_, messages) = reader_frame(
            &context,
            &mailbox,
            &[],
            Reading::default(),
            &mut state,
            click_at(&input, first),
        );

        assert!(
            matches!(
                messages.as_slice(),
                [Message::ApplyAction(MailAction::MoveTo(to))] if *to == Folder::INBOX
            ),
            "the first action in {folder:?} did not lead back to the Inbox"
        );
    }
}

#[test]
fn the_conversation_takes_actions_while_the_list_refreshes() {
    let (mut mailbox, request, detail) = loading_reader("Message", Folder::INBOX, 1);
    mailbox.finish_conversation(&request, Ok(detail));
    let _ = mailbox.refresh(9).unwrap();
    let context = egui::Context::default();
    theme::install(&context);
    let mut state = UiState::default();
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(540.0, 950.0),
        )),
        ..Default::default()
    };
    let (shapes, _) = reader_frame(
        &context,
        &mailbox,
        &[],
        Reading::default(),
        &mut state,
        input.clone(),
    );
    let archive = buttons(&shapes)[0].center();

    let (_, messages) = reader_frame(
        &context,
        &mailbox,
        &[],
        Reading::default(),
        &mut state,
        click_at(&input, archive),
    );

    assert!(
        matches!(
            messages.as_slice(),
            [Message::ApplyAction(MailAction::MoveTo(to))]
                if *to == Folder::System(MailFolder::Archive)
        ),
        "the toolbar did not archive while the list refreshed"
    );
}

#[test]
fn a_rows_menu_acts_while_the_list_refreshes() {
    let (mut mailbox, _, _) = loading_reader("Message", Folder::INBOX, 1);
    let _ = mailbox.refresh(9).unwrap();
    let row = mailbox.conversations()[0].clone();
    let context = egui::Context::default();
    theme::install(&context);
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(540.0, 950.0),
        )),
        ..Default::default()
    };
    let frame = |input: egui::RawInput| {
        let mut messages = Vec::new();
        let output = context.run_ui(input, |ui| {
            conversation_menu(ui, &mailbox, &row, &mut messages);
        });
        let shapes = output.shapes.clone();
        output.drop_without_applying_deltas();
        (shapes, messages)
    };

    let (shapes, _) = frame(input.clone());
    let (_, messages) = frame(click_at(&input, text_rect(&shapes, "Archive").center()));

    assert!(
        matches!(
            messages.as_slice(),
            [Message::ApplyRowAction(id, MailAction::MoveTo(to))]
                if id == "conversation" && *to == Folder::System(MailFolder::Archive)
        ),
        "the row's menu did not archive while the list refreshed"
    );
}

#[test]
fn the_move_menu_moves_the_conversation_into_a_folder() {
    let invoices = Folder::custom("kZ9", "Invoices");
    let places = [invoices.clone()];
    let (mut mailbox, request, detail) = loading_reader("Message", Folder::INBOX, 1);
    mailbox.finish_conversation(&request, Ok(detail));
    let context = egui::Context::default();
    theme::install(&context);
    let mut state = UiState::default();
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(540.0, 950.0),
        )),
        ..Default::default()
    };
    let mut frame = |input: egui::RawInput| {
        reader_frame(
            &context,
            &mailbox,
            &places,
            Reading::default(),
            &mut state,
            input,
        )
    };

    let (shapes, _) = frame(input.clone());
    frame(click_at(&input, text_rect(&shapes, "Move to…").center()));
    // A new popup is measured on its first frame and shown on the next.
    frame(input.clone());
    let (shapes, _) = frame(input.clone());
    let (_, messages) = frame(click_at(&input, text_rect(&shapes, "Invoices").center()));

    assert!(
        matches!(
            messages.as_slice(),
            [Message::ApplyAction(MailAction::MoveTo(to))] if *to == invoices
        ),
        "the menu did not move the conversation into Invoices"
    );
}

#[test]
fn a_long_link_scrolls_inside_its_prompt_and_leaves_the_buttons_in_view() {
    let link = PendingLink {
        url: format!("https://example.com/?{}", "tracking=1&".repeat(400)),
        target: "example.com".into(),
    };
    let context = egui::Context::default();
    theme::install(&context);
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 400.0));
    let frame = |events: Vec<egui::Event>| {
        let mut answer = None;
        let output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            },
            |ui| answer = link_dialog(ui.ctx(), &link),
        );
        let open = output
            .shapes
            .iter()
            .find_map(|clipped| crate::ui::test_support::shape_text_rect(&clipped.shape, "Open"));
        output.drop_without_applying_deltas();
        (open, answer)
    };

    let mut open = None;
    for _ in 0..3 {
        open = frame(Vec::new()).0;
    }
    let open = open.expect("the prompt offers Open");
    assert!(screen.contains_rect(open), "Open is at {open:?}");

    let (_, answer) = frame(click_events(open.center(), egui::PointerButton::Primary));
    assert!(matches!(answer, Some(Message::OpenLink)));
}

#[test]
fn text_the_sender_hid_shows_only_when_asked_for_and_in_grey() {
    let word = |text: &str, hidden: bool| RichSpan {
        text: text.into(),
        hidden,
        ..RichSpan::default()
    };
    let spans = [word("Pay", false), word("x", true), word("Pal", false)];
    let context = egui::Context::default();
    theme::install(&context);
    let drawn = |reading: Reading| {
        let mut muted = None;
        let output = context.run_ui(egui::RawInput::default(), |ui| {
            muted = Some(theme::colors(ui).muted);
            rich_spans(ui, &spans, None, reading, &mut Vec::new());
        });
        let texts: Vec<(String, bool)> = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => Some((
                    text.galley.text().to_owned(),
                    Some(text.galley.job.sections[0].format.color) == muted,
                )),
                _ => None,
            })
            .collect();
        output.drop_without_applying_deltas();
        texts
    };
    let text = |text: &str, grey: bool| (text.to_owned(), grey);

    assert_eq!(
        drawn(Reading::default()),
        [text("Pay", false), text("Pal", false)]
    );
    assert_eq!(
        drawn(Reading {
            show_hidden_text: true,
            ..Reading::default()
        }),
        [text("Pay", false), text("x", true), text("Pal", false)]
    );
}

#[test]
fn the_senders_style_counts_unless_settings_leave_it_out() {
    let styled = RichSpan {
        text: "Sale".into(),
        styled: ruston_core::html::rich::SenderStyle {
            strong: true,
            ..Default::default()
        },
        ..RichSpan::default()
    };
    let centered = RichBlock {
        kind: BlockKind::Paragraph(vec![styled.clone()]),
        quote_depth: 0,
        hidden: false,
        centered: true,
        joined: false,
    };
    let context = egui::Context::default();
    theme::install(&context);
    // Where the word is drawn, and whether in the bold ink.
    let drawn = |reading: Reading| {
        let mut strong = None;
        let output = context.run_ui(egui::RawInput::default(), |ui| {
            ui.set_width(600.0);
            strong = Some(ui.visuals().strong_text_color());
            block(ui, &centered, no_pictures(), reading, &mut Vec::new());
        });
        let found = output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) if text.galley.text() == "Sale" => Some((
                    text.pos.x,
                    Some(text.galley.job.sections[0].format.color) == strong,
                )),
                _ => None,
            })
            .expect("the word is drawn");
        output.drop_without_applying_deltas();
        found
    };

    let (left, bold) = drawn(Reading::default());
    assert!(bold, "bold where the sender's style says so");
    assert!(left > 200.0, "a short centered line is centered: {left}");
    let (left, bold) = drawn(Reading {
        sender_text_styles: false,
        ..Reading::default()
    });
    assert!(!bold);
    assert!(left < 50.0, "only the tags count: {left}");
}

#[test]
fn a_table_of_data_lines_its_columns_up_unless_settings_read_it_as_lines() {
    let body = ruston_core::html::rich::parse(
        "<table><tr><th>Item</th><th>Price</th></tr>\
         <tr><td>Coffee</td><td>$7.00</td></tr></table>",
    );
    let table = body.blocks[0].clone();
    assert!(matches!(table.kind, BlockKind::Table(_)));
    let context = egui::Context::default();
    theme::install(&context);
    let drawn = |reading: Reading| {
        let mut aligned = false;
        // A grid settles its columns over a frame or two.
        for _ in 0..3 {
            let output = context.run_ui(egui::RawInput::default(), |ui| {
                ui.set_width(600.0);
                block(ui, &table, no_pictures(), reading, &mut Vec::new());
            });
            // Where a word's first letter is: a label in a wrapping line
            // starts at the line's left and moves its text along.
            let left = |word: &str| {
                output
                    .shapes
                    .iter()
                    .find_map(|clipped| match &clipped.shape {
                        egui::Shape::Text(text) if text.galley.text() == word => {
                            let row = &text.galley.rows[0];
                            Some(text.pos.x + row.pos.x + row.row.glyphs[0].pos.x)
                        }
                        _ => None,
                    })
                    .expect("the word is drawn")
            };
            aligned = (left("Price") - left("$7.00")).abs() < 1.0;
            output.drop_without_applying_deltas();
        }
        aligned
    };

    assert!(
        drawn(Reading::default()),
        "the price sits under its heading"
    );
    assert!(!drawn(Reading {
        data_tables: false,
        ..Reading::default()
    }));
}

#[test]
fn lines_the_sender_set_tight_are_drawn_as_next_lines() {
    let body = ruston_core::html::rich::parse(
        "<p style='margin:0'>Check-in</p><p style='margin:0'>Thu, Dec 4</p>\
         <p>A paragraph of its own</p>",
    );
    let context = egui::Context::default();
    theme::install(&context);
    let output = context.run_ui(egui::RawInput::default(), |ui| {
        ui.set_width(600.0);
        blocks(
            ui,
            &body.blocks,
            0,
            no_pictures(),
            Reading::default(),
            &mut Vec::new(),
        );
    });
    let shapes = output.shapes.clone();
    output.drop_without_applying_deltas();

    let label = text_rect(&shapes, "Check-in");
    let date = text_rect(&shapes, "Thu, Dec 4");
    let paragraph = text_rect(&shapes, "A paragraph of its own");
    assert!(
        (date.top() - label.bottom()).abs() < 1.0,
        "the date sits right under its label: {label:?} {date:?}"
    );
    assert!(
        paragraph.top() - date.bottom() >= 12.0,
        "a paragraph keeps its room: {date:?} {paragraph:?}"
    );
}

#[test]
fn a_picture_inside_a_link_opens_it_through_the_prompt() {
    let picture = |link: Option<&str>| RichBlock {
        kind: BlockKind::Image {
            description: "Go to Marie".into(),
            content_id: None,
            link: link.map(Into::into),
        },
        quote_depth: 0,
        hidden: false,
        centered: false,
        joined: false,
    };
    let context = egui::Context::default();
    theme::install(&context);
    let frame = |picture: &RichBlock, events: Vec<egui::Event>| {
        let mut messages = Vec::new();
        let output = context.run_ui(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ui| {
                ui.set_width(600.0);
                block(
                    ui,
                    picture,
                    no_pictures(),
                    Reading::default(),
                    &mut messages,
                );
            },
        );
        let note = output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text)
                    if text.galley.text() == "Image not loaded: Go to Marie" =>
                {
                    Some(egui::Rect::from_min_size(text.pos, text.galley.size()))
                }
                _ => None,
            })
            .expect("the description stands in for the picture");
        output.drop_without_applying_deltas();
        (note, messages)
    };

    for link in [Some("https://example.com/profile"), None] {
        let picture = picture(link);
        let (note, _) = frame(&picture, Vec::new());
        let press = |pressed| egui::Event::PointerButton {
            pos: note.center(),
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        let _ = frame(
            &picture,
            vec![egui::Event::PointerMoved(note.center()), press(true)],
        );
        let (_, messages) = frame(&picture, vec![press(false)]);
        match link {
            Some(link) => assert!(
                matches!(&messages[..], [Message::LinkClicked(clicked)] if clicked == link),
                "a click goes through the prompt that says where it leads"
            ),
            None => assert!(messages.is_empty(), "{} messages", messages.len()),
        }
    }
}

#[test]
fn a_link_the_sender_drew_as_a_button_is_one_and_still_asks_where_it_goes() {
    let part = |text: &str, strong: bool| RichSpan {
        text: text.into(),
        strong,
        link: Some("https://shop.example/".into()),
        button: true,
        ..RichSpan::default()
    };
    let spans = [part("Buy ", false), part("now", true)];
    let context = egui::Context::default();
    theme::install(&context);
    let frame = |reading: Reading, events: Vec<egui::Event>| {
        let mut messages = Vec::new();
        let output = context.run_ui(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ui| rich_spans(ui, &spans, None, reading, &mut messages),
        );
        let texts: Vec<(String, egui::Rect)> = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => Some((
                    text.galley.text().to_owned(),
                    egui::Rect::from_min_size(text.pos, text.galley.size()),
                )),
                _ => None,
            })
            .collect();
        output.drop_without_applying_deltas();
        (texts, messages)
    };

    // One button holds the whole link, whatever styles its text has.
    let (texts, _) = frame(Reading::default(), Vec::new());
    let [(text, button)] = &texts[..] else {
        panic!("expected one button: {texts:?}");
    };
    assert_eq!(text, "Buy now");
    let press = |pressed| egui::Event::PointerButton {
        pos: button.center(),
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::default(),
    };
    let _ = frame(
        Reading::default(),
        vec![egui::Event::PointerMoved(button.center()), press(true)],
    );
    let (_, messages) = frame(Reading::default(), vec![press(false)]);
    assert!(
        matches!(&messages[..], [Message::LinkClicked(link)] if link == "https://shop.example/"),
        "a click goes through the prompt that says where it leads"
    );

    // Without the sender's styles it is the link it always was.
    let (texts, _) = frame(
        Reading {
            sender_text_styles: false,
            ..Reading::default()
        },
        Vec::new(),
    );
    let drawn: Vec<&str> = texts.iter().map(|(text, _)| text.as_str()).collect();
    assert_eq!(drawn, ["Buy ", "now"]);
}

#[test]
fn a_link_is_underlined_at_rest_and_one_drawn_as_a_button_is_not() {
    let link = |text: &str, button: bool| RichSpan {
        text: text.into(),
        link: Some("https://example.test/".into()),
        button,
        ..RichSpan::default()
    };
    let plain = RichSpan {
        text: "Read ".into(),
        ..RichSpan::default()
    };
    let spans = [plain, link("the report", false), link("Buy", true)];
    let context = egui::Context::default();
    theme::install(&context);
    let output = context.run_ui(egui::RawInput::default(), |ui| {
        rich_spans(ui, &spans, None, Reading::default(), &mut Vec::new());
    });
    let underlined: Vec<(String, bool)> = output
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) => Some((
                text.galley.text().to_owned(),
                text.galley
                    .job
                    .sections
                    .iter()
                    .all(|section| section.format.underline != egui::Stroke::NONE),
            )),
            _ => None,
        })
        .collect();
    output.drop_without_applying_deltas();
    assert_eq!(
        underlined,
        [
            ("Read ".to_owned(), false),
            ("the report".to_owned(), true),
            ("Buy".to_owned(), false)
        ]
    );
}

#[test]
fn a_links_menu_opens_it_through_the_prompt_or_copies_its_address() {
    let span = RichSpan {
        text: "the agenda".into(),
        link: Some("https://example.com/agenda".into()),
        ..RichSpan::default()
    };
    let context = egui::Context::default();
    theme::install(&context);
    let frame = |events: Vec<egui::Event>| {
        let mut messages = Vec::new();
        let output = context.run_ui(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ui| {
                rich_spans(
                    ui,
                    std::slice::from_ref(&span),
                    None,
                    Reading::default(),
                    &mut messages,
                );
            },
        );
        let shapes = output.shapes.clone();
        output.drop_without_applying_deltas();
        (shapes, messages)
    };

    let (shapes, _) = frame(Vec::new());
    let link = text_rect(&shapes, "the agenda");
    let (_, asked) = frame(click_events(link.center(), egui::PointerButton::Secondary));
    assert!(asked.is_empty(), "a right click follows nothing");
    let mut shapes = Vec::new();
    for _ in 0..3 {
        shapes = frame(Vec::new()).0;
    }
    text_rect(&shapes, "Open link…");
    let copy = text_rect(&shapes, "Copy link");
    let (_, asked) = frame(click_events(copy.center(), egui::PointerButton::Primary));

    assert!(
        matches!(
            asked.as_slice(),
            [Message::CopyLinkAddress(address)] if address == "https://example.com/agenda"
        ),
        "Copy link should ask for this link's address"
    );
}

#[test]
fn a_headers_menu_copies_the_senders_or_the_recipients_addresses() {
    let mut message = header_message(Some("Alex Rivera"));
    message.cc = vec![address(Some("Ops"), "ops@example.org")];
    message.bcc = vec![address(None, "audit@example.org")];
    let mut reader = ConversationReader::new(ConversationDetail {
        id: "conversation".into(),
        subject: None,
        labels: Vec::new(),
        messages: vec![message],
    });
    reader.toggle("message");
    let context = egui::Context::default();
    theme::install(&context);
    let frame = |events: Vec<egui::Event>| {
        let mut messages = Vec::new();
        let output = context.run_ui(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ui| {
                ui.set_width(420.0);
                message_card(
                    ui,
                    &reader.detail().messages[0],
                    reader.preview_at(0),
                    &reader,
                    AttachmentSaves::default(),
                    Reading::default(),
                    &mut messages,
                );
            },
        );
        let shapes = output.shapes.clone();
        output.drop_without_applying_deltas();
        (shapes, messages)
    };
    let choose = |item: &str| {
        let (shapes, _) = frame(Vec::new());
        let header = text_rect(&shapes, "Preview");
        let (_, asked) = frame(click_events(
            header.center(),
            egui::PointerButton::Secondary,
        ));
        assert!(asked.is_empty(), "a right click leaves the card as it is");
        let mut shapes = Vec::new();
        for _ in 0..3 {
            shapes = frame(Vec::new()).0;
        }
        let item = text_rect(&shapes, item);
        frame(click_events(item.center(), egui::PointerButton::Primary)).1
    };

    assert!(matches!(
        choose("Copy sender's address").as_slice(),
        [Message::CopyAddresses(addresses)] if addresses == &["alex@example.com"]
    ));
    assert!(matches!(
        choose("Copy recipients' addresses").as_slice(),
        [Message::CopyAddresses(addresses)]
            if addresses == &["team@example.org", "ops@example.org", "audit@example.org"]
    ));
}

/// Every icon painted, as the rectangles their textures fill.
fn icons(shapes: &[egui::epaint::ClippedShape]) -> Vec<egui::Rect> {
    shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::epaint::Shape::Rect(rect) if rect.brush.is_some() => Some(rect.rect),
            _ => None,
        })
        .collect()
}

/// The Labels button's tag: the last icon of the toolbar's first row.
fn labels_button(shapes: &[egui::epaint::ClippedShape]) -> egui::Rect {
    let icons = icons(shapes);
    let row = icons
        .iter()
        .map(|icon| icon.top())
        .fold(f32::INFINITY, f32::min);
    icons
        .into_iter()
        .filter(|icon| (icon.top() - row).abs() <= 0.5)
        .max_by(|a, b| a.left().total_cmp(&b.left()))
        .expect("the toolbar shows its icons")
}

#[test]
fn the_labels_menu_opens_from_its_tag_and_lists_a_box_and_a_name() {
    let places = [Folder::label("lW", "Work"), Folder::label("lF", "Family")];
    let (mut mailbox, request, mut detail) = loading_reader("Message", Folder::INBOX, 1);
    detail.labels = vec!["lW".into()];
    mailbox.finish_conversation(&request, Ok(detail));
    let context = egui::Context::default();
    theme::install(&context);
    context.enable_accesskit();
    let mut state = UiState::default();
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(540.0, 950.0),
        )),
        ..Default::default()
    };
    let mut frame = |input: egui::RawInput| {
        reader_frame(
            &context,
            &mailbox,
            &places,
            Reading::default(),
            &mut state,
            input,
        )
    };

    let (shapes, _) = frame(input.clone());
    let tag = labels_button(&shapes);
    let closed = icons(&shapes).len();
    // The conversation carries a label, so the tag is filled, like the star.
    assert!(
        shapes.iter().any(|shape| matches!(&shape.shape,
                egui::epaint::Shape::Rect(rect)
                    if rect.fill == theme::ACCENT && rect.rect.contains(tag.center()))),
        "a labelled conversation shows a filled tag"
    );
    assert!(
        !shapes.iter().any(|shape| matches!(&shape.shape,
                egui::epaint::Shape::Text(text) if text.galley.text().starts_with("Labels"))),
        "the button shows its tag, not a title"
    );
    frame(click_at(&input, tag.center()));
    for _ in 0..30 {
        frame(input.clone());
    }
    let (shapes, _) = frame(input.clone());

    // Each label is a box and its name: the menu adds no icon of its own.
    assert_eq!(icons(&shapes).len(), closed);
    let work = text_rect(&shapes, "Work");
    let family = text_rect(&shapes, "Family");
    assert!((work.left() - family.left()).abs() <= 0.5);
    assert!(
        work.left() - tag.left() < 60.0,
        "nothing wide before the names"
    );
}

#[test]
fn a_narrow_reader_keeps_its_buttons_and_body_inside_the_pane() {
    let places = [
        Folder::custom("kZ9", "Invoices"),
        Folder::label("lW", "Work"),
    ];
    let (mut mailbox, request, detail) = loading_reader("Message", Folder::INBOX, 1);
    mailbox.finish_conversation(&request, Ok(detail));
    let context = egui::Context::default();
    theme::install(&context);
    let mut state = UiState::default();
    // The reader at its narrowest: a 200 pt pane without its padding.
    let pane = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(168.0, 900.0));
    let input = egui::RawInput {
        screen_rect: Some(pane),
        ..Default::default()
    };
    let mut shapes = Vec::new();
    for _ in 0..3 {
        shapes = reader_frame(
            &context,
            &mailbox,
            &places,
            Reading::default(),
            &mut state,
            input.clone(),
        )
        .0;
    }

    for shape in &shapes {
        if let egui::epaint::Shape::Text(text) = &shape.shape {
            let rect = egui::Rect::from_min_size(text.pos, text.galley.size());
            assert!(
                rect.right() <= pane.right() + 0.5,
                "{:?} reaches {} in a {} pt pane",
                text.galley.text(),
                rect.right(),
                pane.width()
            );
            if text.galley.text() == "Move to…" {
                assert_eq!(text.galley.rows.len(), 1, "Move to… wrapped");
            }
        }
    }
    text_rect(&shapes, "Forward");
}

#[test]
fn opening_a_long_thread_brings_its_first_unread_message_into_view() {
    let land = |landing: bool| {
        let (mut mailbox, request, mut detail) = loading_reader("A long thread", Folder::INBOX, 8);
        for (index, message) in detail.messages.iter_mut().enumerate() {
            message.sender = address(Some(&format!("Person {index}")), "p@example.com");
        }
        detail.messages[3].unread = true;
        mailbox.finish_conversation(&request, Ok(detail));
        let context = egui::Context::default();
        theme::install(&context);
        let mut state = UiState {
            land_reader: landing,
            ..UiState::default()
        };
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(540.0, 500.0),
            )),
            ..Default::default()
        };
        let mut shapes = Vec::new();
        for _ in 0..3 {
            shapes = reader_frame(
                &context,
                &mailbox,
                &[],
                Reading::default(),
                &mut state,
                input.clone(),
            )
            .0;
        }
        assert!(!state.land_reader, "landing happens once");
        text_rect(&shapes, "Person 3").top()
    };

    assert!(land(false) > 500.0 / 3.0, "the case needs a long thread");
    assert!(land(true) < 60.0, "the unread message starts at the top");
}

fn address(name: Option<&str>, email: &str) -> MailAddress {
    MailAddress {
        name: name.map(str::to_owned),
        address: email.to_owned(),
    }
}

#[test]
fn message_times_are_complete() {
    let at = |d, h, min| {
        date(2026, 9, d)
            .at(h, min, 0, 0)
            .to_zoned_datetime(TimeZone::UTC)
            .unwrap()
            .timestamp()
            .as_second()
    };
    assert_eq!(
        format_message_time(at(11, 9, 42), &TimeZone::UTC),
        "Sep 11, 2026 at 09:42"
    );
    // A day of one digit is written as one, in the reader's own zone.
    let east = jiff::tz::offset(2).to_time_zone();
    assert_eq!(
        format_message_time(at(8, 23, 30), &east),
        "Sep 9, 2026 at 01:30"
    );
    // A time no date can hold shows nothing.
    assert_eq!(format_message_time(i64::MAX, &TimeZone::UTC), "");
}

#[test]
fn addresses_fall_back_safely() {
    assert_eq!(
        address_label(&address(Some("Alex Rivera"), "alex@example.com")),
        "Alex Rivera <alex@example.com>"
    );
    assert_eq!(
        address_label(&address(None, "team@example.org")),
        "team@example.org"
    );
    assert_eq!(
        address_label(&address(Some(""), "team@example.org")),
        "team@example.org"
    );
    assert_eq!(address_label(&address(Some("Alex"), "")), "Alex");
    assert_eq!(address_label(&MailAddress::default()), "(unknown)");
}

#[test]
fn recipients_keep_every_address() {
    let recipients: Vec<_> = (0..12)
        .map(|index| address(None, &format!("person{index}@example.com")))
        .collect();
    let label = recipients_label(&recipients);
    for recipient in &recipients {
        assert!(label.contains(&recipient.address));
    }
    assert_eq!(recipients_label(&[]), "(no recipients)");
}

#[test]
fn sizes_read_at_a_glance() {
    assert_eq!(size_label(0), "0 bytes");
    assert_eq!(size_label(1), "1 byte");
    assert_eq!(size_label(1_023), "1023 bytes");
    assert_eq!(size_label(1_024), "1.0 KB");
    assert_eq!(size_label(20 * 1_024), "20 KB");
    assert_eq!(size_label(1_024 * 1_024), "1.0 MB");
    assert_eq!(size_label(15 * 1_024 * 1_024), "15 MB");
    assert_eq!(size_label(3 * 1_024 * 1_024 * 1_024), "3.0 GB");
}

#[test]
fn message_counts_are_pluralized() {
    assert_eq!(message_count_label(0), "No messages");
    assert_eq!(message_count_label(1), "1 message");
    assert_eq!(message_count_label(7), "7 messages");
}

#[test]
fn loading_message_card_fills_the_reading_column_and_shows_body_lines() {
    let context = egui::Context::default();
    let mut rect = egui::Rect::NOTHING;

    context
        .run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(READING_WIDTH, 900.0),
                )),
                ..Default::default()
            },
            |ui| {
                ui.set_width(READING_WIDTH);
                rect = skeleton_message_card(ui, Color32::GRAY, true).rect;
            },
        )
        .drop_without_applying_deltas();

    assert!((rect.width() - READING_WIDTH).abs() < 0.1);
    assert!(
        rect.height() > 100.0,
        "loading message card lost its expanded body: {rect:?}"
    );
}

fn loading_reader(
    subject: &str,
    folder: Folder,
    count: u32,
) -> (Mailbox, crate::app::ReaderRequest, ConversationDetail) {
    let summary = ConversationSummary {
        id: "conversation".into(),
        kind: crate::mail::SummaryKind::Conversation,
        subject: Some(subject.into()),
        correspondents: None,
        participants: Vec::new(),
        preview: None,
        time: None,
        unread: false,
        starred: false,
        message_count: count,
        has_attachments: false,
    };
    let (mut mailbox, page) = Mailbox::open(folder, 50, 1, 2);
    mailbox.finish_page(
        page.id,
        Ok(crate::mail::ConversationPage {
            conversations: vec![summary.clone()],
            total: 1,
            inspect_candidates: Vec::new(),
        }),
    );
    let request = mailbox
        .start_conversation_load(summary.id.clone(), 3)
        .unwrap();
    let detail = ConversationDetail {
        id: summary.id,
        subject: summary.subject,
        labels: Vec::new(),
        messages: (0..count)
            .map(|index| MailMessage {
                id: format!("message-{index}"),
                verdict: Verdict::Unverified,
                sender: address(Some("Alex Rivera"), "alex@example.com"),
                to: vec![address(None, "team@example.org")],
                cc: Vec::new(),
                bcc: Vec::new(),
                reply_to: Vec::new(),
                reply_all_cc: Vec::new(),
                reply_redirected: false,
                time: Some(1_791_327_600 + i64::from(index)),
                unread: false,
                // The expanded body overflows the viewport, exercising the
                // floating scrollbar without predicting its eventual height.
                body: MessageBody::PlainText(if index == count - 1 {
                    format!("Hello.\n{}", "More message content.\n".repeat(60))
                } else {
                    "Hello.".into()
                }),
                attachments: Vec::new(),
            })
            .collect(),
    };
    (mailbox, request, detail)
}

fn reader_frame(
    context: &egui::Context,
    mailbox: &Mailbox,
    places: &[Folder],
    reading: Reading,
    state: &mut UiState,
    input: egui::RawInput,
) -> (Vec<egui::epaint::ClippedShape>, Vec<Message>) {
    let mut messages = Vec::new();
    let output = context.run_ui(input, |ui| {
        let view = ReaderView {
            mailbox,
            places,
            actions_available: true,
            saves: AttachmentSaves::default(),
            reading,
        };
        show(ui, &view, state, &mut messages);
    });
    let shapes = output.shapes.clone();
    output.drop_without_applying_deltas();
    (shapes, messages)
}

/// The buttons drawn: rectangles with a widget's corners.
fn buttons(shapes: &[egui::epaint::ClippedShape]) -> Vec<egui::Rect> {
    rounded_rects(shapes, theme::WIDGET_CORNER_RADIUS)
}

/// The cards drawn, such as a message's.
fn cards(shapes: &[egui::epaint::ClippedShape]) -> Vec<egui::Rect> {
    rounded_rects(shapes, theme::CARD_CORNER_RADIUS)
}

/// The bars of a loading skeleton drawn; a checkbox drawn too would count.
fn skeleton_bars(shapes: &[egui::epaint::ClippedShape]) -> Vec<egui::Rect> {
    rounded_rects(shapes, theme::SKELETON_CORNER_RADIUS)
}

fn rounded_rects(shapes: &[egui::epaint::ClippedShape], radius: u8) -> Vec<egui::Rect> {
    shapes
        .iter()
        .filter_map(|shape| {
            if let egui::epaint::Shape::Rect(rect) = &shape.shape
                && rect.corner_radius == egui::CornerRadius::same(radius)
            {
                Some(rect.rect)
            } else {
                None
            }
        })
        .collect()
}

fn assert_same_pixel_position(label: &str, before: egui::Pos2, after: egui::Pos2, scale: f32) {
    let delta = (after - before).abs() * scale;
    assert!(
        delta.x <= 0.5 && delta.y <= 0.5,
        "{label} moved by {delta:?} pixels: {before:?} -> {after:?}, scale={scale}"
    );
}

fn assert_reader_geometry(
    loading: &[egui::epaint::ClippedShape],
    loaded: &[egui::epaint::ClippedShape],
    subject: &str,
    count: u32,
    expanded: bool,
    scale: f32,
) {
    let before = cards(loading)[0];
    let after = cards(loaded)[0];
    assert_same_pixel_position("card top", before.left_top(), after.left_top(), scale);
    assert_same_pixel_position("card right", before.right_top(), after.right_top(), scale);
    for label in [subject, &message_count_label(count as usize)] {
        let before = text_rect(loading, label);
        let after = text_rect(loaded, label);
        assert_same_pixel_position(label, before.left_top(), after.left_top(), scale);
        assert_same_pixel_position(label, before.right_bottom(), after.right_bottom(), scale);
    }
    let controls = |shapes: &[egui::epaint::ClippedShape], top: f32| {
        buttons(shapes)
            .into_iter()
            .filter(|rect| rect.bottom() < top)
            .collect::<Vec<_>>()
    };
    let before_controls = controls(loading, before.top());
    let after_controls = controls(loaded, after.top());
    assert_eq!(before_controls.len(), after_controls.len());
    assert!(before_controls.len() >= 5);
    for (before, after) in before_controls.iter().zip(after_controls) {
        assert_same_pixel_position("toolbar", before.left_top(), after.left_top(), scale);
        assert_same_pixel_position(
            "toolbar",
            before.right_bottom(),
            after.right_bottom(),
            scale,
        );
    }
    // Each loaded text has a loading bar whose corner is where the text
    // goes, whatever order the bars are drawn in.
    let bars = skeleton_bars(loading);
    let bar_at = |label: &str, corner: fn(&egui::Rect) -> egui::Pos2, point: egui::Pos2| {
        *bars
            .iter()
            .find(|bar| {
                let delta = (corner(bar) - point).abs() * scale;
                delta.x <= 0.5 && delta.y <= 0.5
            })
            .unwrap_or_else(|| panic!("no bar for {label} at {point:?}, scale={scale}: {bars:?}"))
    };
    let sender = text_rect(loaded, "Alex Rivera");
    let sender_bar = bar_at("sender", egui::Rect::left_top, sender.left_top());
    assert!((sender_bar.height() - sender.height()).abs() * scale <= 0.5);
    let date = text_rect(
        loaded,
        &format_message_time(1_791_327_600, &TimeZone::system()),
    );
    bar_at("date", egui::Rect::right_top, date.right_top());
    if expanded {
        for label in ["alex@example.com", "To: team@example.org"] {
            bar_at(
                label,
                egui::Rect::left_top,
                text_rect(loaded, label).left_top(),
            );
        }
        for label in ["Reply", "Reply all", "Forward"] {
            assert_same_pixel_position(
                label,
                text_rect(loading, label).left_top(),
                text_rect(loaded, label).left_top(),
                scale,
            );
        }
        let body = loaded
            .iter()
            .find_map(|shape| {
                if let egui::epaint::Shape::Text(text) = &shape.shape
                    && text.galley.text().starts_with("Hello.")
                {
                    Some(text.pos)
                } else {
                    None
                }
            })
            .unwrap();
        bar_at("body start", egui::Rect::left_top, body);
    } else {
        bar_at(
            "preview",
            egui::Rect::left_top,
            text_rect(loaded, "Hello.").left_top(),
        );
    }
}

#[test]
fn loading_and_loaded_readers_keep_the_same_positions_at_every_scale() {
    use super::super::fonts::Typeface;

    for (appearance, typeface) in [
        (crate::settings::Appearance::Dark, Typeface::Bundled),
        (crate::settings::Appearance::Light, Typeface::Bundled),
        (crate::settings::Appearance::Dark, Typeface::System),
        (crate::settings::Appearance::Light, Typeface::System),
    ] {
        for (width, zoom, native_scale) in [
            (540.0, 1.0, 1.5),
            (980.0, 1.0, 2.0),
            (300.0, 1.0, 1.0),
            (300.0, 1.75, 2.0),
        ] {
            let context = egui::Context::default();
            theme::install(&context);
            super::super::fonts::apply(&context, typeface);
            theme::apply(&context, appearance);
            context.set_zoom_factor(zoom);
            for subject in [
                "Artículo faltante en pedido",
                "A long subject that wraps across several lines in a narrow reading column instead of shifting the toolbar after loading",
            ] {
                for folder in [Folder::INBOX, Folder::System(MailFolder::Trash)] {
                    for (count, expand_all_messages) in [(1, false), (3, false), (3, true)] {
                        let (mut mailbox, request, detail) =
                            loading_reader(subject, folder.clone(), count);
                        // Isolate persisted layout/scroll state while
                        // reusing expensive font resources at this scale.
                        context.data_mut(|data| data.clear());
                        let reading = Reading {
                            expand_all_messages,
                            ..Default::default()
                        };
                        let mut state = UiState {
                            scroll_reader_top: true,
                            ..Default::default()
                        };
                        let mut input = egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width, 950.0),
                            )),
                            ..Default::default()
                        };
                        input
                            .viewports
                            .get_mut(&egui::ViewportId::ROOT)
                            .unwrap()
                            .native_pixels_per_point = Some(native_scale);
                        let places = [Folder::label("work", "Work")];
                        let render = |mailbox: &Mailbox, state: &mut UiState| {
                            let (shapes, messages) = reader_frame(
                                &context,
                                mailbox,
                                &places,
                                reading,
                                state,
                                input.clone(),
                            );
                            assert!(messages.is_empty());
                            shapes
                        };
                        // Apply the pending zoom/native scale before measuring
                        // the transition, as in an already-open desktop window.
                        for _ in 0..2 {
                            render(&mailbox, &mut state);
                        }
                        let loading = render(&mailbox, &mut state);
                        assert!(
                            !state.scroll_reader_top,
                            "loading must consume the scroll reset"
                        );
                        mailbox.finish_conversation(&request, Ok(detail));
                        // Check the very first loaded frame and settled frames.
                        for _ in 0..3 {
                            let loaded = render(&mailbox, &mut state);
                            assert_reader_geometry(
                                &loading,
                                &loaded,
                                subject,
                                count,
                                expand_all_messages || count == 1,
                                context.pixels_per_point(),
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn applied_labels_do_not_push_the_card_down_after_loading() {
    for width in (300..=500).step_by(10) {
        let (mut mailbox, request, mut detail) = loading_reader("Message", Folder::INBOX, 1);
        detail.labels = vec!["work".into()];
        let context = egui::Context::default();
        theme::install(&context);
        let mut state = UiState::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(width as f32, 950.0),
            )),
            ..Default::default()
        };
        let places = [Folder::label("work", "Work")];
        let (loading, _) = reader_frame(
            &context,
            &mailbox,
            &places,
            Reading::default(),
            &mut state,
            input.clone(),
        );
        mailbox.finish_conversation(&request, Ok(detail));
        let (loaded, _) = reader_frame(
            &context,
            &mailbox,
            &places,
            Reading::default(),
            &mut state,
            input,
        );
        assert_same_pixel_position(
            "card with applied labels",
            cards(&loading)[0].left_top(),
            cards(&loaded)[0].left_top(),
            context.pixels_per_point(),
        );
    }
}

#[test]
fn loading_controls_do_not_dispatch_actions_and_loaded_reply_still_works() {
    let (mut mailbox, request, detail) = loading_reader("Message", Folder::INBOX, 1);
    let context = egui::Context::default();
    theme::install(&context);
    let mut state = UiState::default();
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(540.0, 950.0),
        )),
        ..Default::default()
    };
    let places = [Folder::label("work", "Work")];
    let (shapes, _) = reader_frame(
        &context,
        &mailbox,
        &places,
        Reading::default(),
        &mut state,
        input.clone(),
    );
    let reply = text_rect(&shapes, "Reply").center();
    let controls = buttons(&shapes);
    for position in [controls[0].center(), labels_button(&shapes).center(), reply] {
        let click = click_at(&input, position);
        let (_, messages) = reader_frame(
            &context,
            &mailbox,
            &places,
            Reading::default(),
            &mut state,
            click,
        );
        assert!(messages.is_empty(), "loading control dispatched an action");
    }
    mailbox.finish_conversation(&request, Ok(detail));
    reader_frame(
        &context,
        &mailbox,
        &places,
        Reading::default(),
        &mut state,
        input.clone(),
    );
    let click = click_at(&input, reply);
    let (_, messages) = reader_frame(
        &context,
        &mailbox,
        &places,
        Reading::default(),
        &mut state,
        click,
    );
    assert!(
        matches!(messages.as_slice(), [Message::Answer { message_id, kind: AnswerKind::Reply }] if message_id == "message-0")
    );
}

#[test]
fn an_unreadable_message_shows_a_notice_and_can_still_be_answered() {
    let (mut mailbox, request, mut detail) = loading_reader("Message", Folder::INBOX, 1);
    detail.messages[0].body = MessageBody::Unreadable;
    mailbox.finish_conversation(&request, Ok(detail));
    let context = egui::Context::default();
    theme::install(&context);
    let mut state = UiState::default();
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(540.0, 950.0),
        )),
        ..Default::default()
    };

    let (shapes, _) = reader_frame(
        &context,
        &mailbox,
        &[],
        Reading::default(),
        &mut state,
        input.clone(),
    );
    text_rect(&shapes, UNREADABLE_NOTICE);
    let reply = text_rect(&shapes, "Reply").center();
    let click = click_at(&input, reply);
    let (_, messages) = reader_frame(
        &context,
        &mailbox,
        &[],
        Reading::default(),
        &mut state,
        click,
    );
    assert!(
        matches!(messages.as_slice(), [Message::Answer { message_id, kind: AnswerKind::Reply }] if message_id == "message-0")
    );
}

#[test]
fn a_collapsed_card_shows_the_real_address_beside_the_lock() {
    let message = MailMessage {
        id: "message".into(),
        verdict: Verdict::Verified,
        sender: address(Some("PayPal <service@paypal.com>"), "phish@proton.me"),
        to: Vec::new(),
        cc: Vec::new(),
        bcc: Vec::new(),
        reply_to: Vec::new(),
        reply_all_cc: Vec::new(),
        reply_redirected: false,
        time: None,
        unread: false,
        body: MessageBody::PlainText("Preview".into()),
        attachments: Vec::new(),
    };
    let context = egui::Context::default();
    theme::install(&context);
    context.enable_accesskit();
    let output = context.run_ui(egui::RawInput::default(), |ui| {
        ui.set_width(420.0);
        message_header(ui, &message, "Preview", false);
    });
    let texts: Vec<String> = output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::epaint::Shape::Text(text) => Some(text.galley.text().to_owned()),
            _ => None,
        })
        .collect();
    let labels: Vec<String> = output
        .platform_output
        .accesskit_update
        .iter()
        .flat_map(|update| &update.nodes)
        .filter_map(|(_, node)| node.label().map(str::to_owned))
        .collect();
    output.drop_without_applying_deltas();

    // A signature vouches for the address, never for the name it chose.
    assert!(
        texts.iter().any(|text| text.contains("phish@proton.me")),
        "collapsed card hides the address: {texts:?}"
    );
    assert!(
        labels.iter().any(|label| label.contains("phish@proton.me")),
        "screen readers hear only the name: {labels:?}"
    );
}

/// Draws one message header `width` wide and answers with its rectangle
/// and its shapes.
fn header_shapes(
    message: &MailMessage,
    expanded: bool,
    width: f32,
) -> (egui::Rect, Vec<egui::epaint::ClippedShape>) {
    header_shapes_at(message, expanded, width, 1.0)
}

/// [`header_shapes`] with text drawn at `scale`.
fn header_shapes_at(
    message: &MailMessage,
    expanded: bool,
    width: f32,
    scale: f32,
) -> (egui::Rect, Vec<egui::epaint::ClippedShape>) {
    let context = egui::Context::default();
    theme::install(&context);
    theme::set_text_scale(&context, scale);
    let mut rect = egui::Rect::NOTHING;
    let output = context.run_ui(egui::RawInput::default(), |ui| {
        ui.set_width(width);
        rect = message_header(ui, message, "Preview", expanded).rect;
    });
    let shapes = output.shapes.clone();
    output.drop_without_applying_deltas();
    (rect, shapes)
}

fn header_message(name: Option<&str>) -> MailMessage {
    MailMessage {
        id: "message".into(),
        verdict: Verdict::Unverified,
        sender: address(name, "alex@example.com"),
        to: vec![address(None, "team@example.org")],
        cc: Vec::new(),
        bcc: Vec::new(),
        reply_to: Vec::new(),
        reply_all_cc: Vec::new(),
        reply_redirected: false,
        time: Some(1_791_327_600),
        unread: false,
        body: MessageBody::PlainText("Preview".into()),
        attachments: Vec::new(),
    }
}

#[test]
fn a_message_header_keeps_the_arrow_pointer() {
    let message = header_message(Some("Alex Rivera"));
    let context = egui::Context::default();
    theme::install(&context);
    let render = |input| {
        let mut rect = egui::Rect::NOTHING;
        let output = context.run_ui(input, |ui| {
            ui.set_width(420.0);
            rect = message_header(ui, &message, "Preview", false).rect;
        });
        let cursor = output.platform_output.cursor_icon;
        output.drop_without_applying_deltas();
        (rect, cursor)
    };

    let (rect, _) = render(egui::RawInput::default());
    let hover = || egui::RawInput {
        events: vec![egui::Event::PointerMoved(rect.center())],
        ..Default::default()
    };
    render(hover());
    assert_eq!(render(hover()).1, egui::CursorIcon::Default);
}

#[test]
fn a_header_grows_with_its_text_and_keeps_its_lines_apart() {
    let mut message = header_message(Some("Alex Rivera"));
    message.cc = vec![address(None, "copy@example.org")];
    for scale in crate::settings::TEXT_SCALES {
        for expanded in [false, true] {
            let (usual, _) = header_shapes_at(&message, expanded, 420.0, 1.0);
            let (header, shapes) = header_shapes_at(&message, expanded, 420.0, scale);
            // egui keeps lengths on a grid of 1/32 pt.
            assert!(
                (header.height() - usual.height() * scale).abs() < 0.05,
                "{scale}: {} pt tall, {} usually",
                header.height(),
                usual.height()
            );
            let labels: &[&str] = if expanded {
                &[
                    "Alex Rivera",
                    "alex@example.com",
                    "To: team@example.org",
                    "Cc: copy@example.org",
                ]
            } else {
                &["Alex Rivera", "Preview"]
            };
            let lines: Vec<egui::Rect> = labels
                .iter()
                .map(|label| text_rect(&shapes, label))
                .collect();
            assert!(header.top() <= lines[0].top(), "{scale}: {lines:?}");
            assert!(
                lines[lines.len() - 1].bottom() <= header.bottom(),
                "{scale}: {lines:?} past {header:?}"
            );
            for pair in lines.windows(2) {
                assert!(pair[0].bottom() <= pair[1].top(), "{scale}: {pair:?}");
            }
        }
    }
}

#[test]
fn an_open_header_gives_cc_and_bcc_lines_of_their_own() {
    let mut message = header_message(Some("Alex Rivera"));
    message.cc = vec![address(None, "copy@example.org")];
    message.bcc = vec![address(None, "hidden@example.net")];
    let (open, shapes) = header_shapes(&message, true, 420.0);
    let (plain, _) = header_shapes(&header_message(Some("Alex Rivera")), true, 420.0);

    // Only the people it was sent to are under To.
    let to = text_rect(&shapes, "To: team@example.org");
    let cc = text_rect(&shapes, "Cc: copy@example.org");
    let bcc = text_rect(&shapes, "Bcc: hidden@example.net");
    for (above, below) in [(to, cc), (cc, bcc)] {
        let gap = below.top() - above.bottom();
        assert!(
            (0.0..=6.0).contains(&gap),
            "{gap} pt between recipient lines"
        );
    }
    assert!(
        bcc.bottom() <= open.bottom() && open.bottom() - bcc.bottom() <= 14.0,
        "the header ends {} pt below its last line",
        open.bottom() - bcc.bottom()
    );
    assert_eq!(open.height() - plain.height(), 2.0 * RECIPIENT_LINE_PITCH);
}

#[test]
fn an_open_header_leaves_no_gap_where_a_sender_has_no_name() {
    for (name, address_line) in [(Some("Alex Rivera"), true), (None, false)] {
        let (rect, shapes) = header_shapes(&header_message(name), true, 420.0);
        let sender = text_rect(&shapes, name.unwrap_or("alex@example.com"));
        let recipients = text_rect(&shapes, "To: team@example.org");
        let above = if address_line {
            text_rect(&shapes, "alex@example.com")
        } else {
            sender
        };

        let gap = recipients.top() - above.bottom();
        assert!(
            (0.0..=6.0).contains(&gap),
            "{name:?}: {gap} pt between the recipients and the line above"
        );
        assert!(
            rect.bottom() - recipients.bottom() <= 14.0,
            "{name:?}: the header keeps blank space below its last line"
        );
    }
}

#[test]
fn the_date_stops_short_of_the_sender_in_a_narrow_reader() {
    for width in [140.0, 200.0, 420.0] {
        let message = header_message(Some("Alexandra Rivera-Montgomery"));
        let (rect, shapes) = header_shapes(&message, false, width);
        let date = shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text)
                    if text.galley.text().starts_with("Oct")
                        || text.galley.text().starts_with("Sep") =>
                {
                    Some(egui::Rect::from_min_size(text.pos, text.galley.size()))
                }
                _ => None,
            })
            .expect("the header should show its date");
        let sender = shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text) if text.galley.text().starts_with("Alex") => {
                    Some(egui::Rect::from_min_size(text.pos, text.galley.size()))
                }
                _ => None,
            })
            .expect("the header should show its sender");

        assert!(
            sender.right() <= date.left(),
            "{width}: sender {sender:?} runs under the date {date:?}"
        );
        assert!((date.right() - rect.right()).abs() <= 0.5, "{width}");
    }
}

/// What screen readers hear for the header of `message`, drawn in a
/// reader 420 points wide, and where the header lands.
fn heard_header(message: &MailMessage, expanded: bool) -> (egui::Rect, String) {
    let context = egui::Context::default();
    theme::install(&context);
    context.enable_accesskit();
    let mut rect = egui::Rect::NOTHING;
    let output = context.run_ui(egui::RawInput::default(), |ui| {
        ui.set_width(420.0);
        rect = message_header(ui, message, "Preview", expanded).rect;
    });
    let status = message_status_text(message);
    let heard = output
        .platform_output
        .accesskit_update
        .iter()
        .flat_map(|update| &update.nodes)
        .filter_map(|(_, node)| node.label().map(str::to_owned))
        .find(|label| label.ends_with(status))
        .unwrap_or_default();
    output.drop_without_applying_deltas();
    (rect, heard)
}

/// The status the header of `message` names, as its icon's tooltip.
fn message_status_text(message: &MailMessage) -> &'static str {
    match message.verdict {
        Verdict::Verified => "Message text verified",
        Verdict::Unsigned => "Message text not signed",
        Verdict::Unverified => "Could not verify the message text",
        Verdict::Invalid => "Message verification failed. Use caution.",
    }
}

/// The texts drawn while the pointer rests at `position` over the
/// header of `message`, tooltips included.
fn texts_on_hover(message: &MailMessage, expanded: bool, position: egui::Pos2) -> Vec<String> {
    let context = egui::Context::default();
    theme::install(&context);
    context.style_mut_of(context.theme(), |style| {
        style.interaction.tooltip_delay = 0.0;
        style.interaction.show_tooltips_only_when_still = false;
    });
    let mut shapes = Vec::new();
    // A tooltip shows once the pointer has rested for a few passes.
    for _ in 0..10 {
        let output = context.run_ui(
            egui::RawInput {
                events: vec![egui::Event::PointerMoved(position)],
                ..Default::default()
            },
            |ui| {
                ui.set_width(420.0);
                message_header(ui, message, "Preview", expanded);
            },
        );
        shapes = output.shapes.clone();
        output.drop_without_applying_deltas();
    }
    shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::epaint::Shape::Text(text) => Some(text.galley.text().to_owned()),
            _ => None,
        })
        .collect()
}

#[test]
fn each_sender_line_has_its_height_spoken_label_and_tooltips() {
    let named = address(Some("Alex Rivera"), "alex@example.com");
    let unnamed = address(None, "alex@example.com");
    let without_address = address(Some("Alex Rivera"), "");
    for (case, sender, expanded, height, identity, icon_drop) in [
        (
            "collapsed with a name",
            &named,
            false,
            44.0,
            "Alex Rivera <alex@example.com>",
            0.0,
        ),
        (
            "collapsed without a name",
            &unnamed,
            false,
            44.0,
            "alex@example.com",
            0.0,
        ),
        // Open, the address gets a line of its own, with the icon.
        (
            "open with a name",
            &named,
            true,
            66.0,
            "Alex Rivera <alex@example.com>",
            20.0,
        ),
        (
            "open without a name",
            &unnamed,
            true,
            47.0,
            "alex@example.com",
            0.0,
        ),
        // A name with no address is drawn alone, as an address would be.
        (
            "open with a name but no address",
            &without_address,
            true,
            47.0,
            "Alex Rivera",
            0.0,
        ),
    ] {
        let message = MailMessage {
            id: "message".into(),
            verdict: Verdict::Unverified,
            sender: sender.clone(),
            to: Vec::new(),
            cc: Vec::new(),
            bcc: Vec::new(),
            reply_to: Vec::new(),
            reply_all_cc: Vec::new(),
            reply_redirected: false,
            time: None,
            unread: false,
            body: MessageBody::PlainText("Preview".into()),
            attachments: Vec::new(),
        };
        let status = message_status_text(&message);

        let (rect, heard) = heard_header(&message, expanded);
        assert_eq!(rect.height(), height, "{case}");
        assert_eq!(heard, format!("{identity}, {status}"), "{case}");

        let icon = rect.left_top()
            + egui::vec2(HEADER_TEXT_INSET, icon_drop)
            + egui::Vec2::splat(SIGNATURE_ICON_SIZE / 2.0);
        let over_icon = texts_on_hover(&message, expanded, icon);
        assert!(
            over_icon.iter().any(|text| text == status),
            "{case}: the icon explains the verdict: {over_icon:?}"
        );
        let toggle = if expanded { "Collapse" } else { "Expand" };
        let chevron = egui::pos2(rect.left() + 8.0, rect.top() + 11.0);
        let over_chevron = texts_on_hover(&message, expanded, chevron);
        assert!(
            over_chevron.iter().any(|text| text == toggle),
            "{case}: the chevron says it will {toggle}: {over_chevron:?}"
        );
    }
}

#[test]
fn the_reader_actions_say_which_move_is_risky_and_which_flags_are_on() {
    let inbox_moves: Vec<(&str, Folder, bool)> = moves(Some(&INBOX))
        .into_iter()
        .map(|button| (button.label, button.destination, button.risky))
        .collect();
    assert_eq!(
        inbox_moves,
        [
            ("Archive", Folder::System(MailFolder::Archive), false),
            ("Move to Spam", Folder::System(MailFolder::Spam), false),
            (
                "Delete (move to Trash)",
                Folder::System(MailFolder::Trash),
                true
            ),
        ]
    );

    let row = ConversationSummary {
        id: "conversation".into(),
        kind: crate::mail::SummaryKind::Conversation,
        subject: None,
        correspondents: None,
        participants: Vec::new(),
        preview: None,
        time: None,
        unread: true,
        starred: true,
        message_count: 1,
        has_attachments: false,
    };
    let row_flags: Vec<(&str, bool, MailAction)> = flags(row.unread, row.starred)
        .into_iter()
        .map(|button| (button.label, button.on, button.action))
        .collect();
    assert_eq!(
        row_flags,
        [
            ("Mark as read", true, MailAction::SetUnread(false)),
            ("Remove star", true, MailAction::SetStarred(false)),
        ]
    );
}

#[test]
fn message_header_is_one_full_width_click_target() {
    let message = MailMessage {
        id: "message".into(),
        verdict: Verdict::Unverified,
        sender: address(Some("Alex Rivera"), "alex@example.com"),
        to: Vec::new(),
        cc: Vec::new(),
        bcc: Vec::new(),
        reply_to: Vec::new(),
        reply_all_cc: Vec::new(),
        reply_redirected: false,
        time: None,
        unread: false,
        body: MessageBody::PlainText("Preview".into()),
        attachments: Vec::new(),
    };
    let context = egui::Context::default();
    theme::install(&context);
    context.style_mut_of(context.theme(), |style| {
        style.interaction.tooltip_delay = 0.0;
        style.interaction.show_tooltips_only_when_still = false;
    });
    let render = |input| {
        let mut result = (egui::Rect::NOTHING, false);
        let output = context.run_ui(input, |ui| {
            ui.set_width(320.0);
            let response = message_header(ui, &message, "Preview", false);
            result = (response.rect, response.clicked());
        });
        let shapes = output.shapes.clone();
        output.drop_without_applying_deltas();
        (result.0, result.1, shapes)
    };

    let (rect, _, _) = render(egui::RawInput::default());
    assert_eq!(rect.width(), 320.0);
    let icon_center = rect.left_top()
        + egui::vec2(HEADER_TEXT_INSET, 0.0)
        + egui::Vec2::splat(SIGNATURE_ICON_SIZE / 2.0);
    let chevron_center = egui::pos2(rect.left() + 8.0, rect.top() + 11.0);
    for position in [icon_center, rect.center(), chevron_center] {
        // egui holds tooltips back for a moment after a click, and the
        // previous position ended with one.
        let mut shapes = Vec::new();
        for _ in 0..10 {
            shapes = render(egui::RawInput {
                events: vec![egui::Event::PointerMoved(position)],
                ..Default::default()
            })
            .2;
        }
        let labels: Vec<_> = shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text) => Some(text.galley.text()),
                _ => None,
            })
            .collect();
        if position == icon_center {
            assert!(
                labels.contains(&"Could not verify the message text"),
                "hovering the icon must explain the verdict: {labels:?}"
            );
        }
        assert_eq!(
            labels.contains(&"Expand"),
            position == chevron_center,
            "only the chevron should explain the toggle: {labels:?}"
        );
        let pointer = |pressed| egui::Event::PointerButton {
            pos: position,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        render(egui::RawInput {
            events: vec![egui::Event::PointerMoved(position), pointer(true)],
            ..Default::default()
        });
        let (_, clicked, _) = render(egui::RawInput {
            events: vec![egui::Event::PointerMoved(position), pointer(false)],
            ..Default::default()
        });

        assert!(clicked);
    }
}

#[test]
fn an_expanded_message_card_only_uses_its_content_height() {
    let detail = ConversationDetail {
        id: "conversation".into(),
        subject: Some("Short message".into()),
        labels: Vec::new(),
        messages: vec![MailMessage {
            id: "message".into(),
            verdict: Verdict::Unverified,
            sender: address(Some("Alex Rivera"), "alex@example.com"),
            to: vec![address(None, "team@example.org")],
            cc: Vec::new(),
            bcc: Vec::new(),
            reply_to: Vec::new(),
            reply_all_cc: Vec::new(),
            reply_redirected: false,
            time: None,
            unread: false,
            body: MessageBody::PlainText("Hello.".into()),
            attachments: Vec::new(),
        }],
    };
    let reader = ConversationReader::new(detail);
    let context = egui::Context::default();
    let mut height = 0.0;

    context
        .run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(680.0, 900.0),
                )),
                ..Default::default()
            },
            |ui| {
                ui.set_width(680.0);
                let top = ui.cursor().top();
                message_card(
                    ui,
                    &reader.detail().messages[0],
                    reader.preview_at(0),
                    &reader,
                    AttachmentSaves::default(),
                    Reading::default(),
                    &mut Vec::new(),
                );
                height = ui.cursor().top() - top;
            },
        )
        .drop_without_applying_deltas();

    assert!(height < 250.0, "message card stretched to {height} points");
}

#[test]
fn a_deeply_nested_list_item_stays_in_its_column() {
    let height = |depth: u8| {
        let item = RichBlock {
            kind: BlockKind::ListItem {
                marker: ListMarker::Bullet,
                depth,
                spans: vec![RichSpan {
                    text: "Nested item".into(),
                    ..RichSpan::default()
                }],
            },
            quote_depth: 0,
            hidden: false,
            centered: false,
            joined: false,
        };
        let context = egui::Context::default();
        let mut height = 0.0;
        context
            .run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(480.0, 900.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    ui.set_width(480.0);
                    let top = ui.cursor().top();
                    block(
                        ui,
                        &item,
                        no_pictures(),
                        Reading::default(),
                        &mut Vec::new(),
                    );
                    height = ui.cursor().top() - top;
                },
            )
            .drop_without_applying_deltas();
        height
    };

    // Past the indent's limit the text still has the column's width,
    // instead of one glyph per line.
    assert_eq!(height(200), height(1));
}

#[test]
fn disclosure_chevron_changes_direction_without_a_font_glyph() {
    let center = egui::pos2(10.0, 10.0);
    let closed = chevron_points(center, false);
    let open = chevron_points(center, true);

    assert!(closed[1].x > closed[0].x && closed[1].x > closed[2].x);
    assert!(open[1].y > open[0].y && open[1].y > open[2].y);
}

/// A verdict, the icon and status the reader shows for it, and the color
/// of that icon.
type VerdictCase = (
    Verdict,
    theme::Icon,
    &'static str,
    fn(&theme::Colors) -> Color32,
);

fn verdict_cases() -> [VerdictCase; 4] {
    [
        (
            Verdict::Verified,
            theme::Icon::Lock,
            "Message text verified",
            |colors| colors.success,
        ),
        (
            Verdict::Unsigned,
            theme::Icon::Unlock,
            "Message text not signed",
            |colors| colors.muted,
        ),
        (
            Verdict::Unverified,
            theme::Icon::Unlock,
            "Could not verify the message text",
            |colors| colors.muted,
        ),
        (
            Verdict::Invalid,
            theme::Icon::Spam,
            "Message verification failed. Use caution.",
            |colors| colors.danger,
        ),
    ]
}

#[test]
fn each_verdict_has_its_icon_status_and_color() {
    for appearance in [
        crate::settings::Appearance::Dark,
        crate::settings::Appearance::Light,
    ] {
        let context = egui::Context::default();
        theme::install(&context);
        theme::apply(&context, appearance);
        context
            .run_ui(egui::RawInput::default(), |ui| {
                let colors = theme::colors(ui);
                for (verdict, icon, status, color) in verdict_cases() {
                    assert_eq!(
                        signature_status(ui, verdict),
                        (icon, status, color(&colors)),
                        "{verdict:?} {appearance:?}"
                    );
                }
            })
            .drop_without_applying_deltas();
    }
}

/// Draws the one message card of `reader`, twice so its text is laid
/// out, and returns the second pass's shapes and the colors it used.
fn rendered_card(
    context: &egui::Context,
    reader: &ConversationReader,
) -> (Vec<egui::epaint::ClippedShape>, theme::Colors) {
    let mut colors = None;
    let mut draw = || {
        context.run_ui(egui::RawInput::default(), |ui| {
            colors = Some(theme::colors(ui));
            message_card(
                ui,
                &reader.detail().messages[0],
                reader.preview_at(0),
                reader,
                AttachmentSaves::default(),
                Reading::default(),
                &mut Vec::new(),
            );
        })
    };
    draw().drop_without_applying_deltas();
    let output = draw();
    let shapes = output.shapes.clone();
    output.drop_without_applying_deltas();
    (shapes, colors.expect("the card was drawn"))
}

/// The image `shapes` draw `icon` with, at the size of the signature
/// icon.
fn icon_shape<'a>(
    context: &egui::Context,
    shapes: &'a [egui::epaint::ClippedShape],
    icon: theme::Icon,
) -> Option<&'a egui::epaint::RectShape> {
    let texture = egui::Image::new(icon.source())
        .source(context)
        .clone()
        .load(
            context,
            egui::TextureOptions::LINEAR,
            egui::load::SizeHint::Size {
                width: SIGNATURE_ICON_SIZE as u32,
                height: SIGNATURE_ICON_SIZE as u32,
                maintain_aspect_ratio: false,
            },
        )
        .expect("signature SVG must load")
        .texture_id()
        .expect("signature SVG must be ready");
    shapes.iter().find_map(|shape| match &shape.shape {
        egui::epaint::Shape::Rect(rect)
            if rect
                .brush
                .as_ref()
                .is_some_and(|brush| brush.fill_texture_id == texture) =>
        {
            Some(rect)
        }
        _ => None,
    })
}

#[test]
fn body_verdict_is_visible_in_expanded_and_collapsed_message_cards() {
    for (verdict, icon, status, color) in verdict_cases() {
        for appearance in [
            crate::settings::Appearance::Dark,
            crate::settings::Appearance::Light,
        ] {
            for name in [None, Some("Sender")] {
                for expanded in [false, true] {
                    let case =
                        format!("{verdict:?} {appearance:?} name={name:?} expanded={expanded}");
                    // Only the verdict decides the icon: a verified
                    // sender comes from another domain than the rest.
                    let sender = if verdict == Verdict::Verified {
                        "sender@example.org"
                    } else {
                        "sender@proton.me"
                    };
                    let message = MailMessage {
                        id: "message".into(),
                        sender: address(name, sender),
                        to: Vec::new(),
                        cc: Vec::new(),
                        bcc: Vec::new(),
                        reply_to: Vec::new(),
                        reply_all_cc: Vec::new(),
                        reply_redirected: false,
                        time: None,
                        unread: false,
                        body: MessageBody::PlainText("Body".into()),
                        attachments: Vec::new(),
                        verdict,
                    };
                    let mut reader = ConversationReader::new(ConversationDetail {
                        id: "conversation".into(),
                        subject: None,
                        labels: Vec::new(),
                        messages: vec![message],
                    });
                    if !expanded {
                        reader.toggle("message");
                    }
                    assert_eq!(
                        reader.is_expanded(&reader.detail().messages[0], Reading::default()),
                        expanded,
                        "{case}"
                    );
                    let context = egui::Context::default();
                    theme::install(&context);
                    theme::apply(&context, appearance);

                    let (shapes, colors) = rendered_card(&context, &reader);

                    let image = icon_shape(&context, &shapes, icon)
                        .unwrap_or_else(|| panic!("{case}: no icon for {status:?}"));
                    assert_eq!(image.fill, color(&colors), "{case}");
                    let identity = text_rect(
                        &shapes,
                        if expanded && name.is_some() {
                            &reader.detail().messages[0].sender.address
                        } else {
                            reader.detail().messages[0].sender.display_name().unwrap()
                        },
                    );
                    let icon_rect = image.rect;
                    assert!(
                        icon_rect.right() < identity.left(),
                        "{case}: icon overlaps address"
                    );
                    assert!(
                        (icon_rect.top() - identity.top()).abs() <= 0.5,
                        "{case}: icon and sender are not aligned"
                    );
                    assert!(
                        (icon_rect.width() - SIGNATURE_ICON_SIZE).abs() <= 0.5,
                        "{case}: the icon is {} wide",
                        icon_rect.width()
                    );
                    assert!(
                        !shapes.iter().any(|shape| matches!(&shape.shape,
                            egui::epaint::Shape::Text(text) if text.galley.text() == status
                        )),
                        "{case}: status should be in the tooltip, not a separate row"
                    );
                }
            }
        }
    }
}

#[test]
fn a_message_of_images_says_how_many_are_not_shown_rather_than_that_it_has_no_text() {
    let context = egui::Context::default();
    theme::install(&context);
    let drawn = |images: usize, text: &str| {
        let mut message = header_message(Some("Alex Rivera"));
        message.body = MessageBody::Rich(RichBody {
            undescribed_images: images,
            ..ruston_core::html::rich::parse(text)
        });
        let reader = ConversationReader::new(ConversationDetail {
            id: "conversation".into(),
            subject: None,
            labels: Vec::new(),
            messages: vec![message],
        });
        let output = context.run_ui(egui::RawInput::default(), |ui| {
            ui.set_width(680.0);
            message_card(
                ui,
                &reader.detail().messages[0],
                reader.preview_at(0),
                &reader,
                AttachmentSaves {
                    saving: None,
                    failed: None,
                    images: None,
                },
                Reading::default(),
                &mut Vec::new(),
            );
        });
        let shapes = output.shapes.clone();
        output.drop_without_applying_deltas();
        let shows = |text: &str| visible_text_rect(&shapes, text).is_some();
        (
            shows("This message has no text."),
            shows("1 image not shown") || shows(&format!("{images} images not shown")),
        )
    };

    // A newsletter of pictures alone is not an empty message.
    assert_eq!(drawn(3, ""), (false, true));
    assert_eq!(drawn(1, "<p>Sale</p>"), (false, true));
    assert_eq!(drawn(0, ""), (true, false));
}

#[test]
fn a_failed_save_is_told_beside_its_own_file() {
    let mut message = header_message(Some("Alex Rivera"));
    message.attachments = ["report.pdf", "photo.jpg"]
        .into_iter()
        .map(|name| MailAttachment {
            id: name.into(),
            name: name.into(),
            size: 1_024,
            inline: false,
            content_id: None,
        })
        .collect();
    let reader = ConversationReader::new(ConversationDetail {
        id: "conversation".into(),
        subject: None,
        labels: Vec::new(),
        messages: vec![message],
    });
    let context = egui::Context::default();
    theme::install(&context);
    let saves = AttachmentSaves {
        saving: None,
        failed: Some(("photo.jpg", crate::downloads::SaveError::Failed)),
        images: None,
    };
    let output = context.run_ui(egui::RawInput::default(), |ui| {
        ui.set_width(680.0);
        message_card(
            ui,
            &reader.detail().messages[0],
            reader.preview_at(0),
            &reader,
            saves,
            Reading::default(),
            &mut Vec::new(),
        );
    });

    let error = text_rect(&output.shapes, "Ruston Mail could not save the file.");
    let report = text_rect(&output.shapes, "Attachment: report.pdf");
    let photo = text_rect(&output.shapes, "Attachment: photo.jpg");
    assert!(
        photo.bottom() <= error.top() && error.top() - photo.bottom() < 20.0,
        "the error belongs under its file: {photo:?} then {error:?}"
    );
    assert!(report.bottom() <= photo.top(), "not under the other file");
    output.drop_without_applying_deltas();
}

#[test]
fn the_parts_of_the_body_are_offered_after_the_files_attached() {
    let mut message = header_message(Some("Alex Rivera"));
    message.attachments = [("logo.png", true), ("report.pdf", false)]
        .into_iter()
        .map(|(name, inline)| MailAttachment {
            id: name.into(),
            name: name.into(),
            size: 1_024,
            inline,
            content_id: None,
        })
        .collect();
    let reader = ConversationReader::new(ConversationDetail {
        id: "conversation".into(),
        subject: None,
        labels: Vec::new(),
        messages: vec![message],
    });
    let context = egui::Context::default();
    theme::install(&context);
    let output = context.run_ui(egui::RawInput::default(), |ui| {
        ui.set_width(680.0);
        message_card(
            ui,
            &reader.detail().messages[0],
            reader.preview_at(0),
            &reader,
            AttachmentSaves {
                saving: None,
                failed: None,
                images: None,
            },
            Reading::default(),
            &mut Vec::new(),
        );
    });

    let report = text_rect(&output.shapes, "Attachment: report.pdf");
    let logo = text_rect(&output.shapes, "In the message: logo.png");
    assert!(
        report.bottom() <= logo.top(),
        "the files attached come first"
    );
    output.drop_without_applying_deltas();
}

#[test]
fn a_link_shows_where_it_goes_while_the_pointer_rests_on_it() {
    let link = |address: &str| RichSpan {
        text: "Your account".into(),
        link: Some(address.into()),
        ..RichSpan::default()
    };
    let hovered = |span: RichSpan| {
        let context = egui::Context::default();
        theme::install(&context);
        context.style_mut_of(context.theme(), |style| {
            style.interaction.tooltip_delay = 0.0;
            style.interaction.show_tooltips_only_when_still = false;
        });
        let mut texts = Vec::new();
        // A tooltip shows once the pointer has rested for a few passes.
        for _ in 0..10 {
            let output = context.run_ui(
                egui::RawInput {
                    events: vec![egui::Event::PointerMoved(egui::pos2(12.0, 8.0))],
                    ..Default::default()
                },
                |ui| {
                    ui.set_width(420.0);
                    rich_spans(
                        ui,
                        std::slice::from_ref(&span),
                        None,
                        Reading::default(),
                        &mut Vec::new(),
                    );
                },
            );
            texts = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::epaint::Shape::Text(text) => Some(text.galley.text().to_owned()),
                    _ => None,
                })
                .collect();
            output.drop_without_applying_deltas();
        }
        texts
    };

    // As the address normalized it: a host in another script shows in ASCII.
    let texts = hovered(link("https://xn--80ak6aa92e.com/sign-in"));
    assert!(
        texts
            .iter()
            .any(|text| text == "https://xn--80ak6aa92e.com/sign-in"),
        "{texts:?}"
    );
    // A long one is cut short, its host first.
    let long = format!("https://track.example/{}", "a".repeat(500));
    let texts = hovered(link(&long));
    let shown = texts
        .iter()
        .find(|text| text.starts_with("https://track.example/"))
        .unwrap();
    assert_eq!(shown.chars().count(), LINK_HOVER_CHARS + 1, "{shown}");
    assert!(shown.ends_with('…'));
}

/// A long newsletter of `paragraphs`, and the reader showing it.
fn long_message(paragraphs: usize) -> ConversationReader {
    let html: String = (0..paragraphs)
        .map(|n| {
            format!(
                "<p>Paragraph {n} of a long newsletter with <b>bold words</b>, a \
                 <a href='https://example.test/{n}'>link to read more</a> and enough plain \
                 text after it to wrap onto a second line in the reading pane, as most \
                 paragraphs of a newsletter do when they say anything at all.</p>"
            )
        })
        .collect();
    let mut message = header_message(Some("Alex Rivera"));
    message.body = MessageBody::Rich(ruston_core::html::rich::parse(&html));
    ConversationReader::new(ConversationDetail {
        id: "conversation".into(),
        subject: None,
        labels: Vec::new(),
        messages: vec![message],
    })
}

/// One frame of `reader`'s message in a scrolling pane 700 points square,
/// scrolled to `offset`: how long its content is, and what was drawn.
fn long_message_frame(
    context: &egui::Context,
    reader: &ConversationReader,
    offset: f32,
) -> (f32, Vec<egui::epaint::ClippedShape>) {
    let mut length = 0.0;
    let output = context.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(700.0, 700.0),
            )),
            ..Default::default()
        },
        |ui| {
            length = egui::ScrollArea::vertical()
                .vertical_scroll_offset(offset)
                .show(ui, |ui| {
                    message_card(
                        ui,
                        &reader.detail().messages[0],
                        reader.preview_at(0),
                        reader,
                        AttachmentSaves {
                            saving: None,
                            failed: None,
                            images: None,
                        },
                        Reading::default(),
                        &mut Vec::new(),
                    );
                })
                .content_size
                .y;
        },
    );
    let shapes = output.shapes.clone();
    output.drop_without_applying_deltas();
    (length, shapes)
}

#[test]
fn a_long_message_keeps_its_length_while_only_what_is_near_is_laid_out() {
    let reader = long_message(300);
    let settled = |context: &egui::Context, offset: f32| {
        let mut frame = (0.0, Vec::new());
        // The scroll bar and the heights settle over a few frames.
        for _ in 0..12 {
            frame = long_message_frame(context, &reader, offset);
        }
        frame
    };
    let near = egui::Context::default();
    theme::install(&near);
    let (length, _) = settled(&near, 0.0);

    // A screen reader has everything laid out: the length is the same.
    let whole = egui::Context::default();
    theme::install(&whole);
    whole.enable_accesskit();
    let (whole_length, _) = settled(&whole, 0.0);
    assert!(
        (length - whole_length).abs() < 1.0,
        "{length} with only what is near, {whole_length} with all"
    );

    // What was out of sight is laid out when it comes into sight.
    let (_, shapes) = settled(&near, length - 700.0);
    let drawn = |paragraph: &str| {
        shapes.iter().any(|clipped| {
            matches!(&clipped.shape, egui::Shape::Text(text)
            if text.galley.text().starts_with(paragraph)
                && clipped.clip_rect.intersects(egui::Rect::from_min_size(
                    text.pos,
                    text.galley.size(),
                )))
        })
    };
    assert!(drawn("Paragraph 299 "), "the end shows at the end");
    assert!(!drawn("Paragraph 0 "), "the start is far out of sight");
}

#[test]
fn a_block_is_out_of_sight_more_than_a_screen_away() {
    let clip = egui::Rect::from_min_max(egui::pos2(0.0, 1000.0), egui::pos2(700.0, 1700.0));
    // In sight, and within a screen above or below it.
    for top in [1200.0, 400.0, 2350.0] {
        assert!(!out_of_sight(top, 60.0, clip), "{top}");
    }
    for top in [200.0, 2500.0] {
        assert!(out_of_sight(top, 60.0, clip), "{top}");
    }
    // A tall block that reaches into the margin is laid out.
    assert!(!out_of_sight(0.0, 400.0, clip));
}

#[test]
#[ignore = "manual benchmark of drawing long messages, in release"]
fn measure_reader_frames_for_long_messages() {
    for paragraphs in [50, 500, 5000] {
        let reader = long_message(paragraphs);
        let context = egui::Context::default();
        theme::install(&context);
        for _ in 0..40 {
            let _ = long_message_frame(&context, &reader, 0.0);
        }
        let frames = 30;
        let start = std::time::Instant::now();
        for frame in 0..frames {
            let _ = long_message_frame(&context, &reader, frame as f32 * 3.0);
        }
        eprintln!(
            "{paragraphs} paragraphs: {:.2} ms/frame",
            start.elapsed().as_secs_f64() * 1000.0 / f64::from(frames)
        );
    }
}

#[test]
fn a_picture_the_message_carries_is_asked_for_shown_and_let_go() {
    let mut message = header_message(Some("Alex Rivera"));
    message.body = MessageBody::Rich(ruston_core::html::rich::parse(
        "<p>Hi</p><img src='cid:logo@example.com' alt='Logo'><img src='cid:gone'>",
    ));
    message.attachments = vec![MailAttachment {
        id: "logo".into(),
        name: "logo.png".into(),
        size: 2_048,
        inline: true,
        content_id: Some("logo@example.com".into()),
    }];
    let message_id = message.id.clone();
    let reader = ConversationReader::new(ConversationDetail {
        id: "conversation".into(),
        subject: None,
        labels: Vec::new(),
        messages: vec![message],
    });
    let context = egui::Context::default();
    theme::install(&context);
    let frame = |images: &HashMap<String, InlineImage>, reading: Reading| {
        let mut messages = Vec::new();
        let output = context.run_ui(egui::RawInput::default(), |ui| {
            ui.set_width(680.0);
            message_card(
                ui,
                &reader.detail().messages[0],
                reader.preview_at(0),
                &reader,
                AttachmentSaves {
                    images: Some(images),
                    ..AttachmentSaves::default()
                },
                reading,
                &mut messages,
            );
        });
        let shapes = output.shapes.clone();
        output.drop_without_applying_deltas();
        (messages, shapes)
    };
    let says = |shapes: &[egui::epaint::ClippedShape], text: &str| {
        visible_text_rect(shapes, text).is_some()
    };
    let has_texture = || {
        context
            .tex_manager()
            .read()
            .allocated()
            .any(|(_, texture)| texture.name == "message-picture-logo")
    };

    // The reader asks for the part the body names, and says it is coming;
    // the one the message has no part for is one image not shown.
    let none = HashMap::new();
    let (messages, shapes) = frame(&none, Reading::default());
    assert!(
        matches!(&messages[..], [Message::LoadInlineImage(message, attachment)]
            if *message == message_id && attachment == "logo"),
        "{}",
        messages.len()
    );
    assert!(says(&shapes, "Loading picture: Logo"));
    assert!(says(&shapes, "1 image not shown"));

    // Once it is here it is drawn, and nothing is asked for again.
    let shown = HashMap::from([(
        "logo".to_owned(),
        InlineImage::Shown(std::sync::Arc::new(crate::pictures::Picture {
            size: [2, 2],
            rgba: vec![255; 16],
        })),
    )]);
    let (messages, shapes) = frame(&shown, Reading::default());
    assert!(messages.is_empty());
    assert!(!says(&shapes, "Loading picture: Logo") && !says(&shapes, "Image not loaded: Logo"));
    assert!(has_texture());

    // With pictures off in Settings it is its description, as one on the web.
    let (messages, shapes) = frame(
        &none,
        Reading {
            show_inline_images: false,
            ..Reading::default()
        },
    );
    assert!(messages.is_empty());
    assert!(says(&shapes, "Image not loaded: Logo"));
    // The picture left with its conversation, and so does its texture, at
    // the window's next frame.
    forget_textures(&context, &none);
    assert!(!has_texture());
}

#[test]
fn textures_go_with_the_pictures_the_app_no_longer_keeps() {
    let context = egui::Context::default();
    let texture = context.load_texture(
        "message-picture-logo",
        egui::ColorImage::filled([1, 1], egui::Color32::WHITE),
        egui::TextureOptions::LINEAR,
    );
    context.data_mut(|data| {
        data.get_temp_mut_or_default::<Textures>(egui::Id::NULL)
            .0
            .insert("logo".into(), (0, texture));
    });
    let kept = |context: &egui::Context| {
        context.data(|data| {
            data.get_temp::<Textures>(egui::Id::NULL)
                .map_or(0, |all| all.0.len())
        })
    };

    let images = HashMap::from([("logo".to_owned(), InlineImage::Loading)]);
    forget_textures(&context, &images);
    assert_eq!(kept(&context), 1);
    // Nothing open, as after closing the reader or signing out.
    forget_textures(&context, &HashMap::new());
    assert_eq!(kept(&context), 0);
}

#[test]
fn typing_in_another_field_does_not_lay_the_whole_message_out() {
    let context = egui::Context::default();
    theme::install(&context);
    // Whether a message drawn under a field and a button is laid out whole,
    // once the focus has settled where `focus` puts it.
    let whole = |focus: Option<bool>| {
        let mut whole = false;
        for _ in 0..3 {
            context
                .run_ui(egui::RawInput::default(), |ui| {
                    let mut typed = String::new();
                    let field = ui.add(egui::TextEdit::singleline(&mut typed).id_salt("search"));
                    let button = ui.button("Archive");
                    match focus {
                        Some(true) => field.request_focus(),
                        Some(false) => button.request_focus(),
                        None => ui.memory_mut(|memory| memory.surrender_focus(button.id)),
                    }
                    whole = Sight::new(ui, Reading::default()).all;
                })
                .drop_without_applying_deltas();
        }
        whole
    };
    assert!(!whole(None));
    // Focus on a button may move into the message with Tab, which reaches
    // only what is laid out.
    assert!(whole(Some(false)));
    // The cursor in the search field blinks and takes every key: the message
    // behind it stays as it was.
    assert!(!whole(Some(true)));
}

#[test]
fn a_long_table_lays_out_only_the_rows_near_sight_and_keeps_its_shape() {
    let mut html = String::from("<table><tr><th>Item</th><th>Count</th><th>Price</th></tr>");
    for row in 0..400 {
        html.push_str(&format!(
            "<tr><td>item {row}</td><td>{}</td><td>${row}.00</td></tr>",
            row % 7
        ));
    }
    html.push_str("</table>");
    let mut message = header_message(Some("Alex Rivera"));
    message.body = MessageBody::Rich(ruston_core::html::rich::parse(&html));
    let reader = ConversationReader::new(ConversationDetail {
        id: "conversation".into(),
        subject: None,
        labels: Vec::new(),
        messages: vec![message],
    });
    // A frame of the message in a pane 700 points square scrolled to
    // `offset`, with a button below it that has the keyboard's focus when
    // `focused`, which has everything laid out: the message's length, where
    // each cell painted is, how many texts were laid out, and whether egui
    // asked for another frame at once.
    let frame = |context: &egui::Context, offset: f32, reading: Reading, focused: bool| {
        let mut length = 0.0;
        let output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(700.0, 700.0),
                )),
                ..Default::default()
            },
            |ui| {
                length = egui::ScrollArea::vertical()
                    .vertical_scroll_offset(offset)
                    .show(ui, |ui| {
                        message_card(
                            ui,
                            &reader.detail().messages[0],
                            reader.preview_at(0),
                            &reader,
                            AttachmentSaves::default(),
                            reading,
                            &mut Vec::new(),
                        );
                    })
                    .content_size
                    .y;
                let button = ui.button("Archive");
                if focused {
                    button.request_focus();
                }
            },
        );
        let cells: HashMap<String, egui::Pos2> = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) if text.galley.text().starts_with("item ") => {
                    Some((text.galley.text().to_owned(), text.pos))
                }
                _ => None,
            })
            .collect();
        let again = output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .is_some_and(|viewport| viewport.repaint_delay.is_zero());
        output.drop_without_applying_deltas();
        // The texts the frame used, which it laid out or took from the cache.
        let texts = context.fonts(|fonts| fonts.num_galleys_in_cache());
        (length, cells, texts, again)
    };
    // The scroll bars and the table's shape settle over a few frames, a
    // bar's animation over a quarter of a second of them.
    let settled = |context: &egui::Context, offset: f32, reading: Reading, focused: bool| {
        let mut last = frame(context, offset, reading, focused);
        for _ in 0..30 {
            last = frame(context, offset, reading, focused);
        }
        last
    };

    for reading in [
        Reading::default(),
        Reading {
            data_tables: false,
            ..Reading::default()
        },
    ] {
        // With the keyboard's focus on a button every row is laid out, which
        // gives the whole shape.
        let whole = egui::Context::default();
        theme::install(&whole);
        let near = egui::Context::default();
        theme::install(&near);
        let (length, _, _, _) = settled(&whole, 0.0, reading, true);
        let middle = length / 2.0;
        let (whole_length, all, every_text, _) = settled(&whole, middle, reading, true);
        let (near_length, painted, texts, again) = settled(&near, middle, reading, false);

        assert!(painted.contains_key("item 200"), "{reading:?}");
        assert!(
            texts * 3 < every_text,
            "{texts} texts laid out near sight, {every_text} with the whole table"
        );
        assert!(
            (near_length - whole_length).abs() < 1.0,
            "{near_length} with the rows near sight, {whole_length} with all"
        );
        // The rows in sight are where they are with the whole table laid
        // out, its columns as wide.
        assert_eq!(painted.len(), all.len(), "{reading:?}");
        for (cell, at) in &painted {
            let whole_at = all[cell];
            assert!(
                (at.x - whole_at.x).abs() < 0.5 && (at.y - whole_at.y).abs() < 0.5,
                "{cell} at {at:?}, {whole_at:?} with the whole table"
            );
        }
        // Nothing keeps asking for another frame once it has settled.
        assert!(!again, "{reading:?}");
    }
}

/// A row of the Inbox, starred, and unread or not.
fn starred_row(id: &str, unread: bool) -> ConversationSummary {
    ConversationSummary {
        id: id.into(),
        kind: crate::mail::SummaryKind::Conversation,
        subject: Some(id.into()),
        correspondents: None,
        participants: Vec::new(),
        preview: None,
        time: None,
        unread,
        starred: true,
        message_count: 1,
        has_attachments: false,
    }
}

/// The Inbox listing "a", "b" and "c", with "a" and "b" chosen together.
fn two_chosen() -> Mailbox {
    let (mut mailbox, page) = Mailbox::open(Folder::INBOX, 50, 1, 2);
    mailbox.finish_page(
        page.id,
        Ok(crate::mail::ConversationPage {
            conversations: vec![
                starred_row("a", true),
                starred_row("b", false),
                starred_row("c", false),
            ],
            total: 3,
            inspect_candidates: Vec::new(),
        }),
    );
    assert_eq!(mailbox.select_through("a"), Some("a".to_owned()));
    let _ = mailbox.start_conversation_load("a".into(), 3);
    assert_eq!(mailbox.select_through("b"), None);
    mailbox
}

#[test]
fn rows_chosen_together_show_how_many_and_what_can_be_done_to_all() {
    let mailbox = two_chosen();
    let context = egui::Context::default();
    theme::install(&context);
    let mut state = UiState::default();
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(540.0, 950.0),
        )),
        ..Default::default()
    };
    let (shapes, _) = reader_frame(
        &context,
        &mailbox,
        &[],
        Reading::default(),
        &mut state,
        input.clone(),
    );

    assert!(visible_text_rect(&shapes, "2 conversations selected").is_some());
    // Archive, Spam and Trash, then read and star, as for one conversation;
    // the labels menu shows only for one.
    let toolbar = buttons(&shapes);
    assert_eq!(toolbar.len(), 5, "{toolbar:?}");
    let (_, messages) = reader_frame(
        &context,
        &mailbox,
        &[],
        Reading::default(),
        &mut state,
        click_at(&input, toolbar[0].center()),
    );
    assert!(matches!(
        messages.as_slice(),
        [Message::ApplyAction(MailAction::MoveTo(to))]
            if *to == Folder::System(MailFolder::Archive)
    ));
}

#[test]
fn rows_chosen_together_are_read_while_any_is_unread_and_starred_unless_all_are() {
    let switches = |rows: &[ConversationSummary]| -> Vec<(&'static str, bool, MailAction)> {
        let rows: Vec<&ConversationSummary> = rows.iter().collect();
        group_flags(&rows)
            .into_iter()
            .map(|button| (button.label, button.on, button.action))
            .collect()
    };

    assert_eq!(
        switches(&[starred_row("a", false), starred_row("b", true)]),
        [
            ("Mark as read", true, MailAction::SetUnread(false)),
            ("Remove star", true, MailAction::SetStarred(false)),
        ]
    );
    let mut plain = starred_row("b", false);
    plain.starred = false;
    assert_eq!(
        switches(&[starred_row("a", false), plain]),
        [
            ("Mark as unread", false, MailAction::SetUnread(true)),
            ("Add star", false, MailAction::SetStarred(true)),
        ]
    );
}

#[test]
fn the_menu_of_a_row_chosen_with_others_acts_on_all_of_them() {
    let mailbox = two_chosen();
    let context = egui::Context::default();
    theme::install(&context);
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(540.0, 950.0),
        )),
        ..Default::default()
    };
    let frame = |input: egui::RawInput| {
        let mut messages = Vec::new();
        let output = context.run_ui(input, |ui| {
            selection_menu(ui, &mailbox, &mut messages);
        });
        let shapes = output.shapes.clone();
        output.drop_without_applying_deltas();
        (shapes, messages)
    };

    let (shapes, _) = frame(input.clone());
    // Both are starred, so the menu takes the star away from both.
    assert!(visible_text_rect(&shapes, "Remove star").is_some());
    let (_, messages) = frame(click_at(&input, text_rect(&shapes, "Archive").center()));

    assert!(matches!(
        messages.as_slice(),
        [Message::ApplyAction(MailAction::MoveTo(to))]
            if *to == Folder::System(MailFolder::Archive)
    ));
}

#[test]
fn quoted_text_waits_behind_its_button_unless_the_message_is_all_quote() {
    let paragraph = |text: &str, quote_depth: u8| RichBlock {
        kind: BlockKind::Paragraph(vec![RichSpan {
            text: text.into(),
            ..RichSpan::default()
        }]),
        quote_depth,
        hidden: false,
        centered: false,
        joined: false,
    };
    let reader = ConversationReader::new(ConversationDetail {
        id: "conversation".into(),
        subject: None,
        labels: Vec::new(),
        messages: Vec::new(),
    });
    let context = egui::Context::default();
    theme::install(&context);
    // The words drawn, in order.
    let drawn = |blocks: Vec<RichBlock>, show_quoted_text: bool| -> Vec<String> {
        let body = RichBody {
            blocks,
            ..RichBody::default()
        };
        let reading = Reading {
            show_quoted_text,
            ..Reading::default()
        };
        let output = context.run_ui(egui::RawInput::default(), |ui| {
            ui.set_width(600.0);
            rich_body(
                ui,
                &body,
                "message",
                &reader,
                no_pictures(),
                reading,
                &mut Vec::new(),
            );
        });
        let words = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                _ => None,
            })
            .collect();
        output.drop_without_applying_deltas();
        words
    };
    let reply = || {
        vec![
            paragraph("Sounds good", 0),
            paragraph("Shall we meet?", 1),
            paragraph("Before that", 2),
            paragraph("Thanks", 0),
        ]
    };

    assert_eq!(
        drawn(reply(), false),
        ["Sounds good", "Show quoted text", "Thanks"]
    );
    assert_eq!(
        drawn(reply(), true),
        [
            "Sounds good",
            "Hide quoted text",
            "Shall we meet?",
            "Before that",
            "Thanks"
        ]
    );
    // A message that is all quote shows it, with no button before it.
    assert_eq!(
        drawn(vec![paragraph("Shall we meet?", 1)], false),
        ["Shall we meet?"]
    );
}
