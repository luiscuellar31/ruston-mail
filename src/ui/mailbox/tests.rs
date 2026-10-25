//! Tests of the mailbox pane: folders, the conversation list, search and the undo notice.

use chrono::Utc;

use super::*;
use crate::mail::{ConversationDetail, ConversationPage, MailboxError, SummaryKind};
use crate::ui::test_support::{text_rect, visible_text_rect};

fn mailbox_frame(
    context: &egui::Context,
    app: &App,
    state: &mut UiState,
    size: egui::Vec2,
    mut draft: Option<&mut Settings>,
    input: egui::RawInput,
) -> (Vec<Message>, Vec<egui::epaint::ClippedShape>) {
    let mut messages = Vec::new();
    let output = context.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            ..input
        },
        |root| show(root, app, draft.as_deref_mut(), state, &mut messages),
    );
    let shapes = output.shapes.clone();
    output.drop_without_applying_deltas();
    (messages, shapes)
}

#[test]
fn screen_readers_hear_the_search_field_by_its_name() {
    let (app, _) = App::boot(true, Settings::default());
    let context = egui::Context::default();
    theme::install(&context);
    context.enable_accesskit();
    let mut state = UiState::default();
    let output = context.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1100.0, 700.0),
            )),
            ..Default::default()
        },
        |root| show(root, &app, None, &mut state, &mut Vec::new()),
    );
    let names = theme::text_field_names(&output);
    output.drop_without_applying_deltas();

    assert!(
        names.iter().any(|heard| heard == "Search mail"),
        "{names:?}"
    );
}

#[test]
fn search_stays_above_visible_conversations_in_wide_and_compact_mailboxes() {
    for (size, zoom) in [
        (egui::vec2(1500.0, 950.0), 1.0),
        (egui::vec2(469.0, 400.0), 1.75),
    ] {
        for query in ["", "p"] {
            let context = egui::Context::default();
            theme::install(&context);
            context.set_zoom_factor(zoom);
            let (mut app, _) = App::boot(true, Settings::default());
            let _ = app.update(Message::ConversationsLoaded(
                1,
                Ok(pagination_page(0, 50, 120)),
            ));
            let _ = app.update(Message::SearchChanged(query.into()));
            assert_eq!(app.mailbox().unwrap().visible_count(), 50);
            let mut state = UiState::default();
            let mut shapes = Vec::new();
            for _ in 0..3 {
                (_, shapes) =
                    mailbox_frame(&context, &app, &mut state, size, None, Default::default());
            }
            let search = visible_text_rect(
                &shapes,
                if query.is_empty() {
                    "Search mail…"
                } else {
                    query
                },
            )
            .expect("search must be visible");
            let refresh = visible_text_rect(&shapes, "Refresh").unwrap();
            assert!(
                search.top() - refresh.bottom() < ROW_HEIGHT,
                "search consumed the pane height: {search:?} in {size:?}"
            );
            let first_row =
                visible_text_rect(&shapes, "p0").expect("loaded mail must be visible below search");
            assert!(first_row.top() > search.bottom());
            assert!(first_row.bottom() <= size.y);
            if !query.is_empty() {
                let clear = visible_text_rect(&shapes, "Clear").expect("Clear must remain visible");
                assert!(clear.center().y < first_row.top());
            }
            let position = first_row.center();
            let (messages, _) = mailbox_frame(
                &context,
                &app,
                &mut state,
                size,
                None,
                egui::RawInput {
                    events: vec![
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
                    ],
                    ..Default::default()
                },
            );
            assert!(
                messages.iter().any(|message| {
                    matches!(message, Message::SelectConversation(id) if id == "p0")
                }),
                "visible mail must remain selectable"
            );
        }
    }
}

#[test]
fn typing_a_search_leaves_the_list_where_it_was() {
    for (size, zoom) in [
        (egui::vec2(1100.0, 700.0), 1.0),
        (egui::vec2(469.0, 400.0), 1.75),
    ] {
        let drawn = |query: &str| {
            let context = egui::Context::default();
            theme::install(&context);
            context.set_zoom_factor(zoom);
            let (mut app, _) = App::boot(true, Settings::default());
            let _ = app.update(Message::ConversationsLoaded(
                1,
                Ok(pagination_page(0, 50, 120)),
            ));
            let _ = app.update(Message::SearchChanged(query.into()));
            let mut state = UiState::default();
            let mut shapes = Vec::new();
            for _ in 0..3 {
                (_, shapes) =
                    mailbox_frame(&context, &app, &mut state, size, None, Default::default());
            }
            shapes
        };

        let first_row = |shapes: &[egui::epaint::ClippedShape]| {
            visible_text_rect(shapes, "p0")
                .expect("the first row is in view")
                .top()
        };
        let searching = drawn("p");
        assert_eq!(first_row(&drawn("")), first_row(&searching), "at {size:?}");
        if zoom == 1.0 {
            let scope = search_scope_label(None, 50);
            let whole = searching.iter().any(|clipped| {
                matches!(&clipped.shape, egui::Shape::Text(text)
                    if text.galley.text() == scope && !text.galley.elided)
            });
            assert!(
                whole,
                "the scope fits its line beside a list of the usual width"
            );
        }
    }
}

#[test]
fn while_signing_out_the_sidebar_says_so_and_cannot_sign_out_again() {
    let context = egui::Context::default();
    theme::install(&context);
    let (mut app, _) = App::boot(true, Settings::default());
    app.set_auth_state(AuthState::SigningOut);
    let mut state = UiState::default();
    let mut messages = Vec::new();
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1500.0, 950.0));

    let mut label = None;
    for _ in 0..3 {
        let output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |root| show(root, &app, None, &mut state, &mut messages),
        );
        label = visible_text_rect(&output.shapes, "Signing out…");
        output.drop_without_applying_deltas();
    }
    let position = label.expect("the sidebar says it is signing out").center();
    context
        .run_ui(
            egui::RawInput {
                screen_rect: Some(screen),
                events: vec![
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
                ],
                ..Default::default()
            },
            |root| show(root, &app, None, &mut state, &mut messages),
        )
        .drop_without_applying_deltas();

    assert!(
        !messages
            .iter()
            .any(|message| matches!(message, Message::Logout)),
        "a sign-out under way must not start another"
    );
}

#[test]
fn folders_scroll_independently_while_account_actions_remain_accessible() {
    let context = egui::Context::default();
    theme::install(&context);
    let (mut app, _) = App::boot(true, Settings::default());
    let _ = app.update(Message::FoldersLoaded(
        1,
        Ok(crate::mail::OwnPlaces {
            folders: (0..30)
                .map(|n| Folder::label(format!("label-{n}"), format!("Label {n}")))
                .collect(),
            ..Default::default()
        }),
    ));
    let size = egui::vec2(410.0, 240.0);
    let mut scroll_id = egui::Id::NULL;
    let mut messages = Vec::new();
    for _ in 0..2 {
        context
            .run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                    ..Default::default()
                },
                |root| {
                    egui::CentralPanel::default().show(root, |ui| {
                        let scroll = sidebar(ui, &app, app.mailbox().unwrap(), &mut messages);
                        scroll_id = scroll.id;
                        assert!(scroll.content_size.y > scroll.inner_rect.height());
                        assert!(scroll.content_size.x <= scroll.inner_rect.width() + 1.0);
                    });
                },
            )
            .drop_without_applying_deltas();
    }
    let mut scroll = egui::scroll_area::State::load(&context, scroll_id).unwrap();
    scroll.offset.y = 100_000.0;
    scroll.store(&context, scroll_id);
    let mut button = None;
    let mut exit_button = None;
    for _ in 0..3 {
        let output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                ..Default::default()
            },
            |root| {
                egui::CentralPanel::default().show(root, |ui| {
                    sidebar(ui, &app, app.mailbox().unwrap(), &mut messages);
                });
            },
        );
        button = visible_text_rect(&output.shapes, "Settings");
        exit_button = visible_text_rect(&output.shapes, "Exit demo");
        output.drop_without_applying_deltas();
    }
    assert!(
        exit_button.is_some(),
        "sign-out must be reachable after scrolling settles"
    );
    let position = button
        .expect("Settings must be reachable at the end of the sidebar")
        .center();
    context
        .run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                events: vec![
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
                ],
                ..Default::default()
            },
            |root| {
                egui::CentralPanel::default().show(root, |ui| {
                    sidebar(ui, &app, app.mailbox().unwrap(), &mut messages);
                });
            },
        )
        .drop_without_applying_deltas();
    assert!(
        messages
            .iter()
            .any(|message| matches!(message, Message::ShowSettings(true)))
    );
}

#[test]
fn an_account_block_that_cannot_fit_stops_asking_for_frames() {
    use std::time::Duration;

    let context = egui::Context::default();
    theme::install(&context);
    let (app, _) = App::boot(true, Settings::default());
    // Lower than the account block, as a short window at a large zoom is.
    let size = egui::vec2(410.0, 60.0);
    let mut delays = Vec::new();
    for second in 0..5 {
        let output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                // Time goes on, so the scroll bar's fade finishes.
                time: Some(f64::from(second)),
                ..Default::default()
            },
            |root| {
                egui::CentralPanel::default().show(root, |ui| {
                    sidebar(ui, &app, app.mailbox().unwrap(), &mut Vec::new());
                });
            },
        );
        delays.push(output.viewport_output[&egui::ViewportId::ROOT].repaint_delay);
        output.drop_without_applying_deltas();
    }
    // The first frames measure the block; once its height holds, nothing
    // is left to draw again.
    assert_eq!(delays.last(), Some(&Duration::MAX), "{delays:?}");
}

#[test]
fn the_accounts_own_folders_and_labels_come_under_headings() {
    let context = egui::Context::default();
    theme::install(&context);
    let draw = |places: Vec<Folder>| {
        let (mut app, _) = App::boot(true, Settings::default());
        let _ = app.update(Message::FoldersLoaded(
            1,
            Ok(crate::mail::OwnPlaces {
                folders: places,
                ..Default::default()
            }),
        ));
        let output = context.run_ui(egui::RawInput::default(), |root| {
            egui::CentralPanel::default().show(root, |ui| {
                sidebar_content(ui, &app, app.mailbox().unwrap(), &mut Vec::new());
            });
        });
        let shapes = output.shapes.clone();
        output.drop_without_applying_deltas();
        shapes
    };

    let shapes = draw(vec![
        Folder::custom("f1", "Invoices"),
        Folder::label("l1", "Travel"),
    ]);
    let order =
        ["Trash", "FOLDERS", "Invoices", "LABELS", "Travel"].map(|text| text_rect(&shapes, text));
    for pair in order.windows(2) {
        assert!(pair[0].bottom() <= pair[1].top(), "{order:?}");
    }

    let shapes = draw(Vec::new());
    assert!(
        visible_text_rect(&shapes, "FOLDERS").is_none(),
        "no heading over nothing"
    );
}

#[test]
fn compact_navigation_and_settings_fit_both_themes_at_high_zoom() {
    for appearance in [
        crate::settings::Appearance::Dark,
        crate::settings::Appearance::Light,
    ] {
        for zoom in [1.5, 1.75, 2.0] {
            let context = egui::Context::default();
            theme::install(&context);
            theme::apply(&context, appearance);
            let mut preferences = Settings::default();
            preferences.zoom = zoom;
            preferences.appearance = appearance;
            let (mut app, _) = App::boot(true, preferences);
            let _ = app.update(Message::SearchChanged("needle".into()));
            context
                .run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(820.0, 480.0),
                        )),
                        ..Default::default()
                    },
                    |_| {},
                )
                .drop_without_applying_deltas();
            context.set_zoom_factor(zoom);
            let size = egui::vec2(820.0, 480.0) / zoom;
            let window = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
            let mut state = UiState::default();
            for view in [CompactView::Mail, CompactView::Folders] {
                state.compact_view = view;
                for frame in 0..3 {
                    let (messages, shapes) =
                        mailbox_frame(&context, &app, &mut state, size, None, Default::default());
                    assert!(
                        !messages
                            .iter()
                            .any(|message| matches!(message, Message::PanelsResized(_)))
                    );
                    if frame < 2 {
                        continue; // Let egui settle the measured bottom panel height.
                    }
                    for label in ["Folders", "Mail", "Reading"] {
                        assert!(
                            window.contains_rect(
                                visible_text_rect(&shapes, label).unwrap_or_else(|| panic!(
                                    "{label} not visible at zoom {zoom}"
                                ))
                            ),
                            "{label} clipped at zoom {zoom}"
                        );
                    }
                    if view == CompactView::Folders {
                        for label in ["Settings", "Exit demo"] {
                            assert!(
                                window.contains_rect(
                                    visible_text_rect(&shapes, label).unwrap_or_else(|| panic!(
                                        "{label} not visible at zoom {zoom}"
                                    ))
                                ),
                                "{label} clipped at zoom {zoom}"
                            );
                        }
                    } else {
                        assert!(
                            window.contains_rect(
                                visible_text_rect(&shapes, "Clear")
                                    .expect("search clear action must fit in the compact pane")
                            )
                        );
                    }
                    assert!(
                        !messages
                            .iter()
                            .any(|message| matches!(message, Message::PanelsResized(_)))
                    );
                }
            }
            let mut draft = app.settings().clone();
            for _ in 0..3 {
                let (_, shapes) = mailbox_frame(
                    &context,
                    &app,
                    &mut state,
                    size,
                    Some(&mut draft),
                    Default::default(),
                );
                for label in ["Back to mail", "Apply"] {
                    assert!(
                        window.contains_rect(visible_text_rect(&shapes, label).unwrap()),
                        "{label} clipped at zoom {zoom}"
                    );
                }
            }
        }
    }
}

#[test]
fn switching_between_compact_and_columns_keeps_the_list_offset_and_split_ratios() {
    let context = egui::Context::default();
    theme::install(&context);
    let (mut app, _) = App::boot(true, Settings::default());
    let _ = app.update(Message::ConversationsLoaded(
        1,
        Ok(pagination_page(0, 50, 120)),
    ));
    let mut state = UiState::default();
    let wide = egui::vec2(1100.0, 700.0);
    for _ in 0..3 {
        mailbox_frame(&context, &app, &mut state, wide, None, Default::default());
    }
    set_list_offset(&context, &state, 500.0);
    mailbox_frame(&context, &app, &mut state, wide, None, Default::default());
    let before = state.conversation_scroll.as_ref().unwrap();
    let (scroll_id, offset) = (before.scroll_id, before.offset);
    let ratios = app.panels();
    let sidebar_before =
        egui::containers::panel::PanelState::load(&context, egui::Id::new(SIDEBAR_PANEL_ID))
            .unwrap()
            .outer_rect
            .width();
    for size in [egui::vec2(410.0, 240.0), wide] {
        for _ in 0..3 {
            mailbox_frame(&context, &app, &mut state, size, None, Default::default());
            let after = state.conversation_scroll.as_ref().unwrap();
            assert_eq!(after.scroll_id, scroll_id);
            assert_eq!(after.offset, offset);
            assert_eq!(app.panels(), ratios);
        }
    }
    let sidebar_after =
        egui::containers::panel::PanelState::load(&context, egui::Id::new(SIDEBAR_PANEL_ID))
            .unwrap()
            .outer_rect
            .width();
    assert_eq!(sidebar_before, sidebar_after);
}

#[test]
fn enter_in_the_search_field_searches_all_mail() {
    let context = egui::Context::default();
    theme::install(&context);
    let (mut app, _) = App::boot(true, Settings::default());
    let _ = app.update(Message::ConversationsLoaded(
        1,
        Ok(pagination_page(0, 5, 5)),
    ));
    let _ = app.update(Message::SearchChanged("invoice".into()));
    let mut state = UiState::default();
    let size = egui::vec2(1100.0, 700.0);
    let enter = || egui::RawInput {
        events: vec![egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
        ..Default::default()
    };
    let submitted = |messages: &[Message]| {
        messages
            .iter()
            .any(|message| matches!(message, Message::SearchSubmitted))
    };

    mailbox_frame(&context, &app, &mut state, size, None, Default::default());
    let (messages, _) = mailbox_frame(&context, &app, &mut state, size, None, enter());
    assert!(
        !submitted(&messages),
        "Enter outside the field is not a search"
    );

    context.memory_mut(|memory| memory.request_focus(egui::Id::new("mail-search")));
    mailbox_frame(&context, &app, &mut state, size, None, Default::default());
    let (messages, _) = mailbox_frame(&context, &app, &mut state, size, None, enter());
    assert!(submitted(&messages), "Enter in the field searches all mail");
}

#[test]
fn high_zoom_uses_the_available_width_without_overwriting_panel_preferences() {
    let context = egui::Context::default();
    theme::install(&context);
    let (app, _) = App::boot(true, Settings::default());
    let mut state = UiState::default();
    let mut messages = Vec::new();
    let window = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(820.0, 480.0) / 1.75);
    for _ in 0..3 {
        messages.clear();
        context
            .run_ui(
                egui::RawInput {
                    screen_rect: Some(window),
                    ..Default::default()
                },
                |root| show(root, &app, None, &mut state, &mut messages),
            )
            .drop_without_applying_deltas();
    }
    assert!(
        egui::containers::panel::PanelState::load(&context, egui::Id::new(CONVERSATION_PANEL_ID))
            .is_none(),
        "three columns must not be forced into a narrow viewport"
    );
    assert!(
        !messages
            .iter()
            .any(|message| matches!(message, Message::PanelsResized(_))),
        "a temporary compact layout must not overwrite saved split ratios"
    );
}

#[test]
fn a_conversation_chosen_while_writing_shows_under_the_folded_draft() {
    let (mut app, _) = App::boot(true, Settings::default());
    let _ = app.update(Message::OpenCompose);
    let _ = app.update(Message::ComposeChanged(
        crate::app::ComposeField::Subject,
        "Plans".into(),
    ));
    let draw = |app: &App| {
        let context = egui::Context::default();
        theme::install(&context);
        let mut state = UiState::default();
        let mut shapes = Vec::new();
        for _ in 0..2 {
            (_, shapes) = mailbox_frame(
                &context,
                app,
                &mut state,
                egui::vec2(1100.0, 700.0),
                None,
                Default::default(),
            );
        }
        shapes
    };
    assert!(visible_text_rect(&draw(&app), "To").is_some(), "writing");

    let _ = app.update(Message::SelectConversation("demo-0".into()));
    let shapes = draw(&app);

    assert!(visible_text_rect(&shapes, "Draft: Plans").is_some());
    assert!(visible_text_rect(&shapes, "Continue writing").is_some());
    assert!(
        visible_text_rect(&shapes, "To").is_none(),
        "the composer is folded away"
    );
}

#[test]
fn one_column_says_why_an_action_failed_over_the_list() {
    let mut mailbox = pagination_mailbox(false);
    let start = mailbox
        .start_action(crate::mail::MailAction::SetStarred(true), 50)
        .unwrap();
    mailbox.finish_action(&start.send.unwrap(), Err(MailboxError::Connection));
    let said = MailboxError::Connection.action_message();
    let (app, _) = App::boot(true, Settings::default());
    let context = egui::Context::default();
    theme::install(&context);

    let output = context.run_ui(egui::RawInput::default(), |ui| {
        ui.set_width(320.0);
        compact_mail(ui, &app, &mailbox, &mut UiState::default(), &mut Vec::new());
    });
    let error = visible_text_rect(&output.shapes, said).expect("the reason is in view");
    let title = text_rect(&output.shapes, "Inbox");
    output.drop_without_applying_deltas();

    assert!(error.bottom() <= title.top(), "{error:?} then {title:?}");
}

#[test]
fn what_the_server_found_is_titled_as_search_results() {
    let context = egui::Context::default();
    theme::install(&context);
    let titled = |search: bool, title: &str| {
        let (texts, _) = pane_frame(&context, &pagination_mailbox(search), None);
        texts.iter().any(|(text, _)| text == title)
    };

    assert!(titled(true, "Search results"));
    assert!(!titled(true, "Inbox"), "the results come from every folder");
    assert!(titled(false, "Inbox"));
    assert!(!titled(false, "Search results"));
}

#[test]
fn a_long_folder_name_is_shortened_beside_refresh_without_widening_the_list() {
    let context = egui::Context::default();
    theme::install(&context);
    let (mut app, _) = App::boot(true, Settings::default());
    let name = "Receipts from every trip we took together across three continents";
    let folder = Folder::custom("trips", name);
    let _ = app.update(Message::FoldersLoaded(
        1,
        Ok(crate::mail::OwnPlaces {
            folders: vec![folder.clone()],
            ..Default::default()
        }),
    ));
    let _ = app.update(Message::SelectFolder(folder));
    let mut state = UiState::default();
    let mut asked = Vec::new();
    let mut shapes = Vec::new();
    for _ in 0..3 {
        let (messages, drawn) = mailbox_frame(
            &context,
            &app,
            &mut state,
            egui::vec2(1100.0, 700.0),
            None,
            egui::RawInput::default(),
        );
        asked.extend(messages);
        shapes = drawn;
    }

    assert!(
        !asked
            .iter()
            .any(|message| matches!(message, Message::PanelsResized(_))),
        "the name must not widen the list"
    );
    let refresh = visible_text_rect(&shapes, "Refresh").expect("Refresh stays in view");
    let title = text_rect(&shapes, name);
    assert!(
        title.right() <= refresh.left(),
        "{title:?} runs into {refresh:?}"
    );
}

fn pagination_row(id: &str, time: i64) -> ConversationSummary {
    ConversationSummary {
        id: id.to_owned(),
        kind: SummaryKind::Conversation,
        time: Some(time),
        subject: Some(id.to_owned()),
        correspondents: Some("Test sender".into()),
        participants: Vec::new(),
        preview: None,
        unread: false,
        starred: false,
        message_count: 1,
        has_attachments: false,
    }
}

fn pagination_page(start: i64, count: i64, total: u32) -> ConversationPage {
    ConversationPage {
        conversations: (start..start + count)
            .map(|n| pagination_row(&format!("p{n}"), 1000 - n))
            .collect(),
        total,
        inspect_candidates: Vec::new(),
    }
}

/// Draws the list pane 320 pt wide, with a click at `click`, and answers
/// with its texts and what it asked for.
fn pane_frame(
    context: &egui::Context,
    mailbox: &Mailbox,
    click: Option<egui::Pos2>,
) -> (Vec<(String, egui::Rect)>, Vec<Message>) {
    let events = click.map_or_else(Vec::new, |pos| {
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        vec![egui::Event::PointerMoved(pos), button(true), button(false)]
    });
    let mut messages = Vec::new();
    let (app, _) = App::boot(true, Settings::default());
    let output = context.run_ui(
        egui::RawInput {
            events,
            ..Default::default()
        },
        |ui| {
            ui.set_width(320.0);
            conversation_pane(ui, &app, mailbox, &mut UiState::default(), &mut messages);
        },
    );
    let texts = output
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            // A centered galley is placed by its middle, so its rows
            // are offset from `pos`.
            egui::epaint::Shape::Text(text) => Some((
                text.galley.text().to_owned(),
                text.galley.rect.translate(text.pos.to_vec2()),
            )),
            _ => None,
        })
        .collect();
    output.drop_without_applying_deltas();
    (texts, messages)
}

fn rect_of(texts: &[(String, egui::Rect)], wanted: &str) -> egui::Rect {
    texts
        .iter()
        .find(|(text, _)| text == wanted)
        .map(|(_, rect)| *rect)
        .unwrap_or_else(|| panic!("missing {wanted}: {texts:?}"))
}

#[test]
fn an_empty_list_centers_its_message_and_what_can_be_done() {
    let (mut mailbox, first) = Mailbox::open(Folder::INBOX, 50, 1, 2);
    mailbox.finish_page(first.id, Err(MailboxError::Connection));
    let context = egui::Context::default();
    theme::install(&context);

    let (texts, _) = pane_frame(&context, &mailbox, None);
    let message = rect_of(&texts, MailboxError::Connection.folder_message());
    let action = rect_of(&texts, "Try again");
    assert!(
        (message.center().x - action.center().x).abs() <= 1.0,
        "{message:?} and {action:?} should share a center"
    );

    let (_, asked) = pane_frame(&context, &mailbox, Some(action.center()));
    assert!(matches!(asked.as_slice(), [Message::RefreshMailbox]));
}

#[test]
fn a_search_that_fails_over_rows_says_so_and_tries_the_search_again() {
    let (mut mailbox, first) = Mailbox::open(Folder::INBOX, 50, 1, 2);
    mailbox.finish_page(first.id, Ok(pagination_page(0, 3, 3)));
    mailbox.set_search_query("p1".into());
    let search = mailbox.start_search(3).unwrap();
    mailbox.finish_search(&search, Err(MailboxError::Service));
    let context = egui::Context::default();
    theme::install(&context);

    let (texts, _) = pane_frame(&context, &mailbox, None);
    rect_of(&texts, "p1");
    let line = rect_of(&texts, "The search failed.");
    let action = rect_of(&texts, "Try again");
    assert!(line.right() < action.left(), "the action follows the line");

    let (_, asked) = pane_frame(&context, &mailbox, Some(action.center()));
    assert!(matches!(asked.as_slice(), [Message::SearchSubmitted]));
}

#[test]
fn the_search_text_starts_where_the_rows_text_does() {
    let (mut mailbox, first) = Mailbox::open(Folder::INBOX, 50, 1, 2);
    mailbox.finish_page(first.id, Ok(pagination_page(0, 3, 3)));
    let context = egui::Context::default();
    theme::install(&context);
    let (app, _) = App::boot(true, Settings::default());
    let output = context.run_ui(egui::RawInput::default(), |ui| {
        ui.set_width(320.0);
        conversation_pane(ui, &app, &mailbox, &mut UiState::default(), &mut Vec::new());
    });
    let left_of = |wanted: &str| {
        output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::epaint::Shape::Text(text) if text.galley.text() == wanted => Some(text.pos.x),
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing {wanted}"))
    };

    let (search, row) = (left_of("Search mail…"), left_of("p0"));
    output.drop_without_applying_deltas();

    assert!(
        (search - row).abs() <= 0.5,
        "search at {search}, rows at {row}"
    );
}

fn pagination_mailbox(search: bool) -> Mailbox {
    let (mut mailbox, first) = Mailbox::open(Folder::INBOX, 50, 1, 2);
    mailbox.finish_page(first.id, Ok(pagination_page(0, 50, 120)));
    if search {
        mailbox.set_search_query("needle".into());
        let request = mailbox.start_search(3).unwrap();
        mailbox.finish_search(&request, Ok(pagination_page(0, 50, 120)));
    }
    let open = mailbox.start_conversation_load("p40".into(), 9).unwrap();
    mailbox.finish_conversation(
        &open,
        Ok(ConversationDetail {
            id: "p40".into(),
            subject: None,
            messages: Vec::new(),
            labels: Vec::new(),
        }),
    );
    mailbox
}

struct ListFrame {
    messages: Vec<Message>,
    button: Option<egui::Pos2>,
    subjects: std::collections::HashMap<String, f32>,
    offset: f32,
}

fn pagination_frame(
    context: &egui::Context,
    mailbox: &Mailbox,
    state: &mut UiState,
    input: egui::RawInput,
) -> ListFrame {
    let mut messages = Vec::new();
    let output = context.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(320.0, 480.0),
            )),
            ..input
        },
        |ui| conversation_list(ui, mailbox, state, &mut messages),
    );
    let mut button = None;
    let mut subjects = std::collections::HashMap::new();
    for clipped in &output.shapes {
        if let egui::epaint::Shape::Text(text) = &clipped.shape {
            if text.galley.text() == "Load more" {
                button = Some(text.pos + text.galley.size() * 0.5);
            }
            subjects.insert(text.galley.text().to_owned(), text.pos.y);
        }
    }
    output.drop_without_applying_deltas();
    ListFrame {
        messages,
        button,
        subjects,
        offset: state.conversation_scroll.as_ref().unwrap().offset,
    }
}

fn pagination_view(mailbox: &Mailbox) -> (egui::Context, UiState, ListFrame) {
    let context = egui::Context::default();
    theme::install(&context);
    context.all_styles_mut(|style| style.scroll_animation = egui::style::ScrollAnimation::none());
    let mut state = UiState::default();
    pagination_frame(&context, mailbox, &mut state, Default::default());
    set_list_offset(&context, &state, 100_000.0);
    pagination_frame(&context, mailbox, &mut state, Default::default());
    let frame = pagination_frame(&context, mailbox, &mut state, Default::default());
    (context, state, frame)
}

fn set_list_offset(context: &egui::Context, state: &UiState, offset: f32) {
    let id = state.conversation_scroll.as_ref().unwrap().scroll_id;
    let mut scroll = egui::scroll_area::State::load(context, id).unwrap();
    scroll.offset.y = offset;
    scroll.store(context, id);
}

#[test]
fn another_folder_or_a_search_opens_at_the_top_of_its_list() {
    for search in [false, true] {
        let mut mailbox = pagination_mailbox(false);
        let (context, mut state, scrolled) = pagination_view(&mailbox);
        assert!(scrolled.offset > 0.0, "the folder was scrolled down");

        if search {
            mailbox.set_search_query("needle".into());
            let request = mailbox.start_search(10).unwrap();
            mailbox.finish_search(&request, Ok(pagination_page(0, 50, 120)));
        } else {
            let request = mailbox
                .select_folder(Folder::System(crate::mail::MailFolder::Archive), 10)
                .unwrap();
            mailbox.finish_page(request.id, Ok(pagination_page(0, 50, 120)));
        }
        let frame = pagination_frame(&context, &mailbox, &mut state, Default::default());
        assert_eq!(frame.offset, 0.0, "search: {search}");
    }
}

#[test]
fn load_more_keeps_reader_and_viewport_while_waiting_and_after_success_or_failure() {
    for search in [false, true] {
        for failed in [false, true] {
            let mut mailbox = pagination_mailbox(search);
            let reader = mailbox.reader_state().clone();
            let (context, mut state, before) = pagination_view(&mailbox);
            let button = before.button.unwrap();
            for pressed in [true, false] {
                let frame = pagination_frame(
                    &context,
                    &mailbox,
                    &mut state,
                    egui::RawInput {
                        events: vec![
                            egui::Event::PointerMoved(button),
                            egui::Event::PointerButton {
                                pos: button,
                                button: egui::PointerButton::Primary,
                                pressed,
                                modifiers: Default::default(),
                            },
                        ],
                        ..Default::default()
                    },
                );
                assert!(
                    frame
                        .messages
                        .iter()
                        .all(|message| matches!(message, Message::LoadMoreConversations))
                );
                if !pressed {
                    assert_eq!(frame.messages.len(), 1);
                }
            }
            let result = if failed {
                Err(MailboxError::Connection)
            } else {
                Ok(pagination_page(50, 50, 100))
            };
            if search {
                let next = mailbox.load_more_search(10).unwrap();
                let waiting = pagination_frame(&context, &mailbox, &mut state, Default::default());
                assert_eq!(waiting.offset, before.offset);
                assert_eq!(mailbox.reader_state(), &reader);
                mailbox.finish_search(&next, result);
            } else {
                let next = mailbox.load_more(10).unwrap();
                let waiting = pagination_frame(&context, &mailbox, &mut state, Default::default());
                assert_eq!(waiting.offset, before.offset);
                assert_eq!(mailbox.reader_state(), &reader);
                mailbox.finish_page(next.id, result);
            }
            let after = pagination_frame(&context, &mailbox, &mut state, Default::default());
            assert_eq!(after.offset, before.offset);
            assert_eq!(after.subjects["p44"], before.subjects["p44"]);
            assert_eq!(mailbox.reader_state(), &reader);
            assert_eq!(mailbox.has_more(), failed);
        }
    }
}

#[test]
fn interleaved_pages_and_refreshes_keep_the_first_visible_row_in_place() {
    for refreshing in [false, true] {
        let mut mailbox = pagination_mailbox(false);
        let (context, mut state, frame) = pagination_view(&mailbox);
        set_list_offset(&context, &state, frame.offset - 17.25);
        let before = pagination_frame(&context, &mailbox, &mut state, Default::default());
        let request = if refreshing {
            mailbox.refresh(10)
        } else {
            mailbox.load_more(10)
        }
        .unwrap();
        let mut page = if refreshing {
            pagination_page(0, 49, 120)
        } else {
            pagination_page(50, 1, 120)
        };
        page.conversations
            .insert(0, pagination_row("overlap", 1001));
        mailbox.finish_page(request.id, Ok(page));
        let after = pagination_frame(&context, &mailbox, &mut state, Default::default());
        assert_eq!(after.offset, before.offset + ROW_PITCH);
        assert_eq!(after.subjects["p44"], before.subjects["p44"]);
        assert_eq!(mailbox.selected_conversation(), Some("p40"));
    }
}

#[test]
fn background_split_keeps_the_next_visible_row_when_the_first_is_replaced() {
    let mut mailbox = pagination_mailbox(false);
    let (context, mut state, before) = pagination_view(&mailbox);
    mailbox.split_conversation(
        &Folder::INBOX,
        "p44",
        vec![
            pagination_row("split-a", 1002),
            pagination_row("split-b", 1001),
        ],
    );
    let after = pagination_frame(&context, &mailbox, &mut state, Default::default());
    assert_eq!(after.subjects["p45"], before.subjects["p45"]);
    assert_eq!(mailbox.selected_conversation(), Some("p40"));
}

#[test]
fn a_keyboard_step_scrolls_the_list_only_as_far_as_the_row() {
    let mailbox = pagination_mailbox(false);
    let context = egui::Context::default();
    theme::install(&context);
    context.all_styles_mut(|style| style.scroll_animation = egui::style::ScrollAnimation::none());
    let mut state = UiState::default();
    // The scroll lands on the pass after the request, and the rows are
    // drawn where it left them on the one after that.
    let mut reveal = |row: &str| {
        state.reveal_conversation = Some(row.into());
        for _ in 0..2 {
            pagination_frame(&context, &mailbox, &mut state, Default::default());
        }
        pagination_frame(&context, &mailbox, &mut state, Default::default())
    };

    // A row already in view leaves the list where it is.
    assert_eq!(reveal("p5").offset, 0.0);
    // One below the view comes up to its bottom edge, not to the middle.
    let below = reveal("p9");
    assert!(below.offset > 0.0);
    assert!(
        below.subjects["p9"] > 480.0 * 0.75,
        "p9 was scrolled to {}",
        below.subjects["p9"]
    );
}

#[test]
fn explicit_navigation_takes_precedence_over_the_saved_viewport() {
    let mut mailbox = pagination_mailbox(false);
    let (context, mut state, _) = pagination_view(&mailbox);
    let next = mailbox.load_more(10).unwrap();
    mailbox.finish_page(
        next.id,
        Ok(ConversationPage {
            conversations: vec![pagination_row("overlap", 1001)],
            total: 120,
            inspect_candidates: Vec::new(),
        }),
    );
    state.reveal_conversation = Some("overlap".into());
    pagination_frame(&context, &mailbox, &mut state, Default::default());
    let after = pagination_frame(&context, &mailbox, &mut state, Default::default());
    assert_eq!(after.offset, 0.0);
    assert!(state.reveal_conversation.is_none());
}

#[test]
fn saved_viewport_does_not_apply_to_another_folder_or_search() {
    let mut mailbox = pagination_mailbox(false);
    let (context, state, _) = pagination_view(&mailbox);
    let saved = state.conversation_scroll.unwrap();
    mailbox.set_search_query("p4".into());
    assert!(
        saved
            .offset_after_update(&mailbox, saved.scroll_id)
            .is_none()
    );
    mailbox.set_search_query(String::new());
    mailbox.select_folder(Folder::System(MailFolder::Sent), 10);
    assert!(
        saved
            .offset_after_update(&mailbox, saved.scroll_id)
            .is_none()
    );
    drop(context);
}

fn inspected_page_members(parent: i64) -> Vec<ConversationSummary> {
    [0, 1]
        .map(|offset| ConversationSummary {
            kind: SummaryKind::Message,
            ..pagination_row(&format!("m{parent}-{offset}"), 1000 - parent - offset)
        })
        .into_iter()
        .collect()
}

#[test]
fn startup_inspection_keeps_an_untouched_list_at_the_top() {
    let mut mailbox = pagination_mailbox(false);
    let context = egui::Context::default();
    theme::install(&context);
    let mut state = UiState::default();
    let before = pagination_frame(&context, &mailbox, &mut state, Default::default());
    mailbox.split_conversation(&Folder::INBOX, "p0", inspected_page_members(0));
    let after = pagination_frame(&context, &mailbox, &mut state, Default::default());
    assert_eq!(before.offset, 0.0);
    assert_eq!(after.offset, 0.0);
    assert!(after.subjects.contains_key("m0-0"));
}

#[test]
fn a_new_mailbox_view_starts_at_the_top_even_with_old_native_scroll_state() {
    let mailbox = pagination_mailbox(false);
    let (context, mut state, before) = pagination_view(&mailbox);
    assert!(before.offset > 0.0);
    state.conversation_scroll = None;
    let fresh = pagination_frame(&context, &mailbox, &mut state, Default::default());
    assert_eq!(fresh.offset, 0.0);
    assert!(fresh.subjects.contains_key("p0"));
}

#[test]
fn first_inspection_keeps_its_conversation_at_the_same_position_in_a_scrolled_list() {
    let mut mailbox = pagination_mailbox(false);
    let (context, mut state, before) = pagination_view(&mailbox);
    mailbox.split_conversation(&Folder::INBOX, "p44", inspected_page_members(44));
    let after = pagination_frame(&context, &mailbox, &mut state, Default::default());
    assert_eq!(after.offset, before.offset);
    assert_eq!(after.subjects["m44-0"], before.subjects["p44"]);
}

#[test]
fn repeated_refresh_and_reinspection_keep_top_middle_and_bottom_without_duplicates() {
    for parent in [0, 20, 45] {
        let mut mailbox = pagination_mailbox(false);
        let id = format!("p{parent}");
        mailbox.split_conversation(&Folder::INBOX, &id, inspected_page_members(parent));
        let (context, mut state, _) = pagination_view(&mailbox);
        set_list_offset(&context, &state, parent as f32 * ROW_PITCH);
        let before = pagination_frame(&context, &mailbox, &mut state, Default::default());
        for request in 20..24 {
            let refresh = mailbox.refresh(request).unwrap();
            let waiting = pagination_frame(&context, &mailbox, &mut state, Default::default());
            assert_eq!(waiting.offset, before.offset);
            let mut page = pagination_page(0, 50, 120);
            page.conversations[parent as usize].unread = request % 2 == 0;
            page.conversations[parent as usize].starred = request % 2 != 0;
            mailbox.finish_page(refresh.id, Ok(page));
            let refreshed = pagination_frame(&context, &mailbox, &mut state, Default::default());
            assert_eq!(refreshed.offset, before.offset);
            mailbox.split_conversation(&Folder::INBOX, &id, inspected_page_members(parent));
            let after = pagination_frame(&context, &mailbox, &mut state, Default::default());
            assert_eq!(after.offset, before.offset);
            assert_eq!(
                after.subjects[&format!("m{parent}-0")],
                before.subjects[&format!("m{parent}-0")]
            );
            assert_eq!(mailbox.visible_count(), 51);
            assert_eq!(
                mailbox
                    .visible_conversations()
                    .map(|row| &row.id)
                    .collect::<std::collections::HashSet<_>>()
                    .len(),
                51
            );
        }
    }
}

#[test]
fn changed_group_retains_the_original_message_anchor_until_reinspection() {
    let mut mailbox = pagination_mailbox(false);
    mailbox.split_conversation(&Folder::INBOX, "p20", inspected_page_members(20));
    let (context, mut state, _) = pagination_view(&mailbox);
    set_list_offset(&context, &state, 21.0 * ROW_PITCH);
    let before = pagination_frame(&context, &mailbox, &mut state, Default::default());
    let refresh = mailbox.refresh(20).unwrap();
    let mut page = pagination_page(0, 50, 120);
    page.conversations[20].message_count += 1;
    mailbox.finish_page(refresh.id, Ok(page));
    pagination_frame(&context, &mailbox, &mut state, Default::default());
    pagination_frame(&context, &mailbox, &mut state, Default::default());
    mailbox.split_conversation(&Folder::INBOX, "p20", inspected_page_members(20));
    let after = pagination_frame(&context, &mailbox, &mut state, Default::default());
    assert_eq!(after.offset, before.offset);
    assert_eq!(after.subjects["m20-1"], before.subjects["m20-1"]);
}

#[test]
fn settings_button_is_neutral_and_soft_violet_while_settings_is_open() {
    fn rects(shape: &egui::epaint::Shape, found: &mut Vec<(Color32, Stroke)>) {
        match shape {
            egui::epaint::Shape::Rect(rect) => found.push((rect.fill, rect.stroke)),
            egui::epaint::Shape::Vec(shapes) => {
                for shape in shapes {
                    rects(shape, found);
                }
            }
            _ => {}
        }
    }

    for appearance in [
        crate::settings::Appearance::Dark,
        crate::settings::Appearance::Light,
    ] {
        for selected in [false, true] {
            let context = egui::Context::default();
            theme::install(&context);
            theme::apply(&context, appearance);
            let render = |input| {
                let mut button = egui::Rect::NOTHING;
                // At rest, under the pointer and pressed.
                let mut expected = [Color32::PLACEHOLDER; 3];
                let output = context.run_ui(input, |ui| {
                    ui.set_width(240.0);
                    let widgets = &ui.visuals().widgets;
                    expected = if selected {
                        [theme::colors(ui).accent_soft; 3]
                    } else {
                        [
                            widgets.inactive.weak_bg_fill,
                            widgets.hovered.weak_bg_fill,
                            widgets.active.weak_bg_fill,
                        ]
                    };
                    button = settings_button(ui, selected).rect;
                });
                let mut found = Vec::new();
                for clipped in &output.shapes {
                    rects(&clipped.shape, &mut found);
                }
                output.drop_without_applying_deltas();
                // No violet outline, which read as a focus ring.
                assert!(
                    found
                        .iter()
                        .all(|(_, stroke)| stroke.color != theme::ACCENT),
                    "{appearance:?}, {selected}"
                );
                (button, found, expected)
            };

            let (button, idle, expected) = render(egui::RawInput::default());
            assert!(
                idle.iter().any(|(fill, _)| *fill == expected[0]),
                "{appearance:?}, {selected}"
            );
            let position = button.center();
            let hover = || egui::RawInput {
                events: vec![egui::Event::PointerMoved(position)],
                ..Default::default()
            };
            render(hover());
            let (_, hovered, _) = render(hover());
            assert!(
                hovered.iter().any(|(fill, _)| *fill == expected[1]),
                "{appearance:?}, {selected}"
            );

            render(egui::RawInput {
                events: vec![egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::default(),
                }],
                ..Default::default()
            });
            let (_, pressed, _) = render(egui::RawInput::default());
            assert!(
                pressed.iter().any(|(fill, _)| *fill == expected[2]),
                "{appearance:?}, {selected}"
            );
        }
    }
}

#[test]
fn settings_replaces_mail_panes_and_preserves_their_split() {
    fn contains_text(shape: &egui::epaint::Shape, text: &str) -> bool {
        match shape {
            egui::epaint::Shape::Text(label) => label.galley.text() == text,
            egui::epaint::Shape::Vec(shapes) => {
                shapes.iter().any(|shape| contains_text(shape, text))
            }
            _ => false,
        }
    }

    let (app, _) = App::boot(true, Settings::default());
    let mut draft = app.settings().clone();
    let mut state = UiState::default();
    let mut messages = Vec::new();
    let original_split = app.panels().conversations;
    let context = egui::Context::default();
    theme::install(&context);
    let output = context.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(820.0, 480.0),
            )),
            ..Default::default()
        },
        |root| {
            show(root, &app, Some(&mut draft), &mut state, &mut messages);
        },
    );
    assert!(
        output
            .shapes
            .iter()
            .any(|clipped| { contains_text(&clipped.shape, "Back to mail") })
    );
    assert!(messages.iter().all(|message| {
        !matches!(message, Message::PanelsResized(panels) if panels.conversations != original_split)
    }));
    output.drop_without_applying_deltas();
}

#[test]
fn a_right_click_on_a_row_offers_its_actions_without_opening_it() {
    let (mut app, _) = App::boot(true, Settings::default());
    let _ = app.update(Message::ConversationsLoaded(
        1,
        Ok(ConversationPage {
            conversations: vec![
                pagination_row("demo-3", 1000),
                pagination_row("demo-4", 900),
            ],
            total: 2,
            inspect_candidates: Vec::new(),
        }),
    ));
    let context = egui::Context::default();
    theme::install(&context);
    let mut state = UiState::default();
    let size = egui::vec2(1100.0, 700.0);
    let mut frame = |events: Vec<egui::Event>| {
        mailbox_frame(
            &context,
            &app,
            &mut state,
            size,
            None,
            egui::RawInput {
                events,
                ..Default::default()
            },
        )
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

    let (_, shapes) = frame(Vec::new());
    let row = visible_text_rect(&shapes, "demo-4").expect("the second row");
    let (asked, _) = frame(click(row.center(), egui::PointerButton::Secondary));
    assert!(asked.is_empty(), "a right click opens nothing");
    let mut shapes = Vec::new();
    for _ in 0..3 {
        shapes = frame(Vec::new()).1;
    }
    for item in [
        "Archive",
        "Move to Spam",
        "Delete (move to Trash)",
        "Mark as unread",
        "Add star",
    ] {
        assert!(
            visible_text_rect(&shapes, item).is_some(),
            "{item} is missing"
        );
    }
    assert!(
        shapes.iter().any(|clipped| matches!(&clipped.shape,
                egui::epaint::Shape::Rect(rect)
                    if rect.stroke.color == theme::ACCENT && rect.rect.contains(row.center()))),
        "the row the menu belongs to is outlined"
    );

    // The sidebar has an Archive too; the menu is drawn above it, last.
    let archive = shapes
        .iter()
        .rev()
        .find_map(|clipped| match &clipped.shape {
            egui::epaint::Shape::Text(text) if text.galley.text() == "Archive" => {
                Some(egui::Rect::from_min_size(text.pos, text.galley.size()))
            }
            _ => None,
        })
        .unwrap();
    let (asked, _) = frame(click(archive.center(), egui::PointerButton::Primary));
    assert!(
        matches!(
            asked.as_slice(),
            [Message::ApplyRowAction(id, crate::mail::MailAction::MoveTo(to))]
                if id == "demo-4" && *to == Folder::System(MailFolder::Archive)
        ),
        "Archive should act on the row that was clicked"
    );
}

#[test]
fn the_compact_reader_opens_only_with_a_conversation_to_read() {
    let (mut app, _) = App::boot(true, Settings::default());
    let _ = app.update(Message::ConversationsLoaded(
        1,
        Ok(ConversationPage {
            conversations: vec![pagination_row("demo-3", 1000)],
            total: 1,
            inspect_candidates: Vec::new(),
        }),
    ));
    let context = egui::Context::default();
    theme::install(&context);
    let size = egui::vec2(469.0, 700.0);
    let mut state = UiState {
        compact_view: CompactView::Reader,
        ..Default::default()
    };

    mailbox_frame(&context, &app, &mut state, size, None, Default::default());
    assert_eq!(state.compact_view, CompactView::Mail, "nothing to read");

    let _ = app.update(Message::SelectConversation("demo-3".into()));
    state.compact_view = CompactView::Reader;
    mailbox_frame(&context, &app, &mut state, size, None, Default::default());
    assert_eq!(state.compact_view, CompactView::Reader);
}

#[test]
fn the_offer_to_undo_sits_under_the_list_it_was_made_from() {
    let (mut app, _) = App::boot(true, Settings::default());
    let _ = app.update(Message::ConversationsLoaded(
        1,
        Ok(ConversationPage {
            conversations: vec![
                pagination_row("demo-3", 1000),
                pagination_row("demo-4", 900),
            ],
            total: 2,
            inspect_candidates: Vec::new(),
        }),
    ));
    let _ = app.update(Message::SelectConversation("demo-3".into()));
    let _ = app.update(Message::ApplyAction(crate::mail::MailAction::MoveTo(
        Folder::System(MailFolder::Archive),
    )));
    let context = egui::Context::default();
    theme::install(&context);
    let mut state = UiState::default();
    let size = egui::vec2(1100.0, 700.0);
    let mut shapes = Vec::new();
    for _ in 0..3 {
        (_, shapes) = mailbox_frame(&context, &app, &mut state, size, None, Default::default());
    }

    let notice = visible_text_rect(&shapes, "Moved to Archive.").expect("an offer to undo");
    let list = visible_text_rect(&shapes, "Refresh").expect("the list's header");
    let search = visible_text_rect(&shapes, "Search mail…").expect("the list's search");
    // Between the list's left edge, near its search text, and its Refresh.
    assert!(
        search.left() - 40.0 < notice.left() && notice.right() < list.right() + 20.0,
        "the notice at {notice:?} leaves the list column"
    );
}

#[test]
fn undo_answers_while_the_list_refreshes() {
    let (mut app, _) = App::boot(true, Settings::default());
    let _ = app.update(Message::ConversationsLoaded(
        1,
        Ok(ConversationPage {
            conversations: vec![pagination_row("demo-3", 1000)],
            total: 1,
            inspect_candidates: Vec::new(),
        }),
    ));
    let _ = app.update(Message::SelectConversation("demo-3".into()));
    let _ = app.update(Message::ApplyAction(crate::mail::MailAction::MoveTo(
        Folder::System(MailFolder::Archive),
    )));
    let _ = app.update(Message::RefreshMailbox);
    assert!(matches!(
        app.mailbox().unwrap().status(),
        ListStatus::Refreshing(_)
    ));
    let context = egui::Context::default();
    theme::install(&context);
    let mut state = UiState::default();
    let size = egui::vec2(1100.0, 700.0);
    let mut shapes = Vec::new();
    for _ in 0..3 {
        (_, shapes) = mailbox_frame(&context, &app, &mut state, size, None, Default::default());
    }
    let undo = visible_text_rect(&shapes, "Undo")
        .expect("the offer's Undo")
        .center();

    let press = |pressed| egui::Event::PointerButton {
        pos: undo,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::default(),
    };
    let click = egui::RawInput {
        events: vec![egui::Event::PointerMoved(undo), press(true), press(false)],
        ..Default::default()
    };
    let (asked, _) = mailbox_frame(&context, &app, &mut state, size, None, click);
    assert!(
        asked
            .iter()
            .any(|message| matches!(message, Message::UndoMove)),
        "Undo waited for the refresh"
    );
}

#[test]
fn an_offer_to_undo_stays_on_screen_while_settings_is_open() {
    let (mut app, _) = App::boot(true, Settings::default());
    // A row the demo account has, so the demo can move it.
    let _ = app.update(Message::ConversationsLoaded(
        1,
        Ok(ConversationPage {
            conversations: vec![pagination_row("demo-3", 1000)],
            total: 1,
            inspect_candidates: Vec::new(),
        }),
    ));
    let _ = app.update(Message::SelectConversation("demo-3".into()));
    let _ = app.update(Message::ApplyAction(crate::mail::MailAction::MoveTo(
        Folder::System(MailFolder::Archive),
    )));
    assert!(app.mailbox().unwrap().undo().is_some());
    let mut draft = app.settings().clone();

    // Wide and compact windows. A new area shows from its second frame.
    for size in [egui::vec2(1100.0, 700.0), egui::vec2(469.0, 700.0)] {
        let context = egui::Context::default();
        theme::install(&context);
        let mut state = UiState::default();
        let mut shapes = Vec::new();
        for _ in 0..3 {
            (_, shapes) = mailbox_frame(
                &context,
                &app,
                &mut state,
                size,
                Some(&mut draft),
                Default::default(),
            );
        }
        assert!(
            visible_text_rect(&shapes, "Moved to Archive.").is_some(),
            "no offer to undo at {size:?}"
        );
    }
}

#[test]
fn new_message_text_is_centered_in_its_button() {
    let context = egui::Context::default();
    let mut centers = (0.0, 0.0);

    context
        .run_ui(egui::RawInput::default(), |ui| {
            ui.set_width(240.0);
            let button = new_message_button(ui);
            centers = (
                button.response.rect.center().x,
                button
                    .rect(egui::Id::new(NEW_MESSAGE_LABEL_ID))
                    .expect("new message label")
                    .center()
                    .x,
            );
        })
        .drop_without_applying_deltas();

    assert!((centers.0 - centers.1).abs() <= 0.5);
}

#[test]
fn selected_folder_name_gets_an_extra_text_pass() {
    fn text_shapes(shape: &egui::epaint::Shape) -> usize {
        match shape {
            egui::epaint::Shape::Text(_) => 1,
            egui::epaint::Shape::Vec(shapes) => shapes.iter().map(text_shapes).sum(),
            _ => 0,
        }
    }

    let render = |selected| {
        let current = if selected {
            Folder::INBOX
        } else {
            Folder::System(MailFolder::Drafts)
        };
        let (mailbox, _) = Mailbox::open(current, 50, 1, 2);
        let context = egui::Context::default();
        let mut output = context.run_ui(egui::RawInput::default(), |ui| {
            ui.set_width(240.0);
            folder_button(
                ui,
                &mailbox,
                Some(mailbox.folder()),
                Folder::INBOX,
                crate::mail::PlaceLook::default(),
                &mut Vec::new(),
            );
        });
        let count = output
            .shapes
            .iter()
            .map(|shape| text_shapes(&shape.shape))
            .sum::<usize>();
        output.textures_delta.clear();
        count
    };

    assert_eq!(render(true), render(false) + 1);
}

/// The fills of every rectangle in one pass's shapes.
fn rect_fills(output: &egui::FullOutput) -> Vec<(Color32, Stroke)> {
    fn collect(shape: &egui::epaint::Shape, found: &mut Vec<(Color32, Stroke)>) {
        match shape {
            egui::epaint::Shape::Rect(rect) => found.push((rect.fill, rect.stroke)),
            egui::epaint::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect(shape, found);
                }
            }
            _ => {}
        }
    }
    let mut found = Vec::new();
    for clipped in &output.shapes {
        collect(&clipped.shape, &mut found);
    }
    found
}

#[test]
fn a_folder_under_the_pointer_takes_a_quiet_tint() {
    for appearance in [
        crate::settings::Appearance::Dark,
        crate::settings::Appearance::Light,
    ] {
        let context = egui::Context::default();
        theme::install(&context);
        theme::apply(&context, appearance);
        let (mailbox, _) = Mailbox::open(Folder::INBOX, 50, 1, 2);
        let drafts = Folder::System(MailFolder::Drafts);
        let render = |input| {
            let mut place = egui::Rect::NOTHING;
            let mut tint = Color32::PLACEHOLDER;
            let output = context.run_ui(input, |ui| {
                ui.set_width(240.0);
                tint = theme::colors(ui).sidebar_hover;
                folder_button(
                    ui,
                    &mailbox,
                    Some(mailbox.folder()),
                    drafts.clone(),
                    crate::mail::PlaceLook::default(),
                    &mut Vec::new(),
                );
                place = ui.min_rect();
            });
            let fills = rect_fills(&output);
            output.drop_without_applying_deltas();
            (place, fills, tint)
        };

        let (place, fills, tint) = render(egui::RawInput::default());
        assert!(
            !fills.iter().any(|(fill, _)| *fill == tint),
            "{appearance:?}"
        );
        let hover = || egui::RawInput {
            events: vec![egui::Event::PointerMoved(place.center())],
            ..Default::default()
        };
        render(hover());
        let (_, fills, _) = render(hover());
        assert!(
            fills
                .iter()
                .any(|(fill, stroke)| *fill == tint && stroke.is_empty()),
            "{appearance:?}: {fills:?}"
        );
    }
}

#[test]
fn an_open_settings_page_leaves_no_folder_marked_as_chosen() {
    let context = egui::Context::default();
    theme::install(&context);
    let (mailbox, _) = Mailbox::open(Folder::INBOX, 50, 1, 2);
    let render = |current| {
        let mut soft = Color32::PLACEHOLDER;
        let output = context.run_ui(egui::RawInput::default(), |ui| {
            ui.set_width(240.0);
            soft = theme::colors(ui).accent_soft;
            folder_button(
                ui,
                &mailbox,
                current,
                Folder::INBOX,
                crate::mail::PlaceLook::default(),
                &mut Vec::new(),
            );
        });
        let marked = rect_fills(&output).iter().any(|(fill, _)| *fill == soft);
        output.drop_without_applying_deltas();
        marked
    };

    assert!(render(Some(mailbox.folder())));
    assert!(!render(None));
}

#[test]
fn search_scope_says_what_the_list_is_showing() {
    let narrowing = search_scope_label(None, 50);
    assert!(narrowing.contains("50 loaded conversations"));
    assert!(narrowing.contains("Enter"));
    assert!(search_scope_label(None, 1).contains("1 loaded conversation "));
    let found = search_scope_label(Some("invoice"), 50);
    assert!(found.contains("invoice"));
    assert!(!found.contains("Enter"));
}

#[test]
fn times_are_formatted_relative_to_now() {
    let now = Utc.with_ymd_and_hms(2026, 9, 11, 18, 0, 0).unwrap();
    let at = |y, m, d, h, min| {
        Utc.with_ymd_and_hms(y, m, d, h, min, 0)
            .unwrap()
            .timestamp()
    };
    assert_eq!(format_time(at(2026, 9, 11, 9, 5), &now), "09:05");
    assert_eq!(format_time(at(2026, 3, 2, 9, 5), &now), "Mar 2");
    assert_eq!(format_time(at(2025, 12, 31, 9, 5), &now), "2025-12-31");
}

#[test]
fn a_bottom_notice_expires_and_a_new_one_gets_its_own_time() {
    let offer = crate::app::UndoMove {
        row_id: "first".into(),
        kind: crate::mail::SummaryKind::Conversation,
        from: Folder::INBOX,
        to: Folder::System(MailFolder::Trash),
        row: None,
    };
    let undo = BottomNotice::Undo(Box::new(offer.clone()));
    let mut state = UiState::default();
    let remaining = |state: &mut UiState, notice: &BottomNotice, now| {
        notice_remaining(state, notice, now, Pointer::Away)
    };

    assert_eq!(
        remaining(&mut state, &undo, 10.0),
        Some(std::time::Duration::from_secs(6))
    );
    assert_eq!(
        remaining(&mut state, &undo, 15.0),
        Some(std::time::Duration::from_secs(1))
    );
    assert_eq!(remaining(&mut state, &undo, 16.0), None);

    let next = BottomNotice::Undo(Box::new(crate::app::UndoMove {
        row_id: "second".into(),
        ..offer
    }));
    assert_eq!(
        remaining(&mut state, &next, 16.0),
        Some(std::time::Duration::from_secs(6))
    );
    let sent = BottomNotice::Confirmation(Notice::Sent);
    assert_eq!(
        remaining(&mut state, &sent, 17.0),
        Some(std::time::Duration::from_secs(6))
    );
}

#[test]
fn a_saved_file_is_confirmed_with_a_way_to_show_it() {
    let path = std::path::PathBuf::from("/tmp/report.pdf");
    let notice = BottomNotice::Confirmation(Notice::Saved(path.clone()));
    let (mailbox, _) = Mailbox::open(Folder::INBOX, 50, 1, 2);
    let context = egui::Context::default();
    theme::install(&context);
    let frame = |events: Vec<egui::Event>| {
        let mut messages = Vec::new();
        let output = context.run_ui(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ui| notice_content(ui, &notice, &mailbox, &mut messages),
        );
        let shapes = output.shapes.clone();
        output.drop_without_applying_deltas();
        (shapes, messages)
    };

    let (shapes, _) = frame(Vec::new());
    text_rect(&shapes, "Saved report.pdf.");
    let reveal = text_rect(&shapes, REVEAL_LABEL).center();
    let press = |pressed| egui::Event::PointerButton {
        pos: reveal,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::default(),
    };
    let (_, asked) = frame(vec![
        egui::Event::PointerMoved(reveal),
        press(true),
        press(false),
    ]);

    assert!(matches!(
        asked.as_slice(),
        [Message::RevealAttachment(shown)] if *shown == path
    ));
}

#[tokio::test]
async fn a_sent_message_is_confirmed_at_the_foot_while_the_pointer_rests_there() {
    let (mut app, _) = App::boot(true, Settings::default());
    let _ = app.update(Message::OpenCompose);
    for (field, value) in [
        (crate::app::ComposeField::To, "ana@example.org"),
        (crate::app::ComposeField::Subject, "Plans"),
        (crate::app::ComposeField::Body, "See you there"),
    ] {
        let _ = app.update(Message::ComposeChanged(field, value.into()));
    }
    for effect in app.update(Message::Send).into_iter() {
        if let crate::app::Effect::Future(sending) = effect {
            let sent = sending.await;
            let _ = app.update(sent);
        }
    }
    assert_eq!(app.notice(), Some(&Notice::Sent));

    let context = egui::Context::default();
    theme::install(&context);
    let mut state = UiState::default();
    let size = egui::vec2(1100.0, 700.0);
    let at = |time: f64, events: Vec<egui::Event>| egui::RawInput {
        time: Some(time),
        events,
        ..Default::default()
    };
    let mut shapes = Vec::new();
    for _ in 0..2 {
        (_, shapes) = mailbox_frame(&context, &app, &mut state, size, None, at(1.0, Vec::new()));
    }
    let notice = visible_text_rect(&shapes, "Message sent.").expect("a confirmation");

    // Past its own time, the pointer on it keeps it.
    let rest = vec![egui::Event::PointerMoved(notice.center())];
    let (asked, shapes) = mailbox_frame(&context, &app, &mut state, size, None, at(9.0, rest));
    assert!(
        !asked
            .iter()
            .any(|message| matches!(message, Message::DismissNotice(_)))
    );
    assert!(visible_text_rect(&shapes, "Message sent.").is_some());

    let away = vec![egui::Event::PointerGone];
    let (asked, _) = mailbox_frame(&context, &app, &mut state, size, None, at(16.0, away));
    assert!(
        asked
            .iter()
            .any(|message| matches!(message, Message::DismissNotice(Notice::Sent)))
    );
}

#[test]
fn a_bottom_notice_stays_while_the_pointer_rests_on_it() {
    let sent = BottomNotice::Confirmation(Notice::Sent);
    let mut state = UiState::default();
    notice_remaining(&mut state, &sent, 1.0, Pointer::Away);

    // Past its own time, it is still there to be read.
    assert_eq!(
        notice_remaining(&mut state, &sent, 9.0, Pointer::OnNotice),
        Some(std::time::Duration::from_secs(6))
    );
    // Once the pointer leaves, its time runs again from then.
    assert_eq!(
        notice_remaining(&mut state, &sent, 14.0, Pointer::Away),
        Some(std::time::Duration::from_secs(1))
    );
    assert_eq!(
        notice_remaining(&mut state, &sent, 15.0, Pointer::Away),
        None
    );
}

#[test]
fn a_long_folder_name_stays_on_one_row() {
    // Account folder names truncate rather than changing row height.
    let (mailbox, _) = Mailbox::open(Folder::INBOX, 50, 1, 2);
    let label = Folder::label(
        "kZ9",
        "A label whose name runs far past anything the sidebar is wide enough for",
    );
    let context = egui::Context::default();
    let mut used = egui::Rect::NOTHING;

    context
        .run_ui(egui::RawInput::default(), |ui| {
            ui.set_width(200.0);
            folder_button(
                ui,
                &mailbox,
                Some(mailbox.folder()),
                label.clone(),
                crate::mail::PlaceLook::default(),
                &mut Vec::new(),
            );
            used = ui.min_rect();
        })
        .drop_without_applying_deltas();

    assert_eq!(used.height(), FOLDER_HEIGHT, "the folder took extra rows");
    assert!(
        used.width() <= 200.0,
        "the sidebar grew to {} for one folder name",
        used.width()
    );
}

#[test]
fn a_row_is_exactly_the_height_the_list_places_it_at() {
    // Rendered and virtualized row heights must match.
    let conversation = ConversationSummary {
        id: "row".into(),
        kind: crate::mail::SummaryKind::Conversation,
        subject: Some("A subject".into()),
        correspondents: Some("Alex Rivera".into()),
        participants: Vec::new(),
        preview: None,
        time: Some(1_700_000_000),
        unread: true,
        starred: true,
        message_count: 3,
        has_attachments: true,
    };
    let context = egui::Context::default();
    let now = Local::now();
    let mut height = 0.0;
    context
        .run_ui(egui::RawInput::default(), |ui| {
            ui.set_width(320.0);
            height = conversation_row(ui, &conversation, true, &now)
                .rect
                .height();
        })
        .drop_without_applying_deltas();

    assert_eq!(height, ROW_HEIGHT);
}

#[test]
fn a_row_keeps_its_sender_above_its_subject_and_clear_of_its_time() {
    let sender = "A sender whose name runs on and on, well past the time";
    let mut conversation = pagination_row("A subject", 1_700_000_000);
    conversation.correspondents = Some(sender.into());
    let context = egui::Context::default();
    let now = Local::now();
    let output = context.run_ui(egui::RawInput::default(), |ui| {
        ui.set_width(320.0);
        conversation_row(ui, &conversation, false, &now);
    });
    let shapes = output.shapes.clone();
    output.drop_without_applying_deltas();

    let sender = visible_text_rect(&shapes, sender).expect("the sender line");
    let subject = visible_text_rect(&shapes, "A subject").expect("the subject line");
    let time = visible_text_rect(&shapes, &format_time(1_700_000_000, &now)).expect("the time");
    assert!(
        sender.center().y < subject.center().y,
        "{sender:?} {subject:?}"
    );
    assert_eq!(sender.left(), subject.left());
    assert!(
        sender.right() <= time.left(),
        "{sender:?} runs into {time:?}"
    );
}

#[test]
fn unread_rows_are_brighter_than_rows_already_read() {
    let now = Local::now();
    for appearance in [
        crate::settings::Appearance::Dark,
        crate::settings::Appearance::Light,
    ] {
        let context = egui::Context::default();
        theme::install(&context);
        theme::apply(&context, appearance);
        let render = |unread: bool| {
            let conversation = ConversationSummary {
                id: "row".into(),
                kind: crate::mail::SummaryKind::Conversation,
                subject: Some("Lunch on Friday".into()),
                correspondents: Some("Alex Rivera".into()),
                participants: Vec::new(),
                preview: None,
                time: None,
                unread,
                starred: false,
                message_count: 1,
                has_attachments: false,
            };
            let mut palette = (
                Color32::TRANSPARENT,
                Color32::TRANSPARENT,
                Color32::TRANSPARENT,
            );
            let output = context.run_ui(egui::RawInput::default(), |ui| {
                ui.set_width(320.0);
                conversation_row(ui, &conversation, false, &now);
                palette = (
                    ui.visuals().strong_text_color(),
                    ui.visuals().text_color(),
                    theme::colors(ui).muted,
                );
            });
            let color_of = |line: &str| {
                output
                    .shapes
                    .iter()
                    .find_map(|clipped| match &clipped.shape {
                        egui::epaint::Shape::Text(text) if text.galley.text() == line => {
                            Some(text.galley.job.sections[0].format.color)
                        }
                        _ => None,
                    })
                    .unwrap_or_else(|| panic!("missing {line}"))
            };
            let lines = (color_of("Alex Rivera"), color_of("Lunch on Friday"));
            output.drop_without_applying_deltas();
            (lines, palette)
        };

        let ((sender, subject), (strong, normal, muted)) = render(true);
        assert_eq!(
            (sender, subject),
            (strong, normal),
            "unread, {appearance:?}"
        );
        let ((sender, subject), _) = render(false);
        assert_eq!((sender, subject), (normal, muted), "read, {appearance:?}");
        assert_ne!(strong, normal, "{appearance:?}");
    }
}

#[test]
fn a_row_tells_screen_readers_its_state_and_keeps_the_arrow() {
    let conversation = ConversationSummary {
        id: "row".into(),
        kind: crate::mail::SummaryKind::Conversation,
        subject: Some("Lunch on Friday".into()),
        correspondents: Some("Alex Rivera".into()),
        participants: Vec::new(),
        preview: None,
        time: None,
        unread: true,
        starred: true,
        message_count: 3,
        has_attachments: true,
    };
    let context = egui::Context::default();
    theme::install(&context);
    context.enable_accesskit();
    let now = Local::now();
    let render = |input| {
        let mut rect = egui::Rect::NOTHING;
        let output = context.run_ui(input, |ui| {
            ui.set_width(320.0);
            rect = conversation_row(ui, &conversation, true, &now).rect;
        });
        let node = output
            .platform_output
            .accesskit_update
            .iter()
            .flat_map(|update| &update.nodes)
            .find_map(|(_, node)| {
                node.label()
                    .filter(|label| label.contains("Alex Rivera"))
                    .map(|label| (label.to_owned(), node.toggled()))
            });
        let cursor = output.platform_output.cursor_icon;
        output.drop_without_applying_deltas();
        (rect, node, cursor)
    };

    let (rect, node, _) = render(egui::RawInput::default());
    assert_eq!(
        node,
        Some((
            "Unread, Alex Rivera, Lunch on Friday, starred, 3 messages, attachment".into(),
            // egui reports a selected widget as toggled on.
            Some(egui::accesskit::Toggled::True)
        ))
    );
    let hover = || egui::RawInput {
        events: vec![egui::Event::PointerMoved(rect.center())],
        ..Default::default()
    };
    render(hover());
    let (_, _, cursor) = render(hover());
    assert_eq!(cursor, egui::CursorIcon::Default, "a row is not a link");
}

#[test]
fn a_skeleton_row_matches_the_real_row_height() {
    egui::__run_test_ui(|ui| {
        ui.set_width(320.0);
        let response = skeleton_conversation_row(ui, 0, Color32::GRAY);
        assert_eq!(response.rect.height(), ROW_HEIGHT);
        assert!(!response.sense.senses_click());
    });
}

#[test]
fn long_correspondents_cannot_widen_the_conversation_pane() {
    let conversation = ConversationSummary {
        id: "long-row".into(),
        kind: crate::mail::SummaryKind::Conversation,
        subject: Some("A subject that also needs to stay bounded".into()),
        correspondents: Some("A very long correspondent name that must be truncated".into()),
        participants: Vec::new(),
        preview: None,
        time: None,
        unread: true,
        starred: false,
        message_count: 1,
        has_attachments: false,
    };
    let context = egui::Context::default();
    let now = Local::now();
    let render = |input| {
        let mut result = (egui::Rect::NOTHING, false);
        context
            .run_ui(input, |ui| {
                ui.set_width(280.0);
                let response = conversation_row(ui, &conversation, false, &now);
                result = (response.rect, response.clicked());
            })
            .drop_without_applying_deltas();
        result
    };

    let (rect, _) = render(egui::RawInput::default());
    assert_eq!(rect.width(), 280.0);
    let position = rect.center();
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
    let (_, clicked) = render(egui::RawInput {
        events: vec![egui::Event::PointerMoved(position), pointer(false)],
        ..Default::default()
    });

    assert!(clicked);
}

#[test]
fn subfolders_sit_under_their_parent_and_labels_wear_their_color() {
    let context = egui::Context::default();
    theme::install(&context);
    let (mut app, _) = App::boot(true, Settings::default());
    let color = [0, 102, 204];
    let looks = [
        ("taxes", crate::mail::PlaceLook::default()),
        (
            "taxes-2024",
            crate::mail::PlaceLook {
                depth: 1,
                color: None,
            },
        ),
        (
            "work",
            crate::mail::PlaceLook {
                depth: 0,
                color: Some(color),
            },
        ),
    ]
    .into_iter()
    .map(|(id, look)| (id.to_owned(), look))
    .collect();
    let _ = app.update(Message::FoldersLoaded(
        1,
        Ok(crate::mail::OwnPlaces {
            folders: vec![
                Folder::custom("taxes", "Taxes"),
                Folder::custom("taxes-2024", "2024"),
                Folder::label("work", "Work"),
            ],
            looks,
        }),
    ));

    let output = context.run_ui(egui::RawInput::default(), |root| {
        egui::CentralPanel::default().show(root, |ui| {
            sidebar_content(ui, &app, app.mailbox().unwrap(), &mut Vec::new());
        });
    });
    let shapes = output.shapes.clone();
    output.drop_without_applying_deltas();

    let parent = crate::ui::test_support::text_rect(&shapes, "Taxes");
    let child = crate::ui::test_support::text_rect(&shapes, "2024");
    assert!(
        child.left() >= parent.left() + SUBFOLDER_INDENT - 0.5,
        "{child:?} is not indented under {parent:?}"
    );
    assert!(
        child.top() > parent.top(),
        "the subfolder comes after its parent"
    );
    assert_eq!(
        label_color(app.place_look(&Folder::label("work", "Work"))),
        egui::Color32::from_rgb(color[0], color[1], color[2])
    );
    assert_eq!(
        label_color(crate::mail::PlaceLook::default()),
        theme::ACCENT
    );
}
