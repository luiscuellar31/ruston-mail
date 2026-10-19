//! Tests of the reader pane: its layout, controls and messages.

use chrono::Utc;

use super::*;
use crate::mail::ConversationDetail;
use crate::ui::test_support::text_rect;

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

/// One frame's input with a click at `position`.
fn click_at(input: &egui::RawInput, position: egui::Pos2) -> egui::RawInput {
    let mut click = input.clone();
    click.events = vec![
        egui::Event::PointerMoved(position),
        egui::Event::PointerButton {
            pos: position,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Default::default(),
        },
        egui::Event::PointerButton {
            pos: position,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        },
    ];
    click
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
        let first = rounded_rects(&shapes, 7)[0].center();

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
    let archive = rounded_rects(&shapes, 7)[0].center();

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
            .find_map(|clipped| match &clipped.shape {
                egui::epaint::Shape::Text(text) if text.galley.text() == "Open" => {
                    Some(egui::Rect::from_min_size(text.pos, text.galley.size()))
                }
                _ => None,
            });
        output.drop_without_applying_deltas();
        (open, answer)
    };

    let mut open = None;
    for _ in 0..3 {
        open = frame(Vec::new()).0;
    }
    let open = open.expect("the prompt offers Open");
    assert!(screen.contains_rect(open), "Open is at {open:?}");

    let press = |pressed| egui::Event::PointerButton {
        pos: open.center(),
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::default(),
    };
    let (_, answer) = frame(vec![
        egui::Event::PointerMoved(open.center()),
        press(true),
        press(false),
    ]);
    assert!(matches!(answer, Some(Message::OpenLink)));
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
            |ui| rich_spans(ui, std::slice::from_ref(&span), None, &mut messages),
        );
        let shapes = output.shapes.clone();
        output.drop_without_applying_deltas();
        (shapes, messages)
    };
    let click = |pos, button| {
        let press = |pressed| egui::Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        vec![egui::Event::PointerMoved(pos), press(true), press(false)]
    };

    let (shapes, _) = frame(Vec::new());
    let link = text_rect(&shapes, "the agenda");
    let (_, asked) = frame(click(link.center(), egui::PointerButton::Secondary));
    assert!(asked.is_empty(), "a right click follows nothing");
    let mut shapes = Vec::new();
    for _ in 0..3 {
        shapes = frame(Vec::new()).0;
    }
    text_rect(&shapes, "Open link…");
    let copy = text_rect(&shapes, "Copy link");
    let (_, asked) = frame(click(copy.center(), egui::PointerButton::Primary));

    assert!(
        matches!(
            asked.as_slice(),
            [Message::CopyLinkAddress(address)] if address == "https://example.com/agenda"
        ),
        "Copy link should ask for this link's address"
    );
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
    let timestamp = Utc
        .with_ymd_and_hms(2026, 9, 11, 9, 42, 0)
        .unwrap()
        .timestamp();
    assert_eq!(
        format_message_time(timestamp, &Utc),
        "Sep 11, 2026 at 09:42"
    );
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
    assert_eq!(size_label(0), "0.0 KB");
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
            saving_attachment: None,
            saved_attachment: None,
            reading,
        };
        show(ui, &view, state, &mut messages);
    });
    let shapes = output.shapes.clone();
    output.drop_without_applying_deltas();
    (shapes, messages)
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
    let before = rounded_rects(loading, 10)[0];
    let after = rounded_rects(loaded, 10)[0];
    assert_same_pixel_position("card top", before.left_top(), after.left_top(), scale);
    assert_same_pixel_position("card right", before.right_top(), after.right_top(), scale);
    for label in [subject, &message_count_label(count as usize)] {
        let before = text_rect(loading, label);
        let after = text_rect(loaded, label);
        assert_same_pixel_position(label, before.left_top(), after.left_top(), scale);
        assert_same_pixel_position(label, before.right_bottom(), after.right_bottom(), scale);
    }
    let controls = |shapes: &[egui::epaint::ClippedShape], top: f32| {
        rounded_rects(shapes, 7)
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
    let bars = rounded_rects(loading, 4);
    let sender = text_rect(loaded, "Alex Rivera");
    assert_same_pixel_position("sender", bars[0].left_top(), sender.left_top(), scale);
    assert!((bars[0].height() - sender.height()).abs() * scale <= 0.5);
    let date_index = if expanded { 3 } else { 2 };
    let date = text_rect(loaded, &format_message_time(1_791_327_600, &Local));
    assert_same_pixel_position(
        "date",
        bars[date_index].right_top(),
        date.right_top(),
        scale,
    );
    if expanded {
        for (index, label) in [(1, "alex@example.com"), (2, "To: team@example.org")] {
            let text = text_rect(loaded, label);
            assert_same_pixel_position(label, bars[index].left_top(), text.left_top(), scale);
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
        assert_same_pixel_position("body start", bars[4].left_top(), body, scale);
    } else {
        assert_same_pixel_position(
            "preview",
            bars[1].left_top(),
            text_rect(loaded, "Hello.").left_top(),
            scale,
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
            rounded_rects(&loading, 10)[0].left_top(),
            rounded_rects(&loaded, 10)[0].left_top(),
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
    let controls = rounded_rects(&shapes, 7);
    for position in [controls[0].center(), labels_button(&shapes).center(), reply] {
        let mut click = input.clone();
        click.events = vec![
            egui::Event::PointerMoved(position),
            egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            },
            egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            },
        ];
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
    let mut click = input;
    click.events = vec![
        egui::Event::PointerMoved(reply),
        egui::Event::PointerButton {
            pos: reply,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Default::default(),
        },
        egui::Event::PointerButton {
            pos: reply,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        },
    ];
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
    let mut click = input;
    click.events = vec![
        egui::Event::PointerMoved(reply),
        egui::Event::PointerButton {
            pos: reply,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Default::default(),
        },
        egui::Event::PointerButton {
            pos: reply,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        },
    ];
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
    let context = egui::Context::default();
    theme::install(&context);
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
    let row_flags: Vec<(&str, bool, MailAction)> = flags(&row)
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
                    None,
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
                marker: "•".into(),
                depth,
                spans: vec![RichSpan {
                    text: "Nested item".into(),
                    ..RichSpan::default()
                }],
            },
            quote_depth: 0,
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
                    block(ui, &item, &mut Vec::new());
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
                None,
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
                        reader.is_expanded("message", Reading::default()),
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
fn a_saved_attachment_renders_notice_and_reveal_button() {
    let path = std::path::PathBuf::from("/tmp/report.pdf");
    let context = egui::Context::default();
    let mut messages = Vec::new();

    let (mailbox, _) = Mailbox::open(Folder::INBOX, 50, 1, 2);
    context
        .run_ui(egui::RawInput::default(), |ui| {
            ui.set_width(680.0);
            let view = ReaderView {
                mailbox: &mailbox,
                places: &[],
                actions_available: true,
                saving_attachment: None,
                saved_attachment: Some(Ok(&path)),
                reading: Reading::default(),
            };
            show(ui, &view, &mut UiState::default(), &mut messages);
        })
        .drop_without_applying_deltas();

    assert!(messages.is_empty());
}
