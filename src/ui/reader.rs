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
const PREVIEW_WORDS: usize = 40;
const LABELS_SHOWN: usize = 8;
const MAX_QUOTE_DEPTH: u8 = 4;
const BODY_SIZE: f32 = 15.0;
const READING_WIDTH: f32 = 680.0;
const MARKER_WIDTH: f32 = 12.0;
const CODE_FILL: Color32 = Color32::from_rgb(18, 19, 24);
/// `expand_bg` grows a code run's fill on every side, so it buys sideways
/// breathing room at the cost of height. Small enough not to reach the line
/// above or below.
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
        ReaderState::Loading { .. } => {
            theme::centered_group(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Loading conversation…");
                });
            });
        }
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
    summary: Option<&ConversationSummary>,
    actions_enabled: bool,
    action_error: Option<crate::mail::MailboxError>,
    saving_attachment: Option<&str>,
    reading: Reading,
    state: &mut UiState,
    messages: &mut Vec<Message>,
) {
    let mut scroll = egui::ScrollArea::vertical()
        .id_salt("reader-scroll")
        .auto_shrink([false, false]);
    if std::mem::take(&mut state.scroll_reader_top) {
        scroll = scroll.vertical_scroll_offset(0.0);
    }
    scroll.show(ui, |ui| {
        // Mail is read in a column of its own width. Once the panel outgrows
        // that, the column is centred rather than left hugging the divider.
        let width = ui.available_width().min(READING_WIDTH);
        ui.horizontal_top(|ui| {
            ui.add_space(((ui.available_width() - width) * 0.5).max(0.0));
            ui.vertical(|ui| {
                ui.set_width(width);
                reading_column(
                    ui,
                    reader,
                    places,
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
}

#[allow(clippy::too_many_arguments)]
fn reading_column(
    ui: &mut egui::Ui,
    reader: &ConversationReader,
    places: &[Folder],
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
        action_toolbar(ui, summary, actions_enabled, messages);
        label_toggles(
            ui,
            detail_data,
            places,
            actions_enabled,
            reader.is_showing_labels(),
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
    for message in &detail_data.messages {
        message_card(ui, message, reader, saving_attachment, reading, messages);
        ui.add_space(12.0);
    }
}

fn action_toolbar(
    ui: &mut egui::Ui,
    summary: &ConversationSummary,
    enabled: bool,
    messages: &mut Vec<Message>,
) {
    let read = if summary.unread {
        ("Mark read", MailAction::SetUnread(false))
    } else {
        ("Mark unread", MailAction::SetUnread(true))
    };
    let star = if summary.starred {
        ("Unstar", MailAction::SetStarred(false))
    } else {
        ("Star", MailAction::SetStarred(true))
    };
    let actions = [
        (
            "Archive",
            MailAction::MoveTo(Folder::System(MailFolder::Archive)),
        ),
        ("Spam", MailAction::MoveTo(Folder::System(MailFolder::Spam))),
        (
            "Trash",
            MailAction::MoveTo(Folder::System(MailFolder::Trash)),
        ),
        read,
        star,
    ];
    ui.horizontal_wrapped(|ui| {
        for (label, action) in actions {
            if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                messages.push(Message::ApplyAction(action));
            }
        }
    });
}

fn label_toggles(
    ui: &mut egui::Ui,
    conversation: &ConversationDetail,
    places: &[Folder],
    enabled: bool,
    showing_all: bool,
    messages: &mut Vec<Message>,
) {
    let labels: Vec<_> = places
        .iter()
        .filter(|place| place.kind() == Some(CustomKind::Label))
        .collect();
    if labels.is_empty() {
        return;
    }
    let crowded = labels.len() > LABELS_SHOWN;
    ui.horizontal_wrapped(|ui| {
        for label in labels
            .iter()
            .copied()
            .filter(|label| shows_label(conversation.carries(label), crowded, showing_all))
        {
            let carried = conversation.carries(label);
            let button = egui::Button::new(label.name()).selected(carried).small();
            if ui.add_enabled(enabled, button).clicked() {
                messages.push(Message::ApplyAction(MailAction::SetLabel {
                    label: label.clone(),
                    on: !carried,
                }));
            }
        }
        if crowded
            && ui
                .small_button(if showing_all {
                    "Fewer labels".to_owned()
                } else {
                    format!("All {} labels", labels.len())
                })
                .clicked()
        {
            messages.push(Message::ToggleLabelsShown);
        }
    });
}

fn shows_label(carried: bool, crowded: bool, showing_all: bool) -> bool {
    carried || !crowded || showing_all
}

fn message_card(
    ui: &mut egui::Ui,
    message: &MailMessage,
    reader: &ConversationReader,
    saving_attachment: Option<&str>,
    reading: Reading,
    messages: &mut Vec<Message>,
) {
    let expanded = reader.is_expanded(&message.id, reading);
    ui.push_id(&message.id, |ui| {
        theme::card().show(ui, |ui| {
            if message_header(ui, message, expanded).clicked() {
                messages.push(Message::ToggleMessageExpanded(message.id.clone()));
            }
            if expanded {
                for attachment in &message.attachments {
                    attachment_row(ui, &message.id, attachment, saving_attachment, messages);
                }
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
fn message_header(ui: &mut egui::Ui, message: &MailMessage, expanded: bool) -> egui::Response {
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
            &preview(&message.body),
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
                egui::Button::new(if this_one { "Saving…" } else { "Save" }).small(),
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
                .small_button(if expanded {
                    "Hide quoted text"
                } else {
                    "Show quoted text"
                })
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
                // The marker gets a column of its own, so a wrapped item
                // continues under its own text rather than under the marker.
                // It stays a label so that it copies out with the item.
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

/// One span's styling, as a laid-out text job.
///
/// Built as a `TextFormat` rather than a `RichText` for the sake of
/// `expand_bg`, which is the only way to widen the fill behind a code run:
/// `RichText` pins it at 1.0, which leaves the background hugging the glyphs.
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
            // Monospace runs wider and taller than the proportional face at
            // the same point size, so it is set a little smaller to sit on
            // the same line as the words around it.
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

fn preview(body: &MessageBody) -> String {
    // A rich body is flattened before it is split into words, not after:
    // splitting each span on its own strands the punctuation that follows a
    // styled run, which read as "confirmed . The" in the collapsed header.
    let text = match body {
        MessageBody::PlainText(content) => content.clone(),
        MessageBody::Rich(rich) => rich.plain_text(),
    };
    text.split_whitespace()
        .take(PREVIEW_WORDS)
        .collect::<Vec<_>>()
        .join(" ")
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
    fn a_crowded_label_row_keeps_only_what_the_conversation_carries() {
        assert!(shows_label(false, false, false));
        assert!(shows_label(true, false, false));
        assert!(shows_label(true, true, false));
        assert!(!shows_label(false, true, false));
        assert!(shows_label(false, true, true));
    }

    fn address(name: Option<&str>, email: &str) -> MailAddress {
        MailAddress {
            name: name.map(str::to_owned),
            address: email.to_owned(),
        }
    }

    fn paragraph(text: &str) -> RichBlock {
        RichBlock {
            kind: BlockKind::Paragraph(vec![RichSpan {
                text: text.to_owned(),
                ..RichSpan::default()
            }]),
            quote_depth: 0,
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
    fn preview_is_a_single_bounded_line() {
        let body = MessageBody::PlainText("Hi Alex,\n\nFirst line.\nSecond line.".into());
        let long = MessageBody::PlainText("word ".repeat(100));
        let rich = MessageBody::Rich(RichBody {
            blocks: vec![paragraph("Hello  there"), paragraph("again")],
        });
        assert_eq!(preview(&body), "Hi Alex, First line. Second line.");
        assert_eq!(preview(&long).split(' ').count(), PREVIEW_WORDS);
        assert_eq!(preview(&rich), "Hello there again");
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
                    let response = message_header(ui, &message, false);
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
    fn disclosure_chevron_changes_direction_without_a_font_glyph() {
        let center = egui::pos2(10.0, 10.0);
        let closed = chevron_points(center, false);
        let open = chevron_points(center, true);

        assert!(closed[1].x > closed[0].x && closed[1].x > closed[2].x);
        assert!(open[1].y > open[0].y && open[1].y > open[2].y);
    }
}
