use chrono::{DateTime, Datelike, Local, TimeZone};
use eframe::egui::AtomExt as _;
use eframe::egui::{self, Align, Align2, Color32, CornerRadius, FontId, Layout, Sense, Stroke};

use super::{UiState, UndoNotice, compose, reader, theme};
use crate::app::{App, ListStatus, Mailbox, Message, UndoMove, panel_ratios, panel_widths};
use crate::mail::{ConversationSummary, CustomKind, Folder, MailFolder};
use crate::settings::ComposePlacement;

pub(super) fn show(
    root: &mut egui::Ui,
    app: &App,
    email: Option<&str>,
    signing_out: bool,
    state: &mut UiState,
    messages: &mut Vec<Message>,
) {
    let mailbox = app.mailbox().expect("authenticated mailbox");
    let context = root.ctx().clone();
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
            if app.settings().compose_placement == ComposePlacement::ReadingPane
                && let Some(writing) = app.compose()
            {
                compose::show_in_pane(ui, writing, messages);
            } else {
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
            }
        });

    undo_notice(&context, mailbox, state, messages);

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

    if new_message_button(ui).clicked() {
        messages.push(Message::OpenCompose);
    }
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

const NEW_MESSAGE_LABEL_ID: &str = "new-message-label";
const FOLDER_NAME_ATOM_ID: &str = "folder-name";
const SELECTED_FOLDER_TEXT_OFFSET: f32 = 0.45;

fn new_message_button(ui: &mut egui::Ui) -> egui::AtomLayoutResponse {
    let width = ui.available_width();
    ui.scope(|ui| {
        ui.visuals_mut().widgets.inactive.weak_bg_fill = theme::ACCENT;
        ui.visuals_mut().widgets.hovered.weak_bg_fill = theme::ACCENT_HOVER;
        ui.visuals_mut().widgets.active.weak_bg_fill = theme::ACCENT_HOVER;
        egui::Button::new((
            egui::Atom::grow(),
            egui::RichText::new("New message")
                .size(15.0)
                .color(Color32::WHITE)
                .atom_id(egui::Id::new(NEW_MESSAGE_LABEL_ID)),
            egui::Atom::grow(),
        ))
        .min_size(egui::vec2(width, 34.0))
        .stroke(Stroke::NONE)
        .atom_ui(ui)
    })
    .inner
}

fn folder_button(
    ui: &mut egui::Ui,
    mailbox: &Mailbox,
    folder: Folder,
    messages: &mut Vec<Message>,
) {
    let selected = &folder == mailbox.folder();
    let icon = folder_icon(&folder);
    let unread = mailbox
        .counts()
        .and_then(|counts| counts.unread(&folder))
        .filter(|count| *count > 0);
    let name = if selected {
        egui::RichText::new(folder.name()).color(Color32::WHITE)
    } else {
        egui::RichText::new(folder.name())
    }
    .atom_id(egui::Id::new(FOLDER_NAME_ATOM_ID));
    let mut button = egui::Button::new((theme::icon_atom(), name))
        .gap(ui.spacing().icon_spacing + 3.0)
        .wrap_mode(egui::TextWrapMode::Truncate)
        .min_size(egui::vec2(ui.available_width(), FOLDER_HEIGHT))
        .selected(selected)
        .frame(true)
        .frame_when_inactive(selected);
    if let Some(count) = unread {
        button = button.right_text(count.to_string());
    }
    if selected {
        button = button.fill(theme::ACCENT_SOFT).stroke(Stroke::NONE);
    }
    let layout = button.atom_ui(ui);
    let icon_color = if icon == theme::Icon::Label {
        theme::ACCENT
    } else if selected {
        Color32::WHITE
    } else {
        ui.style().interact(&layout.response).fg_stroke.color
    };
    theme::paint_atom_icon(ui, &layout, icon, icon_color);
    if selected && let Some(rect) = layout.rect(egui::Id::new(FOLDER_NAME_ATOM_ID)) {
        let painter = ui.painter().with_clip_rect(rect);
        theme::paint_truncated_text_aligned(
            &painter,
            rect.left_center() + egui::vec2(SELECTED_FOLDER_TEXT_OFFSET, 0.0),
            Align2::LEFT_CENTER,
            folder.name(),
            egui::TextStyle::Button.resolve(ui.style()),
            Color32::WHITE,
            rect.width() - SELECTED_FOLDER_TEXT_OFFSET,
        );
    }
    let response = layout.response;
    let name = folder.name().to_owned();
    response.widget_info(|| {
        let label = unread.map_or_else(|| name.clone(), |count| format!("{name}, {count} unread"));
        egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, label)
    });
    if response.clicked() {
        messages.push(Message::SelectFolder(folder));
    }
}

fn folder_icon(folder: &Folder) -> theme::Icon {
    match folder {
        Folder::System(MailFolder::Inbox) => theme::Icon::Inbox,
        Folder::System(MailFolder::Drafts) => theme::Icon::Drafts,
        Folder::System(MailFolder::Sent) => theme::Icon::Sent,
        Folder::System(MailFolder::Starred) => theme::Icon::Star,
        Folder::System(MailFolder::Archive) => theme::Icon::Archive,
        Folder::System(MailFolder::Spam) => theme::Icon::Spam,
        Folder::System(MailFolder::Trash) => theme::Icon::Trash,
        Folder::Custom {
            kind: CustomKind::Folder,
            ..
        } => theme::Icon::Folder,
        Folder::Custom {
            kind: CustomKind::Label,
            ..
        } => theme::Icon::Label,
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
            theme::text_field(&mut query)
                .id(egui::Id::new("mail-search"))
                .hint_text("Search mail…")
                .margin(egui::Margin::symmetric(ROW_HORIZONTAL_PADDING as i8, 8))
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
    ui.add_space(4.0);

    let empty = mailbox.visible_conversations().next().is_none();
    match (mailbox.status(), empty) {
        (ListStatus::Loading(_), _) => conversation_skeleton(ui),
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

const UNDO_NOTICE_SECONDS: f64 = 6.0;

fn undo_notice(
    context: &egui::Context,
    mailbox: &Mailbox,
    state: &mut UiState,
    messages: &mut Vec<Message>,
) {
    let Some(offer) = mailbox.undo() else {
        state.undo_notice = None;
        return;
    };
    let now = context.input(|input| input.time);
    let Some(remaining) = sync_undo_notice(state, offer, now) else {
        messages.push(Message::DismissUndo(offer.clone()));
        return;
    };
    context.request_repaint_after(remaining);

    let _ = egui::Area::new(egui::Id::new("undo-move-notice"))
        .anchor(Align2::CENTER_BOTTOM, egui::vec2(0.0, -20.0))
        .order(egui::Order::Foreground)
        .show(context, |ui| {
            theme::card()
                .stroke(Stroke::new(1.0, theme::ACCENT))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(format!("Moved to {}.", offer.to.name()));
                        if ui.add(theme::compact_button("Undo")).clicked() {
                            messages.push(Message::UndoMove);
                        }
                    });
                });
        });
}

fn sync_undo_notice(
    state: &mut UiState,
    offer: &UndoMove,
    now: f64,
) -> Option<std::time::Duration> {
    if state
        .undo_notice
        .as_ref()
        .is_none_or(|notice| notice.offer != *offer)
    {
        state.undo_notice = Some(UndoNotice {
            offer: offer.clone(),
            expires_at: now + UNDO_NOTICE_SECONDS,
        });
    }
    let remaining = state.undo_notice.as_ref()?.expires_at - now;
    (remaining > 0.0).then(|| std::time::Duration::from_secs_f64(remaining))
}

/// Virtualized rows must draw at [`ROW_HEIGHT`] to stay aligned with scrolling.
const FOLDER_HEIGHT: f32 = 31.0;
const ROW_HEIGHT: f32 = 60.0;
const ROW_GAP: f32 = 10.0;
const ROW_HORIZONTAL_PADDING: f32 = 18.0;
const ROW_LINE_CENTER_OFFSET: f32 = 12.5;

fn conversation_list(
    ui: &mut egui::Ui,
    mailbox: &Mailbox,
    state: &mut UiState,
    messages: &mut Vec<Message>,
) {
    let now = Local::now();
    let selected = mailbox.selected_conversation();
    // Index visible rows once for virtualized access.
    let rows: Vec<&ConversationSummary> = mailbox.visible_conversations().collect();
    let total = rows.len() + usize::from(ends_with_load_more(mailbox));

    ui.scope(|ui| {
        // Read by `show_rows` to work out where each row goes, so it is set
        // on this `Ui` and not inside the closure.
        ui.spacing_mut().item_spacing.y = ROW_GAP;
        ui.spacing_mut().scroll = theme::panel_scroll_style();
        let scroll = egui::ScrollArea::vertical()
            .id_salt("conversation-scroll")
            .auto_shrink([false, false]);

        // Synthesized rectangles can reveal rows outside the rendered range.
        let reveal = state
            .reveal_conversation
            .as_deref()
            .and_then(|wanted| rows.iter().position(|row| row.id == wanted));

        scroll.show_rows(ui, ROW_HEIGHT, total, |ui, range| {
            if let Some(index) = reveal {
                let row_step = ROW_HEIGHT + ROW_GAP;
                let content_top = ui.max_rect().top() - range.start as f32 * row_step;
                let row_rect = egui::Rect::from_min_size(
                    egui::pos2(ui.max_rect().left(), content_top + index as f32 * row_step),
                    egui::vec2(ui.available_width(), ROW_HEIGHT),
                );
                ui.scroll_to_rect(row_rect, Some(egui::Align::Center));
            }

            for index in range {
                let Some(conversation) = rows.get(index) else {
                    load_more(ui, mailbox, messages);
                    continue;
                };
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
                if response.clicked() {
                    messages.push(Message::SelectConversation(conversation.id.clone()));
                }
            }
        });
        if reveal.is_some() {
            state.reveal_conversation = None;
        }
    });
}

/// Whether the virtualized list needs a trailing load-more row.
fn ends_with_load_more(mailbox: &Mailbox) -> bool {
    matches!(mailbox.status(), ListStatus::LoadingMore(_)) || mailbox.has_more()
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
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, ROW_HEIGHT), Sense::click());
    let hovered_fill = if response.hovered() && !selected {
        Color32::from_rgb(28, 29, 36)
    } else {
        fill
    };
    ui.painter().rect(
        rect,
        CornerRadius::same(8),
        hovered_fill,
        Stroke::NONE,
        egui::StrokeKind::Inside,
    );

    let sender_center_y = rect.center().y - ROW_LINE_CENTER_OFFSET;
    let subject_center_y = rect.center().y + ROW_LINE_CENTER_OFFSET;

    if conversation.unread {
        // On the sender line: at the row's centre it reads as belonging to
        // neither line.
        ui.painter().circle_filled(
            egui::pos2(rect.left() + 7.0, sender_center_y),
            3.5,
            theme::ACCENT,
        );
    }

    let meta_width = 70.0;
    let text_left = rect.left() + ROW_HORIZONTAL_PADDING;
    let text_width = rect.right() - ROW_HORIZONTAL_PADDING - meta_width - text_left;
    let painter = ui.painter_at(rect);
    theme::paint_truncated_text_aligned(
        &painter,
        egui::pos2(text_left, sender_center_y),
        Align2::LEFT_CENTER,
        correspondents,
        FontId::proportional(14.0),
        ui.visuals().strong_text_color(),
        text_width,
    );
    theme::paint_truncated_text_aligned(
        &painter,
        egui::pos2(text_left, subject_center_y),
        Align2::LEFT_CENTER,
        subject,
        FontId::proportional(14.0),
        theme::MUTED,
        text_width,
    );

    let meta_font = FontId::proportional(11.0);
    painter.text(
        egui::pos2(rect.right() - ROW_HORIZONTAL_PADDING, sender_center_y),
        Align2::RIGHT_CENTER,
        time,
        meta_font.clone(),
        theme::MUTED,
    );

    let mut facts = Vec::new();
    if conversation.message_count > 1 {
        facts.push(conversation.message_count.to_string());
    }
    if conversation.starred {
        facts.push(theme::STAR.to_owned());
    }
    let facts_left = painter
        .text(
            egui::pos2(rect.right() - ROW_HORIZONTAL_PADDING, subject_center_y),
            Align2::RIGHT_CENTER,
            facts.join(&format!(" {} ", theme::DOT)),
            meta_font,
            theme::MUTED,
        )
        .left();
    if conversation.has_attachments {
        theme::paint_clip(
            &painter,
            egui::pos2(facts_left - 9.0, subject_center_y),
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

fn conversation_skeleton(ui: &mut egui::Ui) {
    let fill = theme::skeleton_fill(ui);
    let available = ui.available_height();
    let rows = (((available + ROW_GAP) / (ROW_HEIGHT + ROW_GAP)).floor() as usize).clamp(1, 12);
    let response = ui
        .scope(|ui| {
            ui.spacing_mut().item_spacing.y = ROW_GAP;
            for index in 0..rows {
                skeleton_conversation_row(ui, index, fill);
            }
        })
        .response;
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::ProgressIndicator,
            true,
            "Loading conversations",
        )
    });
}

fn skeleton_conversation_row(ui: &mut egui::Ui, index: usize, fill: Color32) -> egui::Response {
    const SENDER_WIDTHS: [f32; 4] = [0.62, 0.48, 0.72, 0.56];
    const SUBJECT_WIDTHS: [f32; 4] = [0.44, 0.66, 0.52, 0.36];

    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, ROW_HEIGHT), Sense::hover());
    let painter = ui.painter_at(rect);
    let sender_center_y = rect.center().y - ROW_LINE_CENTER_OFFSET;
    let subject_center_y = rect.center().y + ROW_LINE_CENTER_OFFSET;
    let text_left = rect.left() + ROW_HORIZONTAL_PADDING;
    let text_right = rect.right() - ROW_HORIZONTAL_PADDING - 70.0;
    let text_width = (text_right - text_left).max(24.0);
    let bar = |center_y: f32, width: f32, height: f32| {
        theme::paint_skeleton(
            &painter,
            egui::Rect::from_min_size(
                egui::pos2(text_left, center_y - height * 0.5),
                egui::vec2(width, height),
            ),
            fill,
        );
    };
    bar(
        sender_center_y,
        text_width * SENDER_WIDTHS[index % SENDER_WIDTHS.len()],
        11.0,
    );
    bar(
        subject_center_y,
        text_width * SUBJECT_WIDTHS[index % SUBJECT_WIDTHS.len()],
        9.0,
    );
    theme::paint_skeleton(
        &painter,
        egui::Rect::from_center_size(
            egui::pos2(
                rect.right() - ROW_HORIZONTAL_PADDING - 18.0,
                sender_center_y,
            ),
            egui::vec2(36.0, 8.0),
        ),
        fill,
    );

    response
}

fn load_more(ui: &mut egui::Ui, mailbox: &Mailbox, messages: &mut Vec<Message>) {
    if matches!(mailbox.status(), ListStatus::LoadingMore(_)) {
        let fill = theme::skeleton_fill(ui);
        let response = skeleton_conversation_row(ui, 0, fill);
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::ProgressIndicator,
                true,
                "Loading more conversations",
            )
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
                folder_button(ui, &mailbox, Folder::INBOX, &mut Vec::new());
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
    fn an_undo_notice_expires_and_a_new_one_gets_its_own_time() {
        let offer = UndoMove {
            row_id: "first".into(),
            kind: crate::mail::SummaryKind::Conversation,
            from: Folder::INBOX,
            to: Folder::System(MailFolder::Trash),
        };
        let mut state = UiState::default();

        assert_eq!(
            sync_undo_notice(&mut state, &offer, 10.0),
            Some(std::time::Duration::from_secs(6))
        );
        assert_eq!(
            sync_undo_notice(&mut state, &offer, 15.0),
            Some(std::time::Duration::from_secs(1))
        );
        assert_eq!(sync_undo_notice(&mut state, &offer, 16.0), None);

        let next = UndoMove {
            row_id: "second".into(),
            ..offer
        };
        assert_eq!(
            sync_undo_notice(&mut state, &next, 16.0),
            Some(std::time::Duration::from_secs(6))
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
                folder_button(ui, &mailbox, label.clone(), &mut Vec::new());
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
}
