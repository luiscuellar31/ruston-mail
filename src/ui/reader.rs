use std::collections::HashSet;
use std::path::Path;

use chrono::{Local, TimeZone};
use eframe::egui::text::LayoutJob;
use eframe::egui::{self, Align, Align2, Color32, FontId, Layout, Sense, Stroke};

use super::mailbox::{detail, folder_icon};
use super::{UiState, theme};
use crate::app::{ConversationReader, Mailbox, Message, PendingLink, ReaderState};
use crate::downloads::SaveError;
use crate::mail::{
    BlockKind, ConversationSummary, CustomKind, Folder, MailAction, MailAddress, MailAttachment,
    MailFolder, MailMessage, MessageBody, RichBlock, RichBody, RichSpan, UNREADABLE_NOTICE,
    Verdict,
};
use crate::settings::Reading;

const SUBJECT_SIZE: f32 = 22.0;
const MAX_QUOTE_DEPTH: u8 = 4;
/// Deeper list items keep this level's indent, as in the CLI, so a deeply
/// nested list cannot push its text out of the column.
const MAX_LIST_DEPTH: u8 = 8;
const BODY_SIZE: f32 = 15.0;
const READING_WIDTH: f32 = 680.0;
const REPLY_TOOLBAR_HEIGHT: f32 = 24.0;
const MARKER_WIDTH: f32 = 12.0;
const SIGNATURE_ICON_SIZE: f32 = 13.0;
const SIGNATURE_ICON_INSET: f32 = SIGNATURE_ICON_SIZE + 4.0;
/// Code background padding, kept clear of adjacent lines.
const CODE_PADDING: f32 = 2.5;

#[cfg(target_os = "macos")]
const REVEAL_LABEL: &str = "Show in Finder";
#[cfg(target_os = "windows")]
const REVEAL_LABEL: &str = "Show in Explorer";
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const REVEAL_LABEL: &str = "Show in folder";

/// What the reader draws around the open conversation, borrowed from the app
/// for one frame.
pub(super) struct ReaderView<'a> {
    pub(super) mailbox: &'a Mailbox,
    /// The folders and labels a conversation can be moved to or given.
    pub(super) places: &'a [Folder],
    pub(super) actions_available: bool,
    pub(super) pending_link: Option<&'a PendingLink>,
    pub(super) saving_attachment: Option<&'a str>,
    pub(super) saved_attachment: Option<Result<&'a Path, SaveError>>,
    pub(super) reading: Reading,
}

pub(super) fn show(
    ui: &mut egui::Ui,
    view: &ReaderView<'_>,
    state: &mut UiState,
    messages: &mut Vec<Message>,
) {
    if let Some(outcome) = &view.saved_attachment {
        match outcome {
            Ok(path) => {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new(format!("Saved to {}", path.display()))
                            .small()
                            .color(theme::colors(ui).success),
                    );
                    if ui.add(theme::compact_button(REVEAL_LABEL)).clicked() {
                        messages.push(Message::RevealAttachment(path.to_path_buf()));
                    }
                });
            }
            Err(error) => {
                ui.label(egui::RichText::new(error.message()).color(theme::colors(ui).danger));
            }
        }
        ui.separator();
    }
    if let Some(link) = view.pending_link {
        link_prompt(ui, link, messages);
        ui.add_space(8.0);
    }

    match view.mailbox.reader_state() {
        ReaderState::Empty => placeholder(ui, "Select a conversation to read it."),
        ReaderState::Loading { .. } => conversation_skeleton(ui, view, state),
        ReaderState::Failed { error, .. } => {
            theme::centered_group(ui, |ui| {
                ui.label(error.conversation_message());
                if ui.button("Retry").clicked() {
                    messages.push(Message::RetryConversation);
                }
            });
        }
        ReaderState::Loaded(reader) => conversation(ui, reader, view, state, messages),
    }
}

fn link_prompt(ui: &mut egui::Ui, link: &PendingLink, messages: &mut Vec<Message>) {
    theme::card(ui)
        .fill(theme::colors(ui).accent_soft)
        .stroke(Stroke::new(1.0, theme::ACCENT))
        .show(ui, |ui| {
            ui.label(format!("Open a link to {}?", link.target));
            // The real destination is what the prompt is for, so it is never
            // smaller than the question.
            ui.add(egui::Label::new(link.url.as_str()).wrap());
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

fn conversation(
    ui: &mut egui::Ui,
    reader: &ConversationReader,
    view: &ReaderView<'_>,
    state: &mut UiState,
    messages: &mut Vec<Message>,
) {
    reader_scroll(ui, state, |ui| reading_column(ui, reader, view, messages));
}

fn reader_scroll(ui: &mut egui::Ui, state: &mut UiState, content: impl FnOnce(&mut egui::Ui)) {
    // Keep scrolling and message expansion attached to the reader when its
    // parent changes between a side-by-side layout and a compact central pane.
    ui.scope_builder(
        egui::UiBuilder::new().id(egui::Id::new("mailbox-reader-content")),
        |ui| {
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
                        content(ui);
                    });
                });
            });
        },
    );
}

/// Centers the bounded reading column, shrinking it when necessary.
fn reading_column_place(available: f32) -> (f32, f32) {
    let width = available.min(READING_WIDTH);
    (((available - width) * 0.5).max(0.0), width)
}

fn conversation_skeleton(ui: &mut egui::Ui, view: &ReaderView<'_>, state: &mut UiState) {
    let fill = theme::skeleton_fill(ui);
    let response = ui
        .scope(|ui| {
            reader_scroll(ui, state, |ui| {
                let summary = view.mailbox.selected_summary();
                // The listing already supplies this text. Rendering it normally
                // also reserves the correct height for wrapped subjects.
                if let Some(summary) = summary {
                    conversation_heading(
                        ui,
                        summary.subject.as_deref(),
                        summary.message_count as usize,
                    );
                    if view.actions_available {
                        action_toolbar(
                            ui,
                            summary,
                            view.mailbox,
                            &[],
                            view.places,
                            false,
                            &mut Vec::new(),
                        );
                    }
                } else {
                    skeleton_label(
                        ui,
                        egui::RichText::new("Loading conversation")
                            .size(SUBJECT_SIZE)
                            .strong(),
                        fill,
                    );
                    skeleton_label(ui, egui::RichText::new("1 message").small(), fill);
                    ui.add_space(8.0);
                }
                ui.add_space(8.0);
                let expanded = view.reading.expand_all_messages
                    || summary.is_none_or(|summary| summary.message_count <= 1);
                skeleton_message_card(ui, fill, expanded);
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

fn skeleton_message_card(ui: &mut egui::Ui, fill: Color32, expanded: bool) -> egui::Response {
    theme::card(ui)
        .show(ui, |ui| {
            let width = ui.available_width();
            ui.set_min_width(width);
            let (header, _) = ui.allocate_exact_size(
                egui::vec2(width, message_header_height(expanded)),
                Sense::hover(),
            );
            paint_message_chevron(ui, header, expanded, false);
            let copy = header_copy_rect(header);
            let sender_height = ui.fonts_mut(|fonts| fonts.row_height(&FontId::proportional(14.0)));
            let detail_height = ui.fonts_mut(|fonts| fonts.row_height(&FontId::proportional(11.0)));
            theme::paint_skeleton(
                ui.painter(),
                egui::Rect::from_min_size(
                    copy.left_top()
                        + egui::vec2(if expanded { 0.0 } else { SIGNATURE_ICON_INSET }, 0.0),
                    egui::vec2(
                        (copy.width() - SIGNATURE_ICON_INSET).max(0.0) * 0.55,
                        sender_height,
                    ),
                ),
                fill,
            );
            let expanded_lines = [(20.0, 0.85, detail_height), (39.0, 0.65, detail_height)];
            let collapsed_lines = [(22.0, 0.85, sender_height)];
            let lines = if expanded {
                &expanded_lines[..]
            } else {
                &collapsed_lines[..]
            };
            for (offset, fraction, height) in lines {
                let inset = if expanded && *offset == 20.0 {
                    SIGNATURE_ICON_INSET
                } else {
                    0.0
                };
                theme::paint_skeleton(
                    ui.painter(),
                    egui::Rect::from_min_size(
                        copy.left_top() + egui::vec2(inset, *offset),
                        egui::vec2((copy.width() - inset).max(0.0) * fraction, *height),
                    ),
                    fill,
                );
            }
            let date_width = 122.0_f32.min(header.width() * 0.3);
            theme::paint_skeleton(
                ui.painter(),
                egui::Rect::from_min_size(
                    egui::pos2(header.right() - date_width, header.top() + 2.0),
                    egui::vec2(date_width, detail_height),
                ),
                fill,
            );
            if expanded {
                reply_toolbar(ui, None, &mut Vec::new());
                ui.separator();
                let line_height =
                    ui.fonts_mut(|fonts| fonts.row_height(&FontId::proportional(BODY_SIZE)));
                for fraction in [0.92, 0.76, 0.84, 0.48] {
                    skeleton_bar(ui, width * fraction, line_height, fill);
                }
            }
        })
        .response
}

fn skeleton_label(ui: &mut egui::Ui, text: egui::RichText, fill: Color32) {
    let response = ui
        .scope_builder(egui::UiBuilder::new().invisible(), |ui| ui.label(text))
        .inner;
    theme::paint_skeleton(ui.painter(), response.rect, fill);
}

fn skeleton_bar(ui: &mut egui::Ui, width: f32, height: f32, fill: Color32) -> egui::Response {
    let size = egui::vec2(width.min(ui.available_width()).max(0.0), height);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    theme::paint_skeleton(ui.painter(), rect, fill);
    response
}

fn reading_column(
    ui: &mut egui::Ui,
    reader: &ConversationReader,
    view: &ReaderView<'_>,
    messages: &mut Vec<Message>,
) {
    let mailbox = view.mailbox;
    let detail_data = reader.detail();
    conversation_heading(
        ui,
        detail_data.subject.as_deref(),
        detail_data.messages.len(),
    );

    let summary = mailbox
        .selected_summary()
        .filter(|_| view.actions_available);
    if let Some(summary) = summary {
        action_toolbar(
            ui,
            summary,
            mailbox,
            &detail_data.labels,
            view.places,
            !mailbox.is_busy() && !mailbox.action_pending(),
            messages,
        );
        if let Some(error) = mailbox.action_error() {
            ui.label(egui::RichText::new(error.action_message()).color(theme::colors(ui).danger));
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
            view.saving_attachment,
            view.reading,
            messages,
        );
        ui.add_space(12.0);
    }
}

fn conversation_heading(ui: &mut egui::Ui, subject: Option<&str>, count: usize) {
    ui.add(
        egui::Label::new(
            egui::RichText::new(subject.unwrap_or("(No subject)"))
                .size(SUBJECT_SIZE)
                .strong(),
        )
        .selectable(true)
        .wrap(),
    );
    detail(ui, &message_count_label(count));
    ui.add_space(8.0);
}

fn action_toolbar(
    ui: &mut egui::Ui,
    summary: &ConversationSummary,
    mailbox: &Mailbox,
    carried_labels: &[String],
    places: &[Folder],
    enabled: bool,
    messages: &mut Vec<Message>,
) {
    let location = mailbox.selected_location();
    ui.horizontal_wrapped(|ui| {
        let back = back_to_inbox(location)
            .map(|label| (theme::Icon::Inbox, label, MailFolder::Inbox, false));
        let away = [
            (theme::Icon::Archive, "Archive", MailFolder::Archive, false),
            (theme::Icon::Spam, "Move to Spam", MailFolder::Spam, false),
            (
                theme::Icon::Trash,
                "Delete (move to Trash)",
                MailFolder::Trash,
                true,
            ),
        ];
        for (icon, label, folder, danger) in back.into_iter().chain(away) {
            let destination = Folder::System(folder);
            if location == Some(&destination) {
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
        move_menu(
            ui,
            &move_destinations(location, mailbox.folder(), places),
            enabled,
            messages,
        );

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

        label_menu(ui, carried_labels, places, enabled, messages);
    });
}

/// Proton's Inbox, which the Move to menu lists beside the account's folders.
static INBOX: Folder = Folder::INBOX;

/// The first button for a row in Archive, Spam or Trash: the way back to the
/// Inbox, which Spam calls Not spam.
fn back_to_inbox(location: Option<&Folder>) -> Option<&'static str> {
    match location.and_then(Folder::system)? {
        MailFolder::Archive | MailFolder::Trash => Some("Move to Inbox"),
        MailFolder::Spam => Some("Not spam (move to Inbox)"),
        MailFolder::Inbox | MailFolder::Drafts | MailFolder::Sent | MailFolder::Starred => None,
    }
}

/// The places the Move to menu lists: the Inbox, unless the row is there or a
/// button already offers it, then every folder the account made except the
/// one the row is in. Sent and Drafts list the account's own mail, which
/// Proton keeps out of the Inbox.
fn move_destinations<'a>(
    location: Option<&Folder>,
    view: &Folder,
    places: &'a [Folder],
) -> Vec<&'a Folder> {
    let own_mail = matches!(view.system(), Some(MailFolder::Sent | MailFolder::Drafts));
    let inbox = location != Some(&INBOX) && back_to_inbox(location).is_none() && !own_mail;
    let folders = places
        .iter()
        .filter(|place| place.kind() == Some(CustomKind::Folder) && Some(*place) != location);

    inbox.then_some(&INBOX).into_iter().chain(folders).collect()
}

fn move_menu(
    ui: &mut egui::Ui,
    destinations: &[&Folder],
    enabled: bool,
    messages: &mut Vec<Message>,
) {
    if destinations.is_empty() {
        return;
    }
    ui.add_enabled_ui(enabled, |ui| {
        egui::containers::menu::MenuButton::new("Move to…").ui(ui, |ui| {
            ui.set_min_width(180.0);
            egui::ScrollArea::vertical()
                .max_height(240.0)
                .show(ui, |ui| {
                    for destination in destinations {
                        if destination_item(ui, destination).clicked() {
                            messages.push(Message::ApplyAction(MailAction::MoveTo(
                                (*destination).clone(),
                            )));
                            ui.close();
                        }
                    }
                });
        });
    });
}

/// One place in the Move to menu, with the icon the sidebar gives it.
fn destination_item(ui: &mut egui::Ui, folder: &Folder) -> egui::Response {
    let layout = egui::Button::new((theme::icon_atom(), folder.name())).atom_ui(ui);
    let color = ui.style().interact(&layout.response).fg_stroke.color;
    theme::paint_atom_icon(ui, &layout, folder_icon(folder), color);
    layout.response
}

fn label_menu(
    ui: &mut egui::Ui,
    carried_labels: &[String],
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
    let carried_ids: HashSet<_> = carried_labels.iter().map(String::as_str).collect();
    let applied = labels
        .iter()
        .filter(|label| label.custom_id().is_some_and(|id| carried_ids.contains(id)))
        .count();
    let title = if applied == 0 {
        "Labels".to_owned()
    } else {
        format!("Labels ({applied})")
    };

    // Counts arrive with the detail. Reserve their widest possible title now,
    // so a newly displayed count cannot wrap or move the message card.
    let max_title = egui::WidgetText::from(format!("Labels ({})", labels.len())).into_galley(
        ui,
        Some(egui::TextWrapMode::Extend),
        f32::INFINITY,
        egui::TextStyle::Button,
    );
    let button = egui::Button::new(title)
        .wrap_mode(egui::TextWrapMode::Extend)
        .min_size(egui::vec2(
            max_title.size().x + 2.0 * ui.spacing().button_padding.x,
            0.0,
        ));

    ui.separator();
    ui.add_enabled_ui(enabled, |ui| {
        egui::containers::menu::MenuButton::from_button(button).ui(ui, |ui| {
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
                            if ui
                                .add(theme::Checkbox::new(&mut on, label.name()))
                                .changed()
                            {
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
        theme::card(ui).show(ui, |ui| {
            if message_header(ui, message, preview, expanded).clicked() {
                messages.push(Message::ToggleMessageExpanded(message.id.clone()));
            }
            if expanded {
                for attachment in &message.attachments {
                    attachment_row(ui, &message.id, attachment, saving_attachment, messages);
                }
                reply_toolbar(ui, Some(&message.id), messages);
                ui.separator();
                theme::selectable_text(ui, |ui| {
                    message_body(ui, message, reader, reading, messages);
                });
            }
        });
    });
}

fn reply_toolbar(ui: &mut egui::Ui, message_id: Option<&str>, messages: &mut Vec<Message>) {
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), REPLY_TOOLBAR_HEIGHT),
        Layout::left_to_right(Align::Center).with_main_align(Align::Max),
        |ui| {
            // Core addresses replies from the message being answered.
            for (label, forward, everyone) in [
                ("Reply", false, false),
                ("Reply all", false, true),
                ("Forward", true, false),
            ] {
                if ui
                    .add_enabled(message_id.is_some(), theme::compact_button(label))
                    .clicked()
                    && let Some(message_id) = message_id
                {
                    messages.push(Message::Answer {
                        message_id: message_id.to_owned(),
                        forward,
                        everyone,
                    });
                }
            }
        },
    );
}

/// Only a verified body signature earns a closed lock, regardless of the
/// sender's email domain. Attachments have their own verification path.
fn signature_status(ui: &egui::Ui, verdict: Verdict) -> (theme::Icon, &'static str, Color32) {
    let colors = theme::colors(ui);
    match verdict {
        Verdict::Verified => (theme::Icon::Lock, "Message text verified", colors.success),
        Verdict::Unsigned => (theme::Icon::Unlock, "Message text not signed", colors.muted),
        Verdict::Unverified => (
            theme::Icon::Unlock,
            "Could not verify the message text",
            colors.muted,
        ),
        Verdict::Invalid => (
            theme::Icon::Spam,
            "Message verification failed. Use caution.",
            colors.danger,
        ),
    }
}

/// What the header says about the text: its signature, or that there is no
/// text to check.
fn message_status(ui: &egui::Ui, message: &MailMessage) -> (theme::Icon, &'static str, Color32) {
    if matches!(message.body, MessageBody::Unreadable) {
        (
            theme::Icon::Unlock,
            "Message could not be decrypted",
            theme::colors(ui).muted,
        )
    } else {
        signature_status(ui, message.verdict)
    }
}

/// The whole header is one disclosure control. Copy selection is enabled only
/// below it, so clicking a sender or preview reliably expands the message.
fn message_header(
    ui: &mut egui::Ui,
    message: &MailMessage,
    preview: &str,
    expanded: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), message_header_height(expanded)),
        Sense::click(),
    );
    paint_message_chevron(ui, rect, expanded, response.hovered());

    let copy_rect = header_copy_rect(rect);
    let painter = ui.painter_at(rect);
    let sender = message.sender.display_name().unwrap_or("(Unknown sender)");
    let named = message.sender.name.is_some() && !message.sender.address.is_empty();
    let separate_address = expanded && named;
    let identity_top =
        copy_rect.left_top() + egui::vec2(0.0, if separate_address { 20.0 } else { 0.0 });
    let icon_rect = egui::Rect::from_min_size(identity_top, egui::Vec2::splat(SIGNATURE_ICON_SIZE));
    let (icon, status, color) = message_status(ui, message);
    egui::Image::new(icon.source())
        .tint(color)
        .paint_at(ui, icon_rect);
    let hovering_icon = ui.rect_contains_pointer(icon_rect);
    let sender_inset = if separate_address {
        0.0
    } else {
        SIGNATURE_ICON_INSET
    };
    let sender_origin = copy_rect.left_top() + egui::vec2(sender_inset, 0.0);
    let sender_width = (copy_rect.width() - sender_inset).max(0.0);
    if named && !expanded {
        // The lock vouches for the address, never for the name the sender
        // chose, so a collapsed card keeps the address beside that name.
        paint_name_and_address(
            ui,
            &painter,
            sender_origin,
            sender,
            &message.sender.address,
            sender_width,
        );
    } else {
        theme::paint_truncated_text(
            &painter,
            sender_origin,
            sender,
            FontId::proportional(14.0),
            ui.visuals().strong_text_color(),
            sender_width,
        );
    }
    if expanded {
        if separate_address {
            header_detail(
                &painter,
                identity_top + egui::vec2(SIGNATURE_ICON_INSET, 0.0),
                &message.sender.address,
                theme::colors(ui).muted,
                (copy_rect.width() - SIGNATURE_ICON_INSET).max(0.0),
            );
        }
        header_detail(
            &painter,
            copy_rect.left_top() + egui::vec2(0.0, 39.0),
            &format!("To: {}", recipients_label(&message.recipients)),
            theme::colors(ui).muted,
            copy_rect.width(),
        );
    } else {
        theme::paint_truncated_text(
            &painter,
            copy_rect.left_top() + egui::vec2(0.0, 22.0),
            preview,
            FontId::proportional(14.0),
            theme::colors(ui).muted,
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
        theme::colors(ui).muted,
    );

    let identity = if named {
        format!("{sender} <{}>", message.sender.address)
    } else {
        sender.to_owned()
    };
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::CollapsingHeader,
            true,
            format!("{identity}, {status}"),
        )
    });
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    if hovering_icon {
        response.on_hover_text(status)
    } else {
        response.on_hover_text(if expanded { "Collapse" } else { "Expand" })
    }
}

/// Paints `name <address>` on one line. The address keeps up to two thirds of
/// the width, so the name is cut first.
fn paint_name_and_address(
    ui: &egui::Ui,
    painter: &egui::Painter,
    origin: egui::Pos2,
    name: &str,
    address: &str,
    width: f32,
) {
    const GAP: f32 = 6.0;
    let muted = theme::colors(ui).muted;
    let strong = ui.visuals().strong_text_color();
    let address = theme::truncated_galley(
        painter,
        &format!("<{address}>"),
        FontId::proportional(12.0),
        muted,
        width * 2.0 / 3.0,
    );
    let name = theme::truncated_galley(
        painter,
        name,
        FontId::proportional(14.0),
        strong,
        (width - address.size().x - GAP).max(0.0),
    );
    // Share the name's baseline rather than its top edge.
    let address_origin = origin
        + egui::vec2(
            name.size().x + GAP,
            (name.size().y - address.size().y).max(0.0),
        );
    painter.galley(origin, name, strong);
    painter.galley(address_origin, address, muted);
}

fn message_header_height(expanded: bool) -> f32 {
    if expanded { 66.0 } else { 44.0 }
}

fn paint_message_chevron(ui: &egui::Ui, rect: egui::Rect, expanded: bool, hovered: bool) {
    let icon_center = egui::pos2(rect.left() + 8.0, rect.top() + 11.0);
    let points = chevron_points(icon_center, expanded);
    let color = if hovered {
        ui.visuals().strong_text_color()
    } else {
        theme::colors(ui).muted
    };
    ui.painter()
        .line_segment([points[0], points[1]], Stroke::new(1.5, color));
    ui.painter()
        .line_segment([points[1], points[2]], Stroke::new(1.5, color));
}

fn header_copy_rect(rect: egui::Rect) -> egui::Rect {
    let date_width = 122.0_f32.min(rect.width() * 0.3);
    egui::Rect::from_min_max(
        egui::pos2(rect.left() + 22.0, rect.top()),
        egui::pos2(rect.right() - date_width - 8.0, rect.bottom()),
    )
}

fn header_detail(
    painter: &egui::Painter,
    position: egui::Pos2,
    text: &str,
    color: Color32,
    width: f32,
) {
    theme::paint_truncated_text(
        painter,
        position,
        text,
        FontId::proportional(11.0),
        color,
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
    ui.horizontal_wrapped(|ui| {
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
        if ui
            .add_enabled(!saving, theme::compact_button("Save as…"))
            .clicked()
        {
            messages.push(Message::SaveAttachmentAs(
                message_id.to_owned(),
                attachment.id.clone(),
                attachment.name.clone(),
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
        MessageBody::Unreadable => {
            ui.label(
                egui::RichText::new(UNREADABLE_NOTICE)
                    .size(BODY_SIZE)
                    .color(theme::colors(ui).danger),
            );
        }
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
        .fill(theme::colors(ui).quote_fill)
        .stroke(Stroke::new(1.0, theme::colors(ui).border))
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
                let indent = (*depth).min(MAX_LIST_DEPTH).saturating_sub(1);
                ui.add_space(18.0 * f32::from(indent));
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
                .fill(theme::colors(ui).code_fill)
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
                .stroke(Stroke::new(1.0, theme::colors(ui).border))
                .corner_radius(6)
                .inner_margin(10)
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(format!("Image not loaded: {description}"))
                                .size(BODY_SIZE - 2.0)
                                .color(theme::colors(ui).muted),
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
                messages.push(Message::LinkClicked(link.to_string()));
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
        format.background = theme::colors(ui).code_fill;
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
        ui.label(egui::RichText::new(message).color(theme::colors(ui).muted));
    });
}

pub(super) fn size_label(bytes: u64) -> String {
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
    use crate::mail::ConversationDetail;

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
            let (mut mailbox, request, detail) =
                loading_reader("Message", Folder::System(folder), 1);
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
                    recipients: vec![address(None, "team@example.org")],
                    reply_to: Vec::new(),
                    reply_all_cc: Vec::new(),
                    reply_redirected: false,
                    time: Some(1_791_327_600 + i64::from(index)),
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
                pending_link: None,
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

    fn text_rect(shapes: &[egui::epaint::ClippedShape], label: &str) -> egui::Rect {
        shapes
            .iter()
            .find_map(|shape| {
                if let egui::epaint::Shape::Text(text) = &shape.shape
                    && text.galley.text() == label
                {
                    Some(egui::Rect::from_min_size(text.pos, text.galley.size()))
                } else {
                    None
                }
            })
            .unwrap_or_else(|| panic!("missing text: {label}"))
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
        for label in [subject, &message_count_label(count as usize), "Labels"] {
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
        for (appearance, system_font) in [
            (crate::settings::Appearance::Dark, false),
            (crate::settings::Appearance::Light, false),
            (crate::settings::Appearance::Dark, true),
            (crate::settings::Appearance::Light, true),
        ] {
            for (width, zoom, native_scale) in [
                (540.0, 1.0, 1.5),
                (980.0, 1.0, 2.0),
                (300.0, 1.0, 1.0),
                (300.0, 1.75, 2.0),
            ] {
                let context = egui::Context::default();
                theme::install(&context);
                super::super::fonts::apply(&context, system_font);
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
        for position in [
            controls[0].center(),
            text_rect(&shapes, "Labels").center(),
            reply,
        ] {
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
            matches!(messages.as_slice(), [Message::Answer { message_id, forward: false, everyone: false }] if message_id == "message-0")
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
            matches!(messages.as_slice(), [Message::Answer { message_id, forward: false, everyone: false }] if message_id == "message-0")
        );
    }

    #[test]
    fn a_collapsed_card_shows_the_real_address_beside_the_lock() {
        let message = MailMessage {
            id: "message".into(),
            verdict: Verdict::Verified,
            sender: address(Some("PayPal <service@paypal.com>"), "phish@proton.me"),
            recipients: Vec::new(),
            reply_to: Vec::new(),
            reply_all_cc: Vec::new(),
            reply_redirected: false,
            time: None,
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

    #[test]
    fn message_header_is_one_full_width_click_target() {
        let message = MailMessage {
            id: "message".into(),
            verdict: Verdict::Unverified,
            sender: address(Some("Alex Rivera"), "alex@example.com"),
            recipients: Vec::new(),
            reply_to: Vec::new(),
            reply_all_cc: Vec::new(),
            reply_redirected: false,
            time: None,
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
        let icon_center =
            header_copy_rect(rect).left_top() + egui::Vec2::splat(SIGNATURE_ICON_SIZE / 2.0);
        for position in [icon_center, rect.center()] {
            render(egui::RawInput {
                events: vec![egui::Event::PointerMoved(position)],
                ..Default::default()
            });
            let (_, _, shapes) = render(egui::RawInput {
                events: vec![egui::Event::PointerMoved(position)],
                ..Default::default()
            });
            if position == icon_center {
                let labels: Vec<_> = shapes
                    .iter()
                    .filter_map(|shape| match &shape.shape {
                        egui::epaint::Shape::Text(text) => Some(text.galley.text()),
                        _ => None,
                    })
                    .collect();
                assert!(
                    labels.contains(&"Could not verify the message text"),
                    "hovering the icon must explain the verdict: {labels:?}"
                );
            }
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
                recipients: vec![address(None, "team@example.org")],
                reply_to: Vec::new(),
                reply_all_cc: Vec::new(),
                reply_redirected: false,
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

    #[test]
    fn body_verdict_is_visible_in_expanded_and_collapsed_message_cards() {
        for (verdict, expected_icon, label) in [
            (
                Verdict::Verified,
                theme::Icon::Lock,
                "Message text verified",
            ),
            (
                Verdict::Unsigned,
                theme::Icon::Unlock,
                "Message text not signed",
            ),
            (
                Verdict::Unverified,
                theme::Icon::Unlock,
                "Could not verify the message text",
            ),
            (
                Verdict::Invalid,
                theme::Icon::Spam,
                "Message verification failed. Use caution.",
            ),
        ] {
            for appearance in [
                crate::settings::Appearance::Dark,
                crate::settings::Appearance::Light,
            ] {
                for name in [None, Some("Sender")] {
                    for expanded in [false, true] {
                        let message = MailMessage {
                            id: "message".into(),
                            sender: address(
                                name,
                                if verdict == Verdict::Verified {
                                    "sender@example.org"
                                } else {
                                    "sender@proton.me"
                                },
                            ),
                            recipients: Vec::new(),
                            reply_to: Vec::new(),
                            reply_all_cc: Vec::new(),
                            reply_redirected: false,
                            time: None,
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
                        assert_eq!(reader.is_expanded("message", Reading::default()), expanded);
                        let context = egui::Context::default();
                        theme::install(&context);
                        theme::apply(&context, appearance);
                        let mut color = Color32::TRANSPARENT;
                        let mut draw = || {
                            context.run_ui(egui::RawInput::default(), |ui| {
                                let colors = theme::colors(ui);
                                color = match verdict {
                                    Verdict::Invalid => colors.danger,
                                    Verdict::Verified => colors.success,
                                    _ => colors.muted,
                                };
                                let (icon, status, actual_color) = signature_status(ui, verdict);
                                assert_eq!(icon, expected_icon);
                                assert_eq!(status, label);
                                assert_eq!(actual_color, color);
                                message_card(
                                    ui,
                                    &reader.detail().messages[0],
                                    reader.preview_at(0),
                                    &reader,
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
                        let texture = egui::Image::new(expected_icon.source())
                            .source(&context)
                            .clone()
                            .load(
                                &context,
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
                        let image = shapes
                            .iter()
                            .find_map(|shape| match &shape.shape {
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
                            .unwrap_or_else(|| panic!("missing {label}, expanded={expanded}"));
                        assert_eq!(image.fill, color);
                        let identity = text_rect(
                            &shapes,
                            if expanded && name.is_some() {
                                &reader.detail().messages[0].sender.address
                            } else {
                                reader.detail().messages[0].sender.display_name().unwrap()
                            },
                        );
                        let icon_rect = image.rect;
                        assert!(icon_rect.right() < identity.left(), "icon overlaps address");
                        assert!((icon_rect.top() - identity.top()).abs() <= 0.5);
                        assert!((icon_rect.width() - SIGNATURE_ICON_SIZE).abs() <= 0.5);
                        assert!(
                            !shapes.iter().any(|shape| matches!(&shape.shape,
                                egui::epaint::Shape::Text(text) if text.galley.text() == label
                            )),
                            "status should be in the tooltip, not a separate row"
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
                    pending_link: None,
                    saving_attachment: None,
                    saved_attachment: Some(Ok(&path)),
                    reading: Reading::default(),
                };
                show(ui, &view, &mut UiState::default(), &mut messages);
            })
            .drop_without_applying_deltas();

        assert!(messages.is_empty());
    }
}
