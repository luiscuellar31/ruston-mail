use super::*;

#[test]
fn screen_readers_hear_each_field_by_its_name() {
    let context = egui::Context::default();
    theme::install(&context);
    context.enable_accesskit();
    let (mut app, _) = crate::app::App::boot(true, crate::settings::Settings::default());
    app.update(Message::OpenCompose);
    app.update(Message::ToggleComposeCopies);
    let output = context.run_ui(egui::RawInput::default(), |ui| {
        show(ui, app.compose().unwrap(), &mut Vec::new());
    });
    let names = theme::text_field_names(&output);
    output.drop_without_applying_deltas();

    for name in ["To", "Cc", "Bcc", "Subject", "Message"] {
        assert!(
            names.iter().any(|heard| heard == name),
            "{name} in {names:?}"
        );
    }
}

#[test]
fn a_small_composer_keeps_actions_visible_and_fields_scrollable() {
    let context = egui::Context::default();
    theme::install(&context);
    let (mut app, _) = crate::app::App::boot(true, crate::settings::Settings::default());
    app.update(Message::OpenCompose);
    app.update(Message::ToggleComposeCopies);
    let window = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(410.0, 240.0));
    let mut messages = Vec::new();
    for _ in 0..3 {
        let output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(window),
                ..Default::default()
            },
            |ui| show(ui, app.compose().unwrap(), &mut messages),
        );
        for label in ["Send", "Discard", "Attach…"] {
            let (rect, clip) = output
                .shapes
                .iter()
                .find_map(|clipped| {
                    if let egui::epaint::Shape::Text(text) = &clipped.shape
                        && text.galley.text() == label
                    {
                        return Some((
                            egui::Rect::from_min_size(text.pos, text.galley.size()),
                            clipped.clip_rect,
                        ));
                    }
                    None
                })
                .unwrap();
            assert!(
                window.contains_rect(rect) && clip.contains_rect(rect),
                "{label} is clipped: {rect:?} in {clip:?}"
            );
        }
        output.drop_without_applying_deltas();
    }
    assert!(!messages.iter().any(|message| matches!(
        message,
        Message::Send | Message::CloseCompose | Message::ComposeChanged(_, _)
    )));
}

#[test]
fn the_composer_offers_one_attach_button_and_writes_at_reading_size() {
    let context = egui::Context::default();
    theme::install(&context);
    let (mut app, _) = crate::app::App::boot(true, crate::settings::Settings::default());
    app.update(Message::OpenCompose);
    let draft = app.compose().unwrap().id();
    app.update(Message::AddComposeAttachments(
        draft,
        vec![std::path::PathBuf::from("/nonexistent/notes.txt")],
    ));
    app.update(Message::ComposeChanged(ComposeField::Body, "Hello".into()));
    let output = context.run_ui(egui::RawInput::default(), |ui| {
        show(ui, app.compose().unwrap(), &mut Vec::new());
    });
    let texts: Vec<_> = output
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::epaint::Shape::Text(text) => Some(text.galley.clone()),
            _ => None,
        })
        .collect();
    output.drop_without_applying_deltas();

    let attach: Vec<_> = texts
        .iter()
        .map(|galley| galley.text())
        .filter(|text| text.starts_with("Attach") && *text != "Attachments")
        .collect();
    assert_eq!(attach, ["Attach…"]);
    let body = texts
        .iter()
        .find(|galley| galley.text() == "Hello")
        .expect("the body should be drawn");
    assert_eq!(body.job.sections[0].format.font_id.size, theme::BODY_SIZE);
}

#[test]
fn an_attachment_shows_its_size_once_read_away_from_the_interface() {
    let file =
        std::env::temp_dir().join(format!("ruston-attachment-size-{}.txt", std::process::id()));
    std::fs::write(&file, [b'a'; 2048]).unwrap();
    let context = egui::Context::default();
    theme::install(&context);
    let (mut app, _) = crate::app::App::boot(true, crate::settings::Settings::default());
    app.update(Message::OpenCompose);
    let draft = app.compose().unwrap().id();
    app.update(Message::AddComposeAttachments(draft, vec![file.clone()]));
    let attachment_label = |app: &crate::app::App| {
        let output = context.run_ui(egui::RawInput::default(), |ui| {
            show(ui, app.compose().unwrap(), &mut Vec::new());
        });
        let label = output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::epaint::Shape::Text(text) if text.galley.text().starts_with('📎') => {
                    Some(text.galley.text().to_owned())
                }
                _ => None,
            })
            .expect("the attachment should be listed");
        output.drop_without_applying_deltas();
        label
    };

    // Drawing never asks the file system, which can hang on a share that
    // went away; the size shows once the app has read it.
    let unread = attachment_label(&app);
    std::fs::remove_file(&file).unwrap();
    assert!(!unread.contains('('), "{unread}");
    app.update(Message::AttachmentSizesRead(
        draft,
        [(file, Some(2048))].into_iter().collect(),
    ));
    assert!(attachment_label(&app).ends_with(&format!("({})", reader::size_label(2048))));
}

#[test]
fn undo_in_a_new_draft_never_brings_back_an_earlier_one() {
    let context = egui::Context::default();
    theme::install(&context);
    let (mut app, _) = crate::app::App::boot(true, crate::settings::Settings::default());
    let mut time = 0.0;
    let mut frame = |app: &crate::app::App, events: Vec<egui::Event>| {
        time += 1.5;
        let mut messages = Vec::new();
        context
            .run_ui(
                egui::RawInput {
                    events,
                    time: Some(time),
                    ..Default::default()
                },
                |ui| show(ui, app.compose().unwrap(), &mut messages),
            )
            .drop_without_applying_deltas();
        messages
    };
    let body_id = |app: &crate::app::App| body_id(app.compose().unwrap().id());

    app.update(Message::OpenCompose);
    frame(&app, Vec::new());
    context.memory_mut(|memory| memory.request_focus(body_id(&app)));
    frame(&app, Vec::new());
    for message in frame(&app, vec![egui::Event::Text("private draft".into())]) {
        app.update(message);
    }
    // Pauses let the editor record what was typed as an undo step.
    frame(&app, Vec::new());
    frame(&app, Vec::new());
    app.update(Message::CloseCompose);
    app.update(Message::DiscardCompose);
    assert!(app.compose().is_none());

    app.update(Message::OpenCompose);
    frame(&app, Vec::new());
    context.memory_mut(|memory| memory.request_focus(body_id(&app)));
    frame(&app, Vec::new());
    let undo = egui::Event::Key {
        key: egui::Key::Z,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::COMMAND,
    };
    let messages = frame(&app, vec![undo]);
    assert!(
        !messages
            .iter()
            .any(|message| matches!(message, Message::ComposeChanged(_, text) if text.contains("private draft"))),
        "undo brought back another draft"
    );
}

#[test]
fn forgetting_a_draft_drops_only_its_editor_memory() {
    let context = egui::Context::default();
    remember_draft(&context, 1);
    remember_draft(&context, 2);

    forget_draft(&context, 1);

    assert!(!remembers_draft(&context, 1));
    assert!(remembers_draft(&context, 2));
}

#[test]
fn forgetting_a_draft_drops_what_every_field_it_showed_kept() {
    let context = egui::Context::default();
    theme::install(&context);
    let (mut app, _) = crate::app::App::boot(true, crate::settings::Settings::default());
    app.update(Message::OpenCompose);
    app.update(Message::ToggleComposeCopies);
    let draft = app.compose().unwrap().id();
    context
        .run_ui(egui::RawInput::default(), |ui| {
            show(ui, app.compose().unwrap(), &mut Vec::new());
        })
        .drop_without_applying_deltas();
    let kept = || context.data(|data| data.count::<egui::text_edit::TextEditState>());
    // To, Cc, Bcc, Subject and the body each keep their own memory.
    assert_eq!(kept(), 5);

    forget_draft(&context, draft);
    assert_eq!(kept(), 0, "a field of the draft kept what was typed");
}

#[test]
fn a_reply_lists_its_recipients_and_warns_when_reply_to_redirects_it() {
    let context = egui::Context::default();
    theme::install(&context);
    let answer = Answering {
        sender: "Alex Rivera".into(),
        subject: "Invoice".into(),
        kind: AnswerKind::ReplyAll,
        to: vec!["attacker@evil.example".into()],
        cc: vec!["bob@corp.example".into()],
        redirected_from: Some("alex@corp.example".into()),
        quotes_original: true,
        forwarded: Vec::new(),
    };
    let mut danger = egui::Color32::PLACEHOLDER;
    let mut body = 0.0;
    let output = context.run_ui(egui::RawInput::default(), |ui| {
        danger = theme::colors(ui).danger;
        body = egui::TextStyle::Body.resolve(ui.style()).size;
        answered(ui, &answer, false);
    });
    let texts: Vec<(String, bool)> = output
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::epaint::Shape::Text(text) => Some((
                text.galley.text().to_owned(),
                text.galley.job.sections.iter().all(|section| {
                    section.format.color == danger && section.format.font_id.size == body
                }),
            )),
            _ => None,
        })
        .collect();
    output.drop_without_applying_deltas();

    let shown = |wanted: &str| texts.iter().find(|(text, _)| text.contains(wanted));
    assert!(shown("To: attacker@evil.example").is_some());
    assert!(shown("Cc: bob@corp.example").is_some());
    let warning = shown("not to its sender, alex@corp.example").expect("redirect warning");
    assert!(warning.1, "the redirect warning stands out");
}

#[test]
fn a_forward_names_the_files_it_carries_and_the_quote() {
    let context = egui::Context::default();
    theme::install(&context);
    let answer = Answering {
        sender: "Alex Rivera".into(),
        subject: "Invoice".into(),
        kind: AnswerKind::Forward,
        to: Vec::new(),
        cc: Vec::new(),
        redirected_from: None,
        quotes_original: true,
        forwarded: vec!["invoice.pdf".into(), "receipt.png".into()],
    };
    let output = context.run_ui(egui::RawInput::default(), |ui| {
        answered(ui, &answer, true);
    });
    let texts: Vec<String> = output
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::epaint::Shape::Text(text) => Some(text.galley.text().to_owned()),
            _ => None,
        })
        .collect();
    output.drop_without_applying_deltas();

    for wanted in [
        "The original is quoted under your text.",
        "Its attachments go with it: invoice.pdf, receipt.png.",
    ] {
        assert!(
            texts.iter().any(|text| text == wanted),
            "{wanted} in {texts:?}"
        );
    }
}

#[test]
fn the_composer_names_recipients_whose_copy_is_not_encrypted() {
    use crate::app::Effect;
    use futures::executor::block_on;

    let context = egui::Context::default();
    theme::install(&context);
    let (mut app, _) = crate::app::App::boot(true, crate::settings::Settings::default());
    app.update(Message::OpenCompose);
    app.update(Message::ComposeChanged(
        ComposeField::To,
        "ada@proton.me, plain@example.com".into(),
    ));
    let shown = |app: &crate::app::App| {
        let mut messages = Vec::new();
        let mut danger = egui::Color32::PLACEHOLDER;
        let mut body = 0.0;
        let output = context.run_ui(egui::RawInput::default(), |ui| {
            danger = theme::colors(ui).danger;
            body = egui::TextStyle::Body.resolve(ui.style()).size;
            show(ui, app.compose().unwrap(), &mut messages);
        });
        // The warning is named and drawn in the danger color at full
        // size, not as a field label.
        let found = output.shapes.iter().any(|clipped| {
            matches!(&clipped.shape, egui::epaint::Shape::Text(text)
                if text.galley.text().contains("not end-to-end encrypt this message for: plain@example.com")
                    && text.galley.job.sections.iter().all(|section| section.format.color == danger && section.format.font_id.size == body))
        });
        output.drop_without_applying_deltas();
        found
    };
    assert!(!shown(&app));

    let effect = app
        .update(Message::ComposeRecipientsLeft)
        .into_iter()
        .next()
        .unwrap();
    let Effect::Future(lookup) = effect else {
        panic!("the lookup runs off the UI thread");
    };
    app.update(block_on(lookup));

    assert!(shown(&app));
}

#[test]
fn the_composer_shows_the_ascii_form_of_an_international_domain() {
    let context = egui::Context::default();
    theme::install(&context);
    let shown = |accept_international_domains| {
        let mut settings = crate::settings::Settings::default();
        settings.accept_international_domains = accept_international_domains;
        let (mut app, _) = crate::app::App::boot(true, settings);
        app.update(Message::OpenCompose);
        app.update(Message::ComposeChanged(
            ComposeField::To,
            "ceo@p\u{430}ypal.com".into(),
        ));
        let mut messages = Vec::new();
        let mut danger = egui::Color32::PLACEHOLDER;
        let mut body = 0.0;
        let output = context.run_ui(egui::RawInput::default(), |ui| {
            danger = theme::colors(ui).danger;
            body = egui::TextStyle::Body.resolve(ui.style()).size;
            show(ui, app.compose().unwrap(), &mut messages);
        });
        // The look-alike is named beside a form that is plainly not paypal.com.
        let found = output.shapes.iter().any(|clipped| {
            matches!(&clipped.shape, egui::epaint::Shape::Text(text)
                if text.galley.text().contains("p\u{430}ypal.com is sent as xn--pypal-4ve.com")
                    && text.galley.job.sections.iter().all(|section| section.format.color == danger && section.format.font_id.size == body))
        });
        output.drop_without_applying_deltas();
        found
    };

    assert!(!shown(false));
    assert!(shown(true));
}

#[test]
fn a_new_draft_puts_the_cursor_where_writing_starts() {
    use crate::mail::demo;

    let context = egui::Context::default();
    theme::install(&context);
    let focused_after_showing = |app: &crate::app::App| {
        context
            .run_ui(egui::RawInput::default(), |ui| {
                show(ui, app.compose().unwrap(), &mut Vec::new());
            })
            .drop_without_applying_deltas();
        context.memory(|memory| memory.focused())
    };

    let (mut app, _) = crate::app::App::boot(true, crate::settings::Settings::default());
    app.update(Message::OpenCompose);
    let draft = app.compose().unwrap().id();
    assert_eq!(
        focused_after_showing(&app),
        Some(field_id(ComposeField::To, draft))
    );
    // Given once: after that the cursor stays wherever the writer puts it.
    context.memory_mut(|memory| memory.surrender_focus(field_id(ComposeField::To, draft)));
    assert_eq!(focused_after_showing(&app), None);

    // A reply knows its recipients, so it starts in the body.
    app.update(Message::DiscardCompose);
    let mailbox = demo::DemoMailbox::new();
    let page =
        mailbox.list_conversations(&crate::mail::Folder::INBOX, 0, demo::PAGE_SIZE, demo::now());
    app.update(Message::ConversationsLoaded(1, page));
    app.update(Message::SelectConversation("demo-0".into()));
    let crate::app::ReaderState::Loading {
        conversation_id,
        request,
    } = app.mailbox().unwrap().reader_state()
    else {
        panic!("the conversation is loading");
    };
    let request = crate::app::ReaderRequest {
        id: *request,
        conversation_id: conversation_id.clone(),
        kind: crate::mail::SummaryKind::Conversation,
        messages: 1,
    };
    let detail = mailbox.conversation_detail(&request.conversation_id, demo::now());
    app.update(Message::ConversationLoaded(request, detail));
    app.update(Message::Answer {
        message_id: "demo-0-2".into(),
        kind: AnswerKind::Reply,
    });
    let reply = app.compose().expect("a reply is open").id();
    assert_eq!(focused_after_showing(&app), Some(body_id(reply)));
}

#[test]
fn field_labels_are_drawn_at_body_size() {
    let context = egui::Context::default();
    theme::install(&context);
    let (mut app, _) = crate::app::App::boot(true, crate::settings::Settings::default());
    app.update(Message::OpenCompose);
    let mut messages = Vec::new();
    let mut body = 0.0;
    let output = context.run_ui(egui::RawInput::default(), |ui| {
        body = egui::TextStyle::Body.resolve(ui.style()).size;
        show(ui, app.compose().unwrap(), &mut messages);
    });
    let size_of = |label: &str| {
        output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::epaint::Shape::Text(text) if text.galley.text() == label => text
                    .galley
                    .job
                    .sections
                    .first()
                    .map(|section| section.format.font_id.size),
                _ => None,
            })
    };

    for label in ["To", "Subject"] {
        assert_eq!(size_of(label), Some(body), "{label}");
    }
    output.drop_without_applying_deltas();
}

#[test]
fn leaving_a_recipient_field_asks_how_the_message_will_be_protected() {
    let context = egui::Context::default();
    theme::install(&context);
    let (mut app, _) = crate::app::App::boot(true, crate::settings::Settings::default());
    app.update(Message::OpenCompose);
    let frame = |events: Vec<egui::Event>| {
        let mut messages = Vec::new();
        context
            .run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ui| show(ui, app.compose().unwrap(), &mut messages),
            )
            .drop_without_applying_deltas();
        messages
            .iter()
            .any(|message| matches!(message, Message::ComposeRecipientsLeft))
    };

    frame(Vec::new());
    let to = field_id(ComposeField::To, app.compose().unwrap().id());
    context.memory_mut(|memory| memory.request_focus(to));
    assert!(!frame(Vec::new()), "entering the field looks nothing up");
    assert!(!frame(Vec::new()), "staying in the field looks nothing up");

    let tab = egui::Event::Key {
        key: egui::Key::Tab,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    // Focus moves during the frame that sees the key; the field reports it by the next one.
    let asked = [frame(vec![tab]), frame(Vec::new())];
    assert!(asked.contains(&true));
    assert!(!frame(Vec::new()), "the lookup is asked for once");
}

#[test]
fn compose_fields_start_at_the_left_and_are_taller_than_the_default() {
    let context = egui::Context::default();
    let mut geometry = None;
    context
        .run_ui(egui::RawInput::default(), |ui| {
            ui.set_width(400.0);
            let left = ui.available_rect_before_wrap().left();
            let mut messages = Vec::new();
            let field = line(
                ui,
                "To",
                ComposeField::To,
                &Compose::default(),
                false,
                &mut messages,
            );

            let mut plain = String::new();
            let plain = ui.add(egui::TextEdit::singleline(&mut plain));
            geometry = Some((left, field.rect, plain.rect.height()));
        })
        .drop_without_applying_deltas();

    let (left, field, plain_height) = geometry.expect("the field was laid out");
    assert!((field.left() - left).abs() <= 1.0);
    assert!(field.height() >= plain_height + 8.0);
}
