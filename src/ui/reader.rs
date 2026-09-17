use std::collections::HashSet;
use std::path::Path;

use chrono::{Local, TimeZone};
use eframe::egui::text::LayoutJob;
use eframe::egui::{self, Align, Align2, Color32, FontId, Layout, Sense, Stroke};

use super::mailbox::detail;
use super::{UiState, theme};
use crate::app::{ConversationReader, Mailbox, Message, PendingLink, ReaderState};
use crate::downloads::SaveError;
use crate::mail::{
    BlockKind, ConversationDetail, ConversationSummary, CustomKind, Folder, MailAction,
    MailAddress, MailAttachment, MailFolder, MailMessage, MessageBody, RichBlock, RichBody,
    RichSpan,
};
use crate::settings::Reading;

const SUBJECT_SIZE: f32 = 22.0;
const MAX_QUOTE_DEPTH: u8 = 4;
const BODY_SIZE: f32 = 15.0;
const READING_WIDTH: f32 = 680.0;
const REPLY_TOOLBAR_HEIGHT: f32 = 24.0;
const MARKER_WIDTH: f32 = 12.0;
const CODE_FILL: Color32 = Color32::from_rgb(18, 19, 24);
/// Code background padding, kept clear of adjacent lines.
const CODE_PADDING: f32 = 2.5;

#[allow(clippy::too_many_arguments)]
pub(super) fn show(
    ui: &mut egui::Ui,
    mailbox: &Mailbox,
    places: &[Folder],
    actions_available: bool,
    pending_link: Option<&PendingLink>,
    saving_attachment: Option<&str>,
    saved_attachment: Option<Result<&Path, SaveError>>,
    reading: Reading,
    state: &mut UiState,
    messages: &mut Vec<Message>,
) {
    if let Some(outcome) = saved_attachment {
        let (text, color) = match outcome {
            Ok(path) => (format!("Saved to {}", path.display()), theme::SUCCESS),
            Err(error) => (error.message().to_owned(), theme::DANGER),
        };
        ui.label(egui::RichText::new(text).small().color(color));
        ui.separator();
    }
    if let Some(link) = pending_link {
        link_prompt(ui, link, messages);
        ui.add_space(8.0);
    }

    match mailbox.reader_state() {
        ReaderState::Empty => placeholder(ui, "Select a conversation to read it."),
        ReaderState::Loading { .. } => conversation_skeleton(ui),
        ReaderState::Failed { error, .. } => {
            theme::centered_group(ui, |ui| {
                ui.label(error.conversation_message());
                if ui.button("Retry").clicked() {
                    messages.push(Message::RetryConversation);
                }
            });
        }
        ReaderState::Loaded(reader) => {
            conversation(
                ui,
                reader,
                places,
                mailbox.folder(),
                mailbox.selected_summary().filter(|_| actions_available),
                !mailbox.is_busy() && !mailbox.action_pending(),
                mailbox.action_error(),
                saving_attachment,
                reading,
                state,
                messages,
            );
        }
    }
}

fn link_prompt(ui: &mut egui::Ui, link: &PendingLink, messages: &mut Vec<Message>) {
    theme::card()
        .fill(theme::ACCENT_SOFT)
        .stroke(Stroke::new(1.0, theme::ACCENT))
        .show(ui, |ui| {
            ui.label(format!("Open a link to {}?", link.target));
            ui.add(egui::Label::new(egui::RichText::new(&link.url).small()).wrap());
            ui.horizontal(|ui| {
                if ui.button("Open").clicked() {
                    messages.push(Message::OpenLink);
                }
                if ui.button("Copy link").clicked() {
                    messages.push(Message::CopyLink);
                }
                if ui.button("Cancel").clicked() {
                    messages.push(Message::DismissLink);
                }
            });
        });
}

#[allow(clippy::too_many_arguments)]
fn conversation(
    ui: &mut egui::Ui,
    reader: &ConversationReader,
    places: &[Folder],
    current_folder: &Folder,
    summary: Option<&ConversationSummary>,
    actions_enabled: bool,
    action_error: Option<crate::mail::MailboxError>,
    saving_attachment: Option<&str>,
    reading: Reading,
    state: &mut UiState,
    messages: &mut Vec<Message>,
) {
    ui.scope(|ui| {
        ui.spacing_mut().scroll = theme::panel_scroll_style();
        let mut scroll = egui::ScrollArea::vertical()
            .id_salt("reader-scroll")
            .auto_shrink([false, false]);
        if std::mem::take(&mut state.scroll_reader_top) {
            scroll = scroll.vertical_scroll_offset(0.0);
        }
        scroll.show(ui, |ui| {
            let (gap, width) = reading_column_place(ui.available_width());
            ui.horizontal_top(|ui| {
                ui.add_space(gap);
                ui.vertical(|ui| {
                    ui.set_width(width);
                    reading_column(
                        ui,
                        reader,
                        places,
                        current_folder,
                        summary,
                        actions_enabled,
                        action_error,
                        saving_attachment,
                        reading,
                        messages,
                    );
                });
            });
        });
    });
}

/// Centers the bounded reading column, shrinking it when necessary.
fn reading_column_place(available: f32) -> (f32, f32) {
    let width = available.min(READING_WIDTH);
    (((available - width) * 0.5).max(0.0), width)
}

fn conversation_skeleton(ui: &mut egui::Ui) {
    let fill = theme::skeleton_fill(ui);
    let response = ui
        .scope(|ui| {
            let (gap, width) = reading_column_place(ui.available_width());
            ui.horizontal_top(|ui| {
                ui.add_space(gap);
                ui.vertical(|ui| {
                    ui.set_width(width);
                    skeleton_bar(ui, width * 0.72, 24.0, fill);
                    skeleton_bar(ui, 52.0, 8.0, fill);
                    ui.add_space(8.0);
                    ui.horizontal_wrapped(|ui| {
                        for _ in 0..3 {
                            skeleton_bar(
                                ui,
                                theme::ICON_BUTTON_MIN_SIZE.x,
                                theme::ICON_BUTTON_MIN_SIZE.y,
                                fill,
                            );
                        }
                        ui.separator();
                        for _ in 0..2 {
                            skeleton_bar(
                                ui,
                                theme::ICON_BUTTON_MIN_SIZE.x,
                                theme::ICON_BUTTON_MIN_SIZE.y,
                                fill,
                            );
                        }
                        ui.separator();
                        skeleton_bar(ui, 60.0, theme::ICON_BUTTON_MIN_SIZE.y, fill);
                    });
                    ui.add_space(8.0);
                    skeleton_message_card(ui, fill);
                });
            });
        })
        .response;
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::ProgressIndicator,
            true,
            "Loading conversation",
        )
    });
}

fn skeleton_message_card(ui: &mut egui::Ui, fill: Color32) -> egui::Response {
    theme::card()
        .show(ui, |ui| {
            let width = ui.available_width();
            ui.set_min_width(width);
            skeleton_bar(ui, width * 0.38, 12.0, fill);
            skeleton_bar(ui, width * 0.24, 8.0, fill);
            ui.add_space(12.0);
            for fraction in [0.92, 0.76, 0.84, 0.48] {
                skeleton_bar(ui, width * fraction, 9.0, fill);
            }
        })
        .response
}

fn skeleton_bar(ui: &mut egui::Ui, width: f32, height: f32, fill: Color32) -> egui::Response {
    let size = egui::vec2(width.min(ui.available_width()).max(0.0), height);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    theme::paint_skeleton(ui.painter(), rect, fill);
    response
}

#[allow(clippy::too_many_arguments)]
fn reading_column(
    ui: &mut egui::Ui,
    reader: &ConversationReader,
    places: &[Folder],
    current_folder: &Folder,
    summary: Option<&ConversationSummary>,
    actions_enabled: bool,
    action_error: Option<crate::mail::MailboxError>,
    saving_attachment: Option<&str>,
    reading: Reading,
    messages: &mut Vec<Message>,
) {
    let detail_data = reader.detail();
    ui.add(
        egui::Label::new(
            egui::RichText::new(detail_data.subject.as_deref().unwrap_or("(No subject)"))
                .size(SUBJECT_SIZE)
                .strong(),
        )
        .selectable(true)
        .wrap(),
    );
    detail(ui, &message_count_label(detail_data.messages.len()));
    ui.add_space(8.0);

    if let Some(summary) = summary {
        action_toolbar(
            ui,
            summary,
            current_folder,
            detail_data,
            places,
            actions_enabled,
            messages,
        );
        if let Some(error) = action_error {
            ui.label(
                egui::RichText::new(error.action_message())
                    .small()
                    .color(theme::DANGER),
            );
        }
    }
    ui.add_space(8.0);

    if detail_data.messages.is_empty() {
        detail(ui, "This conversation has no messages.");
    }
    for (index, message) in detail_data.messages.iter().enumerate() {
        message_card(
            ui,
            message,
            reader.preview_at(index),
            reader,
            saving_attachment,
            reading,
            messages,
        );
        ui.add_space(12.0);
    }
}

fn action_toolbar(
    ui: &mut egui::Ui,
    summary: &ConversationSummary,
    current_folder: &Folder,
    conversation: &ConversationDetail,
    places: &[Folder],
    enabled: bool,
    messages: &mut Vec<Message>,
) {
    ui.horizontal_wrapped(|ui| {
        for (icon, label, folder, danger) in [
            (theme::Icon::Archive, "Archive", MailFolder::Archive, false),
            (theme::Icon::Spam, "Move to Spam", MailFolder::Spam, false),
            (
                theme::Icon::Trash,
                "Delete (move to Trash)",
                MailFolder::Trash,
                true,
            ),
        ] {
            let destination = Folder::System(folder);
            if current_folder == &destination {
                continue;
            }
            let mut button = theme::IconButton::new(icon, label);
            if danger {
                button = button.danger();
            }
            let response = ui.add_enabled(enabled, button);
            if response.clicked() {
                messages.push(Message::ApplyAction(MailAction::MoveTo(destination)));
            }
        }

        ui.separator();
        for (icon, label, selected, action) in [
            (
                theme::Icon::Mail,
                if summary.unread {
                    "Mark as read"
                } else {
                    "Mark as unread"
                },
                summary.unread,
                MailAction::SetUnread(!summary.unread),
            ),
            (
                theme::Icon::Star,
                if summary.starred {
                    "Remove star"
                } else {
                    "Add star"
                },
                summary.starred,
                MailAction::SetStarred(!summary.starred),
            ),
        ] {
            if ui
                .add_enabled(
                    enabled,
                    theme::IconButton::new(icon, label).selected(selected),
                )
                .clicked()
            {
                messages.push(Message::ApplyAction(action));
            }
        }

        label_menu(ui, conversation, places, enabled, messages);
    });
}

fn label_menu(
    ui: &mut egui::Ui,
    conversation: &ConversationDetail,
    places: &[Folder],
    enabled: bool,
    messages: &mut Vec<Message>,
) {
    let labels: Vec<_> = places
        .iter()
        .filter(|place| place.kind() == Some(CustomKind::Label))
        .collect();
    if labels.is_empty() {
        return;
    }
    let carried_ids: HashSet<_> = conversation.labels.iter().map(String::as_str).collect();
    let applied = labels
        .iter()
        .filter(|label| label.custom_id().is_some_and(|id| carried_ids.contains(id)))
        .count();
    let title = if applied == 0 {
        "Labels".to_owned()
    } else {
        format!("Labels ({applied})")
    };

    ui.separator();
    ui.add_enabled_ui(enabled, |ui| {
        ui.menu_button(title, |ui| {
            ui.set_min_width(180.0);
            egui::ScrollArea::vertical()
                .max_height(240.0)
                .show(ui, |ui| {
                    for label in labels {
                        ui.horizontal(|ui| {
                            let (dot, _) = ui.allocate_exact_size(
                                egui::vec2(10.0, ui.spacing().interact_size.y),
                                Sense::hover(),
                            );
                            ui.painter().circle_filled(dot.center(), 3.5, theme::ACCENT);
                            let carried =
                                label.custom_id().is_some_and(|id| carried_ids.contains(id));
                            let mut on = carried;
                            if ui.checkbox(&mut on, label.name()).changed() {
                                messages.push(Message::ApplyAction(MailAction::SetLabel {
                                    label: label.clone(),
                                    on: !carried,
                                }));
                                ui.close();
                            }
                        });
                    }
                });
        });
    });
}

fn message_card(
    ui: &mut egui::Ui,
    message: &MailMessage,
    preview: &str,
    reader: &ConversationReader,
    saving_attachment: Option<&str>,
    reading: Reading,
    messages: &mut Vec<Message>,
) {
    let expanded = reader.is_expanded(&message.id, reading);
    ui.push_id(&message.id, |ui| {
        theme::card().show(ui, |ui| {
            if message_header(ui, message, preview, expanded).clicked() {
                messages.push(Message::ToggleMessageExpanded(message.id.clone()));
            }
            if expanded {
                for attachment in &message.attachments {
                    attachment_row(ui, &message.id, attachment, saving_attachment, messages);
                }
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), REPLY_TOOLBAR_HEIGHT),
                    Layout::left_to_right(Align::Center).with_main_align(Align::Max),
                    |ui| {
                        // Proton determines reply recipients from the message.
                        for (label, forward, everyone) in [
                            ("Reply", false, false),
                            ("Reply all", false, true),
                            ("Forward", true, false),
                        ] {
                            let mut button = theme::compact_button(label);
                            if label == "Reply" {
                                button = button
                                    .fill(theme::ACCENT_SOFT)
                                    .stroke(Stroke::new(1.0, theme::ACCENT));
                            }
                            if ui.add(button).clicked() {
                                messages.push(Message::Answer {
                                    message_id: message.id.clone(),
                                    forward,
                                    everyone,
                                });
                            }
                        }
                    },
                );
                ui.separator();
                theme::selectable_text(ui, |ui| {
                    message_body(ui, message, reader, reading, messages);
                });
            }
        });
    });
}

/// The whole header is one disclosure control. Copy selection is enabled only
/// below it, so clicking a sender or preview reliably expands the message.
fn message_header(
    ui: &mut egui::Ui,
    message: &MailMessage,
    preview: &str,
    expanded: bool,
) -> egui::Response {
    let height = if expanded { 66.0 } else { 44.0 };
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), height), Sense::click());
    let icon_center = egui::pos2(rect.left() + 8.0, rect.top() + 11.0);
    let points = chevron_points(icon_center, expanded);
    let color = if response.hovered() {
        Color32::WHITE
    } else {
        theme::MUTED
    };
    ui.painter()
        .line_segment([points[0], points[1]], Stroke::new(1.5, color));
    ui.painter()
        .line_segment([points[1], points[2]], Stroke::new(1.5, color));

    let date_width = 122.0_f32.min(rect.width() * 0.3);
    let copy_rect = egui::Rect::from_min_max(
        egui::pos2(rect.left() + 22.0, rect.top()),
        egui::pos2(rect.right() - date_width - 8.0, rect.bottom()),
    );
    let painter = ui.painter_at(rect);
    let sender = message.sender.display_name().unwrap_or("(Unknown sender)");
    theme::paint_truncated_text(
        &painter,
        copy_rect.left_top(),
        sender,
        FontId::proportional(14.0),
        ui.visuals().strong_text_color(),
        copy_rect.width(),
    );
    if expanded {
        if message.sender.name.is_some() && !message.sender.address.is_empty() {
            header_detail(
                &painter,
                copy_rect.left_top() + egui::vec2(0.0, 20.0),
                &message.sender.address,
                copy_rect.width(),
            );
        }
        header_detail(
            &painter,
            copy_rect.left_top() + egui::vec2(0.0, 39.0),
            &format!("To: {}", recipients_label(&message.recipients)),
            copy_rect.width(),
        );
    } else {
        theme::paint_truncated_text(
            &painter,
            copy_rect.left_top() + egui::vec2(0.0, 22.0),
            preview,
            FontId::proportional(14.0),
            theme::MUTED,
            copy_rect.width(),
        );
    }

    let time = message
        .time
        .map(|time| format_message_time(time, &Local))
        .unwrap_or_default();
    painter.text(
        egui::pos2(rect.right(), rect.top() + 2.0),
        Align2::RIGHT_TOP,
        time,
        FontId::proportional(11.0),
        theme::MUTED,
    );

    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::CollapsingHeader, true, sender)
    });
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(if expanded { "Collapse" } else { "Expand" })
}

fn header_detail(painter: &egui::Painter, position: egui::Pos2, text: &str, width: f32) {
    theme::paint_truncated_text(
        painter,
        position,
        text,
        FontId::proportional(11.0),
        theme::MUTED,
        width,
    );
}

fn chevron_points(center: egui::Pos2, expanded: bool) -> [egui::Pos2; 3] {
    if expanded {
        [
            center + egui::vec2(-4.0, -2.0),
            center + egui::vec2(0.0, 2.0),
            center + egui::vec2(4.0, -2.0),
        ]
    } else {
        [
            center + egui::vec2(-2.0, -4.0),
            center + egui::vec2(2.0, 0.0),
            center + egui::vec2(-2.0, 4.0),
        ]
    }
}

fn attachment_row(
    ui: &mut egui::Ui,
    message_id: &str,
    attachment: &MailAttachment,
    saving_attachment: Option<&str>,
    messages: &mut Vec<Message>,
) {
    let saving = saving_attachment.is_some();
    let this_one = saving_attachment == Some(attachment.id.as_str());
    ui.horizontal(|ui| {
        detail(ui, &format!("Attachment: {}", attachment.name));
        detail(ui, &size_label(attachment.size));
        if ui
            .add_enabled(
                !saving,
                theme::compact_button(if this_one { "Saving…" } else { "Save" }),
            )
            .clicked()
        {
            messages.push(Message::SaveAttachment(
                message_id.to_owned(),
                attachment.id.clone(),
            ));
        }
    });
}

fn message_body(
    ui: &mut egui::Ui,
    message: &MailMessage,
    reader: &ConversationReader,
    reading: Reading,
    messages: &mut Vec<Message>,
) {
    match &message.body {
        MessageBody::PlainText(content) => {
            ui.add(
                egui::Label::new(egui::RichText::new(content).size(BODY_SIZE))
                    .selectable(true)
                    .wrap(),
            );
        }
        MessageBody::Rich(body) => rich_body(ui, body, &message.id, reader, reading, messages),
    }
}

fn rich_body(
    ui: &mut egui::Ui,
    body: &RichBody,
    message_id: &str,
    reader: &ConversationReader,
    reading: Reading,
    messages: &mut Vec<Message>,
) {
    if body.blocks.is_empty() {
        detail(ui, "This message has no text.");
        return;
    }
    if body.blocks.iter().all(|block| block.quote_depth > 0) {
        blocks(ui, &body.blocks, 0, messages);
        return;
    }

    let mut rest = &body.blocks[..];
    let mut quote = 0;
    while let Some(first) = rest.first() {
        if first.quote_depth > 0 {
            let run = rest
                .iter()
                .take_while(|block| block.quote_depth > 0)
                .count();
            let expanded = reader.is_quote_expanded(message_id, quote, reading);
            if ui
                .add(theme::compact_button(if expanded {
                    "Hide quoted text"
                } else {
                    "Show quoted text"
                }))
                .clicked()
            {
                messages.push(Message::ToggleQuoteExpanded(message_id.to_owned(), quote));
            }
            if expanded {
                quote_box(ui, |ui| blocks(ui, &rest[..run], 1, messages));
            }
            quote += 1;
            rest = &rest[run..];
        } else {
            block(ui, first, messages);
            rest = &rest[1..];
        }
        ui.add_space(8.0);
    }
}

fn blocks(ui: &mut egui::Ui, rich_blocks: &[RichBlock], depth: u8, messages: &mut Vec<Message>) {
    let mut rest = rich_blocks;
    while let Some(first) = rest.first() {
        if first.quote_depth > depth && depth < MAX_QUOTE_DEPTH {
            let run = rest
                .iter()
                .take_while(|block| block.quote_depth > depth)
                .count();
            quote_box(ui, |ui| blocks(ui, &rest[..run], depth + 1, messages));
            rest = &rest[run..];
        } else {
            block(ui, first, messages);
            rest = &rest[1..];
        }
        ui.add_space(8.0);
    }
}

fn quote_box(ui: &mut egui::Ui, content: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(Color32::from_rgb(27, 28, 35))
        .stroke(Stroke::new(1.0, theme::BORDER))
        .inner_margin(10)
        .show(ui, content);
}

fn block(ui: &mut egui::Ui, block: &RichBlock, messages: &mut Vec<Message>) {
    match &block.kind {
        BlockKind::Paragraph(spans) => rich_line(ui, spans, None, messages),
        BlockKind::Heading { level, spans } => rich_line(ui, spans, Some(*level), messages),
        BlockKind::ListItem {
            marker,
            depth,
            spans,
        } => {
            ui.horizontal_top(|ui| {
                ui.add_space(18.0 * f32::from(depth.saturating_sub(1)));
                // A selectable marker column keeps wrapped text aligned.
                ui.allocate_ui_with_layout(
                    egui::vec2(MARKER_WIDTH, 0.0),
                    Layout::right_to_left(Align::TOP),
                    |ui| {
                        ui.add(
                            egui::Label::new(egui::RichText::new(marker).size(BODY_SIZE))
                                .selectable(true),
                        );
                    },
                );
                ui.vertical(|ui| rich_line(ui, spans, None, messages));
            });
        }
        BlockKind::Preformatted(content) => {
            egui::Frame::new()
                .fill(CODE_FILL)
                .corner_radius(6)
                .inner_margin(10)
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(egui::RichText::new(content).monospace().size(BODY_SIZE))
                            .selectable(true)
                            .wrap(),
                    );
                });
        }
        BlockKind::Image { description } => {
            // Remote images are never fetched; the description stands in for
            // one, as a placeholder rather than a footnote.
            egui::Frame::new()
                .stroke(Stroke::new(1.0, theme::BORDER))
                .corner_radius(6)
                .inner_margin(10)
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(format!("Image not loaded: {description}"))
                                .size(BODY_SIZE - 2.0)
                                .color(theme::MUTED),
                        )
                        .selectable(true)
                        .wrap(),
                    );
                });
        }
        BlockKind::Rule => {
            ui.separator();
        }
    }
}

fn rich_line(
    ui: &mut egui::Ui,
    spans: &[RichSpan],
    heading: Option<u8>,
    messages: &mut Vec<Message>,
) {
    ui.horizontal_wrapped(|ui| rich_spans(ui, spans, heading, messages));
}

fn rich_spans(
    ui: &mut egui::Ui,
    spans: &[RichSpan],
    heading: Option<u8>,
    messages: &mut Vec<Message>,
) {
    ui.spacing_mut().item_spacing.x = 0.0;
    for span in spans {
        let job = span_layout(ui, span, heading);
        if let Some(link) = &span.link {
            if ui.add(egui::Link::new(job)).clicked() {
                messages.push(Message::LinkClicked(link.clone()));
            }
        } else {
            ui.add(egui::Label::new(job).selectable(true).wrap());
        }
    }
}

/// Builds span styling; `TextFormat` allows padded code backgrounds.
fn span_layout(ui: &egui::Ui, span: &RichSpan, heading: Option<u8>) -> LayoutJob {
    let size = heading.map(heading_size).unwrap_or(BODY_SIZE);
    // egui's bundled faces have no bold, so weight reads as a brighter ink.
    let ink = if span.strong || heading.is_some() {
        ui.visuals().strong_text_color()
    } else {
        ui.visuals().text_color()
    };
    let mut format = egui::TextFormat {
        font_id: if span.code {
            // Slightly smaller monospace aligns with surrounding text.
            FontId::monospace(size - 1.5)
        } else {
            FontId::proportional(size)
        },
        // A link's colour is the one thing left to `Link`, which fills in
        // the placeholder with the hyperlink colour and underlines on hover.
        color: if span.link.is_some() {
            Color32::PLACEHOLDER
        } else {
            ink
        },
        italics: span.emphasis,
        ..Default::default()
    };
    if span.code {
        format.background = CODE_FILL;
        format.expand_bg = CODE_PADDING;
    }
    if span.struck {
        let rule = if span.link.is_some() {
            ui.visuals().hyperlink_color
        } else {
            ink
        };
        format.strikethrough = Stroke::new(1.0, rule);
    }
    LayoutJob::single_section(span.text.clone(), format)
}

fn heading_size(level: u8) -> f32 {
    match level {
        1 => 24.0,
        2 => 20.0,
        _ => 17.0,
    }
}

fn placeholder(ui: &mut egui::Ui, message: &str) {
    ui.centered_and_justified(|ui| {
        ui.label(egui::RichText::new(message).color(theme::MUTED));
    });
}

fn size_label(bytes: u64) -> String {
    const UNIT: f64 = 1024.0;
    let bytes = bytes as f64;
    for (limit, suffix) in [
        (UNIT, "KB"),
        (UNIT * UNIT, "MB"),
        (UNIT * UNIT * UNIT, "GB"),
    ] {
        if bytes < limit * UNIT {
            let value = bytes / limit;
            return if value < 10.0 {
                format!("{value:.1} {suffix}")
            } else {
                format!("{value:.0} {suffix}")
            };
        }
    }
    format!("{:.0} GB", bytes / (UNIT * UNIT * UNIT))
}

fn message_count_label(count: usize) -> String {
    match count {
        0 => "No messages".to_owned(),
        1 => "1 message".to_owned(),
        count => format!("{count} messages"),
    }
}

fn address_label(address: &MailAddress) -> String {
    let name = address.name.as_deref().filter(|name| !name.is_empty());
    match (name, address.address.as_str()) {
        (Some(name), "") => name.to_owned(),
        (Some(name), email) => format!("{name} <{email}>"),
        (None, "") => "(unknown)".to_owned(),
        (None, email) => email.to_owned(),
    }
}

fn recipients_label(recipients: &[MailAddress]) -> String {
    if recipients.is_empty() {
        return "(no recipients)".to_owned();
    }
    recipients
        .iter()
        .map(address_label)
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_message_time<Tz: TimeZone>(timestamp: i64, zone: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    zone.timestamp_opt(timestamp, 0)
        .single()
        .map(|time| time.format("%b %-d, %Y at %H:%M").to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;

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
        let summary = ConversationSummary {
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
        let conversation = ConversationDetail {
            id: summary.id.clone(),
            subject: None,
            labels: vec!["work".into()],
            messages: Vec::new(),
        };
        let places = [Folder::label("work", "Work")];
        let context = egui::Context::default();
        let mut height = 0.0;

        context
            .run_ui(egui::RawInput::default(), |ui| {
                ui.set_width(680.0);
                action_toolbar(
                    ui,
                    &summary,
                    &Folder::INBOX,
                    &conversation,
                    &places,
                    true,
                    &mut Vec::new(),
                );
                height = ui.min_rect().height();
            })
            .drop_without_applying_deltas();

        assert!(height <= 32.0, "toolbar wrapped to {height} points");
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
                    rect = skeleton_message_card(ui, Color32::GRAY).rect;
                },
            )
            .drop_without_applying_deltas();

        assert!((rect.width() - READING_WIDTH).abs() < 0.1);
        assert!(
            rect.height() > 100.0,
            "loading message card lost its expanded body: {rect:?}"
        );
    }

    #[test]
    fn message_header_is_one_full_width_click_target() {
        let message = MailMessage {
            id: "message".into(),
            sender: address(Some("Alex Rivera"), "alex@example.com"),
            recipients: Vec::new(),
            time: None,
            body: MessageBody::PlainText("Preview".into()),
            attachments: Vec::new(),
        };
        let context = egui::Context::default();
        let render = |input| {
            let mut result = (egui::Rect::NOTHING, false);
            context
                .run_ui(input, |ui| {
                    ui.set_width(320.0);
                    let response = message_header(ui, &message, "Preview", false);
                    result = (response.rect, response.clicked());
                })
                .drop_without_applying_deltas();
            result
        };

        let (rect, _) = render(egui::RawInput::default());
        assert_eq!(rect.width(), 320.0);
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
    fn an_expanded_message_card_only_uses_its_content_height() {
        let detail = ConversationDetail {
            id: "conversation".into(),
            subject: Some("Short message".into()),
            labels: Vec::new(),
            messages: vec![MailMessage {
                id: "message".into(),
                sender: address(Some("Alex Rivera"), "alex@example.com"),
                recipients: vec![address(None, "team@example.org")],
                time: None,
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
    fn disclosure_chevron_changes_direction_without_a_font_glyph() {
        let center = egui::pos2(10.0, 10.0);
        let closed = chevron_points(center, false);
        let open = chevron_points(center, true);

        assert!(closed[1].x > closed[0].x && closed[1].x > closed[2].x);
        assert!(open[1].y > open[0].y && open[1].y > open[2].y);
    }
}
