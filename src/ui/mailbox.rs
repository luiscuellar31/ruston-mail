use chrono::{DateTime, Datelike, Local, TimeZone};
use eframe::egui::{self, Align, Align2, Color32, CornerRadius, FontId, Layout, Sense, Stroke};

use super::{UiState, reader, theme};
use crate::app::{App, ListStatus, Mailbox, Message, panel_ratios, panel_widths};
use crate::mail::{ConversationSummary, CustomKind, Folder, MailFolder};

/// One of the few marks egui's bundled fonts carry; the paperclip beside it
/// in a row has to be drawn instead (see [`theme::paint_clip`]).
const STAR: &str = "★";

pub(super) fn show(
    root: &mut egui::Ui,
    app: &App,
    email: Option<&str>,
    signing_out: bool,
    state: &mut UiState,
    messages: &mut Vec<Message>,
) {
    let mailbox = app.mailbox().expect("authenticated mailbox");
    let window_width = root.available_width();
    let widths = panel_widths(app.panels(), window_width);

    let sidebar = egui::Panel::left("mailbox-sidebar")
        .default_size(widths.sidebar)
        .size_range(200.0..=(window_width - 400.0).max(200.0))
        .resizable(true)
        .frame(theme::panel_frame(theme::SIDEBAR))
        .show(root, |ui| {
            sidebar(ui, app, mailbox, email, signing_out, messages)
        });

    let remaining = (window_width - sidebar.response.rect.width()).max(400.0);
    let conversations = egui::Panel::left("conversation-list")
        .default_size(widths.conversations)
        .size_range(200.0..=(remaining - 200.0).max(200.0))
        .resizable(true)
        .frame(theme::panel_frame(theme::PANEL))
        .show(root, |ui| conversation_pane(ui, mailbox, state, messages));

    egui::CentralPanel::default()
        .frame(theme::panel_frame(theme::PANEL))
        .show(root, |ui| {
            reader::show(
                ui,
                mailbox,
                app.folders(),
                app.mailbox_actions_available(),
                app.pending_link(),
                app.saving_attachment(),
                app.saved_attachment(),
                app.settings().reading(),
                state,
                messages,
            );
        });

    let actual = panel_ratios(
        sidebar.response.rect.width(),
        conversations.response.rect.width(),
        window_width,
    );
    let stored = app.panels();
    if (actual.sidebar - stored.sidebar).abs() > 0.002
        || (actual.conversations - stored.conversations).abs() > 0.002
    {
        messages.push(Message::PanelsResized(actual));
    }
}

fn sidebar(
    ui: &mut egui::Ui,
    app: &App,
    mailbox: &Mailbox,
    email: Option<&str>,
    signing_out: bool,
    messages: &mut Vec<Message>,
) {
    ui.heading(egui::RichText::new("Ruston Mail").size(23.0));
    ui.add_space(12.0);

    for folder in MailFolder::ALL.into_iter().map(Folder::System).chain(
        app.folders()
            .iter()
            .filter(|folder| folder.kind() == Some(CustomKind::Folder))
            .cloned(),
    ) {
        folder_button(ui, mailbox, folder, messages);
    }

    let labels: Vec<_> = app
        .folders()
        .iter()
        .filter(|folder| folder.kind() == Some(CustomKind::Label))
        .cloned()
        .collect();
    if !labels.is_empty() {
        ui.add_space(14.0);
        detail(ui, "LABELS");
        for label in labels {
            folder_button(ui, mailbox, label, messages);
        }
    }

    ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
        let logout_label = if signing_out {
            "Signing out…"
        } else if app.is_demo() {
            "Exit demo"
        } else {
            "Sign out"
        };
        if ui
            .add_enabled(
                !signing_out,
                egui::Button::new(logout_label).min_size(egui::vec2(ui.available_width(), 32.0)),
            )
            .clicked()
        {
            messages.push(Message::Logout);
        }
        // Same frame and height as Sign out below it, outlined instead of
        // filled: one button, plainly a different kind of action.
        if ui
            .add(
                egui::Button::new("Settings")
                    .min_size(egui::vec2(ui.available_width(), 32.0))
                    .fill(Color32::TRANSPARENT)
                    .stroke(Stroke::new(1.0, theme::ACCENT)),
            )
            .clicked()
        {
            messages.push(Message::ShowSettings(true));
        }
        if let Some(error) = app.error_message() {
            ui.label(egui::RichText::new(error).small().color(theme::DANGER));
        }
        detail(
            ui,
            if app.is_demo() {
                "Demo mode · fictional mail"
            } else {
                email.unwrap_or("Proton Mail account")
            },
        );
    });
}

fn folder_button(
    ui: &mut egui::Ui,
    mailbox: &Mailbox,
    folder: Folder,
    messages: &mut Vec<Message>,
) {
    let selected = &folder == mailbox.folder();
    let unread = mailbox
        .counts()
        .and_then(|counts| counts.unread(&folder))
        .filter(|count| *count > 0);
    let mut button = egui::Button::new(folder.name())
        .wrap_mode(egui::TextWrapMode::Truncate)
        .min_size(egui::vec2(ui.available_width(), 31.0))
        .frame(selected);
    if selected {
        button = button
            .fill(theme::ACCENT_SOFT)
            .stroke(Stroke::new(1.0, theme::ACCENT));
    }
    let response = ui.add(button);
    if let Some(count) = unread {
        // Painted at the panel edge rather than padded into the label, which
        // left the count wherever the folder's name happened to end.
        ui.painter().text(
            egui::pos2(response.rect.right() - 10.0, response.rect.center().y),
            Align2::RIGHT_CENTER,
            count.to_string(),
            FontId::proportional(12.0),
            theme::MUTED,
        );
        let name = folder.name().to_owned();
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Button,
                true,
                format!("{name}, {count} unread"),
            )
        });
    }
    if response.clicked() {
        messages.push(Message::SelectFolder(folder));
    }
}

fn conversation_pane(
    ui: &mut egui::Ui,
    mailbox: &Mailbox,
    state: &mut UiState,
    messages: &mut Vec<Message>,
) {
    ui.horizontal(|ui| {
        ui.heading(egui::RichText::new(mailbox.folder().name()).size(21.0));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let refreshing = matches!(mailbox.status(), ListStatus::Refreshing(_));
            if ui
                .add_enabled(
                    !mailbox.is_busy(),
                    egui::Button::new(if refreshing {
                        "Refreshing…"
                    } else {
                        "Refresh"
                    }),
                )
                .clicked()
            {
                messages.push(Message::RefreshMailbox);
            }
        });
    });
    ui.add_space(6.0);

    let mut query = mailbox.search_query().to_owned();
    ui.horizontal(|ui| {
        let response = ui.add(
            egui::TextEdit::singleline(&mut query)
                .id(egui::Id::new("mail-search"))
                .hint_text("Search mail…")
                .desired_width(f32::INFINITY),
        );
        if state.focus_search {
            response.request_focus();
            state.focus_search = false;
        }
        if response.changed() {
            messages.push(Message::SearchChanged(query.clone()));
        }
        if response.has_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
            messages.push(Message::SearchSubmitted);
        }
        if !query.is_empty() && ui.button("Clear").clicked() {
            messages.push(Message::SearchChanged(String::new()));
        }
    });
    if !mailbox.search_query().is_empty() {
        detail(
            ui,
            &search_scope_label(mailbox.search_results(), mailbox.loaded_count()),
        );
    }
    if let Some(undo) = mailbox.undo() {
        ui.add_space(4.0);
        theme::card().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(format!("Moved to {}.", undo.to.name()));
                if ui.small_button("Undo").clicked() {
                    messages.push(Message::UndoMove);
                }
            });
        });
    }
    ui.add_space(4.0);

    let empty = mailbox.visible_conversations().next().is_none();
    match (mailbox.status(), empty) {
        (ListStatus::Loading(_), _) => centered(ui, "Loading conversations…"),
        (ListStatus::Failed(error), true) => {
            centered(ui, error.message());
            if ui.button("Try again").clicked() {
                messages.push(Message::RefreshMailbox);
            }
        }
        (_, true) if mailbox.search_results().is_some() => {
            centered(ui, "Proton found no mail matching that.");
        }
        (_, true) if mailbox.is_searching() => {
            centered(ui, "No matches in the conversations loaded so far.");
            detail(ui, "Press Enter to search all of your mail.");
            load_more(ui, mailbox, messages);
        }
        (_, true) => centered(
            ui,
            &format!("No conversations in {}.", mailbox.folder().name()),
        ),
        _ => conversation_list(ui, mailbox, state, messages),
    }
}

fn conversation_list(
    ui: &mut egui::Ui,
    mailbox: &Mailbox,
    state: &mut UiState,
    messages: &mut Vec<Message>,
) {
    let now = Local::now();
    let selected = mailbox.selected_conversation();
    egui::ScrollArea::vertical()
        .id_salt("conversation-scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for conversation in mailbox.visible_conversations() {
                let response = ui
                    .push_id(&conversation.id, |ui| {
                        conversation_row(
                            ui,
                            conversation,
                            selected == Some(conversation.id.as_str()),
                            &now,
                        )
                    })
                    .inner;
                if state.reveal_conversation.as_deref() == Some(conversation.id.as_str()) {
                    response.scroll_to_me(Some(Align::Center));
                    state.reveal_conversation = None;
                }
                if response.clicked() {
                    messages.push(Message::SelectConversation(conversation.id.clone()));
                }
                ui.add_space(2.0);
            }
            load_more(ui, mailbox, messages);
        });
}

fn conversation_row(
    ui: &mut egui::Ui,
    conversation: &ConversationSummary,
    selected: bool,
    now: &DateTime<Local>,
) -> egui::Response {
    let correspondents = conversation
        .correspondents
        .as_deref()
        .unwrap_or("(Unknown)");
    let subject = conversation.subject.as_deref().unwrap_or("(No subject)");
    let time = conversation
        .time
        .map(|time| format_time(time, now))
        .unwrap_or_default();
    let fill = if selected {
        theme::ACCENT_SOFT
    } else {
        Color32::TRANSPARENT
    };
    let stroke = if selected {
        Stroke::new(1.0, theme::ACCENT)
    } else {
        Stroke::NONE
    };

    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 54.0), Sense::click());
    let hovered_fill = if response.hovered() && !selected {
        Color32::from_rgb(28, 29, 36)
    } else {
        fill
    };
    ui.painter().rect(
        rect,
        CornerRadius::same(8),
        hovered_fill,
        stroke,
        egui::StrokeKind::Inside,
    );

    if conversation.unread {
        // On the sender line: at the row's centre it reads as belonging to
        // neither line.
        ui.painter().circle_filled(
            egui::pos2(rect.left() + 7.0, rect.top() + 13.0),
            3.5,
            theme::ACCENT,
        );
    }

    let meta_width = 70.0;
    let content_rect = egui::Rect::from_min_max(
        egui::pos2(rect.left() + 18.0, rect.top() + 5.0),
        egui::pos2(rect.right() - meta_width - 8.0, rect.bottom() - 5.0),
    );
    let painter = ui.painter_at(rect);
    theme::paint_truncated_text(
        &painter,
        content_rect.left_top(),
        correspondents,
        FontId::proportional(14.0),
        ui.visuals().strong_text_color(),
        content_rect.width(),
    );
    theme::paint_truncated_text(
        &painter,
        content_rect.left_top() + egui::vec2(0.0, 25.0),
        subject,
        FontId::proportional(14.0),
        theme::MUTED,
        content_rect.width(),
    );

    let meta_font = FontId::proportional(11.0);
    painter.text(
        egui::pos2(rect.right() - 8.0, rect.top() + 7.0),
        Align2::RIGHT_TOP,
        time,
        meta_font.clone(),
        theme::MUTED,
    );

    let mut facts = Vec::new();
    if conversation.message_count > 1 {
        facts.push(conversation.message_count.to_string());
    }
    if conversation.starred {
        facts.push(STAR.to_owned());
    }
    let bottom = rect.bottom() - 7.0;
    let facts_left = painter
        .text(
            egui::pos2(rect.right() - 8.0, bottom),
            Align2::RIGHT_BOTTOM,
            facts.join(" · "),
            meta_font,
            theme::MUTED,
        )
        .left();
    if conversation.has_attachments {
        theme::paint_clip(
            &painter,
            egui::pos2(facts_left - 9.0, bottom - 6.75),
            theme::MUTED,
        );
    }

    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            true,
            format!("{correspondents}: {subject}"),
        )
    });
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn load_more(ui: &mut egui::Ui, mailbox: &Mailbox, messages: &mut Vec<Message>) {
    if matches!(mailbox.status(), ListStatus::LoadingMore(_)) {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Loading more…");
        });
    } else if mailbox.has_more()
        && ui
            .add_enabled(!mailbox.is_busy(), egui::Button::new("Load more"))
            .clicked()
    {
        messages.push(Message::LoadMoreConversations);
    }
}

fn centered(ui: &mut egui::Ui, text: &str) {
    ui.add_space(24.0);
    ui.vertical_centered(|ui| {
        ui.label(egui::RichText::new(text).color(theme::MUTED));
    });
}

pub(super) fn detail(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).small().color(theme::MUTED));
}

fn search_scope_label(results: Option<&str>, loaded: usize) -> String {
    match (results, loaded) {
        (Some(query), _) => format!("Showing what the server found for “{query}”."),
        (None, 1) => {
            "Narrowing the 1 conversation loaded so far. Press Enter to search all mail.".to_owned()
        }
        (None, loaded) => format!(
            "Narrowing the {loaded} conversations loaded so far. Press Enter to search all mail."
        ),
    }
}

fn format_time<Tz: TimeZone>(timestamp: i64, now: &DateTime<Tz>) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let Some(time) = now.timezone().timestamp_opt(timestamp, 0).single() else {
        return String::new();
    };
    let format = if time.date_naive() == now.date_naive() {
        "%H:%M"
    } else if time.year() == now.year() {
        "%b %-d"
    } else {
        "%Y-%m-%d"
    };
    time.format(format).to_string()
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;

    #[test]
    fn search_scope_says_what_the_list_is_showing() {
        let narrowing = search_scope_label(None, 50);
        assert!(narrowing.contains("50 conversations loaded so far"));
        assert!(narrowing.contains("Enter"));
        assert!(search_scope_label(None, 1).contains("1 conversation loaded"));
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
}
