use std::collections::HashSet;
use std::path::Path;

use chrono::{Local, TimeZone};
use eframe::egui::text::LayoutJob;
use eframe::egui::{self, Align, Color32, FontId, Layout, Sense, Stroke};

use super::mailbox::{detail, folder_icon};
use super::{UiState, theme};
use crate::app::{AnswerKind, ConversationReader, Mailbox, Message, PendingLink, ReaderState};
use crate::downloads::SaveError;
use crate::mail::{
    BlockKind, ConversationSummary, CustomKind, Folder, MailAction, MailAddress, MailAttachment,
    MailFolder, MailMessage, MessageBody, RichBlock, RichBody, RichSpan, UNREADABLE_NOTICE,
    Verdict,
};
use crate::settings::Reading;

const SUBJECT_SIZE: f32 = 22.0;
/// A sender's address beside their name.
const ADDRESS_SIZE: f32 = 12.0;
const MAX_QUOTE_DEPTH: u8 = 4;
/// Deeper list items keep this level's indent, as in the CLI, so a deeply
/// nested list cannot push its text out of the column.
const MAX_LIST_DEPTH: u8 = 8;
const READING_WIDTH: f32 = 680.0;
const REPLY_TOOLBAR_HEIGHT: f32 = 24.0;
const MARKER_WIDTH: f32 = 12.0;
const SIGNATURE_ICON_SIZE: f32 = 13.0;
const SIGNATURE_ICON_INSET: f32 = SIGNATURE_ICON_SIZE + 4.0;
/// The column left of a message header's text, which holds the chevron.
const HEADER_TEXT_INSET: f32 = 22.0;
/// The space kept between a header's text and its date.
const HEADER_DATE_GAP: f32 = 8.0;
/// About six lines of a link's address before it scrolls.
const LINK_ADDRESS_MAX_HEIGHT: f32 = 120.0;
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
    match view.mailbox.reader_state() {
        ReaderState::Empty => placeholder(ui, "Select a conversation to read it."),
        ReaderState::Loading { .. } => conversation_skeleton(ui, view, state),
        ReaderState::Failed { error, .. } => {
            theme::centered_group(ui, |ui| {
                ui.label(
                    egui::RichText::new(error.conversation_message())
                        .color(theme::colors(ui).danger),
                );
                if ui.button("Try again").clicked() {
                    messages.push(Message::RetryConversation);
                }
            });
        }
        ReaderState::Loaded(reader) => conversation(ui, reader, view, state, messages),
    }
}

/// Asks before a link leaves for the browser or the composer. It is the
/// window's question, so it shows wherever the reader is, and the address
/// scrolls inside it, so a long one cannot push the buttons away.
pub(super) fn link_dialog(context: &egui::Context, link: &PendingLink) -> Option<Message> {
    theme::confirmation_with(
        context,
        "link-prompt",
        &format!("Open a link to {}?", link.target),
        |ui| {
            // The real destination is what the prompt is for, so it is never
            // smaller than the question.
            egui::ScrollArea::vertical()
                .max_height(LINK_ADDRESS_MAX_HEIGHT)
                .show(ui, |ui| ui.add(egui::Label::new(link.url.as_str()).wrap()));
        },
        &[
            ("Cancel", theme::Emphasis::Secondary, Message::DismissLink),
            ("Copy link", theme::Emphasis::Secondary, Message::CopyLink),
            ("Open", theme::Emphasis::Primary, Message::OpenLink),
        ],
        Message::DismissLink,
    )
}

fn conversation(
    ui: &mut egui::Ui,
    reader: &ConversationReader,
    view: &ReaderView<'_>,
    state: &mut UiState,
    messages: &mut Vec<Message>,
) {
    let landing = std::mem::take(&mut state.land_reader)
        .then(|| reader.landing())
        .flatten();
    reader_scroll(ui, state, |ui| {
        reading_column(ui, reader, view, landing, messages);
    });
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
                        disabled_action_toolbar(ui, summary, view.mailbox, view.places);
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
            // A loading card does not know the sender yet, so it keeps room
            // for a name and an address of its own.
            let (header, _) = ui.allocate_exact_size(
                egui::vec2(
                    width,
                    message_header_height(expanded, SenderLine::NameOverAddress, 1),
                ),
                Sense::hover(),
            );
            paint_message_chevron(ui, header, expanded, None);
            let date_width = 122.0_f32.min(header.width() * 0.3);
            let copy = header_copy_rect(header, date_width);
            let sender_height =
                ui.fonts_mut(|fonts| fonts.row_height(&FontId::proportional(theme::ROW_TEXT_SIZE)));
            let detail_height =
                ui.fonts_mut(|fonts| fonts.row_height(&FontId::proportional(theme::FOOTNOTE_SIZE)));
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
            let expanded_lines = [
                (20.0, 0.85, detail_height),
                (
                    recipients_top(SenderLine::NameOverAddress),
                    0.65,
                    detail_height,
                ),
            ];
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
                    ui.fonts_mut(|fonts| fonts.row_height(&FontId::proportional(theme::BODY_SIZE)));
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

/// The open conversation. `landing` is the message to bring into view, on
/// the pass after the conversation opens.
fn reading_column(
    ui: &mut egui::Ui,
    reader: &ConversationReader,
    view: &ReaderView<'_>,
    landing: Option<&str>,
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
            // An action asked for while another is on its way waits its turn.
            mailbox.takes_actions(),
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
        let top = ui.cursor().top();
        message_card(
            ui,
            message,
            reader.preview_at(index),
            reader,
            view.saving_attachment,
            view.reading,
            messages,
        );
        if landing == Some(message.id.as_str()) {
            land_on(ui, top);
        }
        ui.add_space(12.0);
    }
}

/// Brings a card that starts at `top` to the top of the reader, unless it
/// already starts in the upper third, where a short thread keeps its heading
/// in view.
fn land_on(ui: &egui::Ui, top: f32) {
    let visible = ui.clip_rect();
    if top <= visible.top() + visible.height() / 3.0 {
        return;
    }
    let card = egui::Rect::from_x_y_ranges(ui.max_rect().x_range(), top..=ui.cursor().top());
    ui.scroll_to_rect_animation(card, Some(Align::TOP), egui::style::ScrollAnimation::none());
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

/// The toolbar with every action disabled, holding its place while the
/// conversation loads.
fn disabled_action_toolbar(
    ui: &mut egui::Ui,
    summary: &ConversationSummary,
    mailbox: &Mailbox,
    places: &[Folder],
) {
    action_toolbar(ui, summary, mailbox, &[], places, false, &mut Vec::new());
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
        for MoveButton {
            icon,
            label,
            destination,
            risky,
        } in moves(location)
        {
            let mut button = theme::IconButton::new(icon, label);
            if risky {
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
        for FlagButton {
            icon,
            label,
            on,
            action,
        } in flags(summary)
        {
            if ui
                .add_enabled(enabled, theme::IconButton::new(icon, label).selected(on))
                .clicked()
            {
                messages.push(Message::ApplyAction(action));
            }
        }

        label_menu(ui, carried_labels, places, enabled, messages);
    });
}

/// A move out of where a conversation is, as a button and a menu item.
struct MoveButton {
    icon: theme::Icon,
    label: &'static str,
    destination: Folder,
    /// The move that deletes, drawn as a warning.
    risky: bool,
}

/// A flag a conversation can be switched to, as a button and a menu item.
struct FlagButton {
    icon: theme::Icon,
    label: &'static str,
    /// Whether the flag is on now.
    on: bool,
    /// The action that switches it.
    action: MailAction,
}

/// The Move to and label menus: wide enough for a folder's name, and they
/// scroll past this height.
const MENU_MIN_WIDTH: f32 = 180.0;
const MENU_MAX_HEIGHT: f32 = 240.0;

/// Proton's Inbox, which the Move to menu lists beside the account's folders.
static INBOX: Folder = Folder::INBOX;

/// The moves out of where a conversation is, as its buttons in the reader and
/// its menu in the list offer them: the way back to the Inbox from Archive,
/// Spam or Trash, then Archive, Spam and Trash, except where it already is.
fn moves(location: Option<&Folder>) -> Vec<MoveButton> {
    let back =
        back_to_inbox(location).map(|label| (theme::Icon::Inbox, label, MailFolder::Inbox, false));
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
    back.into_iter()
        .chain(away)
        .map(|(icon, label, folder, risky)| MoveButton {
            icon,
            label,
            destination: Folder::System(folder),
            risky,
        })
        .filter(|button| location != Some(&button.destination))
        .collect()
}

/// The read and star flags a conversation can be switched to.
fn flags(summary: &ConversationSummary) -> [FlagButton; 2] {
    [
        FlagButton {
            icon: theme::Icon::Mail,
            label: if summary.unread {
                "Mark as read"
            } else {
                "Mark as unread"
            },
            on: summary.unread,
            action: MailAction::SetUnread(!summary.unread),
        },
        FlagButton {
            icon: theme::Icon::Star,
            label: if summary.starred {
                "Remove star"
            } else {
                "Add star"
            },
            on: summary.starred,
            action: MailAction::SetStarred(!summary.starred),
        },
    ]
}

/// The menu a right click opens on a conversation in the list: the moves and
/// flags the reader offers, for that conversation, without opening it.
pub(super) fn conversation_menu(
    ui: &mut egui::Ui,
    mailbox: &Mailbox,
    row: &ConversationSummary,
    messages: &mut Vec<Message>,
) {
    ui.set_min_width(200.0);
    // An action asked for while the rows on screen take none would not start.
    let enabled = mailbox.takes_actions();
    let mut item = |ui: &mut egui::Ui, icon, label, action| {
        if ui
            .add_enabled_ui(enabled, |ui| icon_item(ui, icon, label))
            .inner
            .clicked()
        {
            messages.push(Message::ApplyRowAction(row.id.clone(), action));
            ui.close();
        }
    };
    for MoveButton {
        icon,
        label,
        destination,
        ..
    } in moves(mailbox.location_of(&row.id))
    {
        item(ui, icon, label, MailAction::MoveTo(destination));
    }
    ui.separator();
    for FlagButton {
        icon,
        label,
        action,
        ..
    } in flags(row)
    {
        item(ui, icon, label, action);
    }
}

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
    // In a narrow reader the toolbar wraps between buttons, never inside one.
    let title = "Move to…";
    let button = egui::Button::new(title).wrap_mode(egui::TextWrapMode::Extend);
    wrap_for(ui, button_width(ui, title));
    ui.add_enabled_ui(enabled, |ui| {
        egui::containers::menu::MenuButton::from_button(button).ui(ui, |ui| {
            ui.set_min_width(MENU_MIN_WIDTH);
            egui::ScrollArea::vertical()
                .max_height(MENU_MAX_HEIGHT)
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
    icon_item(ui, folder_icon(folder), folder.name())
}

/// A menu entry led by its icon.
fn icon_item(ui: &mut egui::Ui, icon: theme::Icon, label: &str) -> egui::Response {
    let layout = egui::Button::new((theme::icon_atom(), label)).atom_ui(ui);
    let color = ui.style().interact(&layout.response).fg_stroke.color;
    theme::paint_atom_icon(ui, &layout, icon, color);
    layout.response
}

/// The width of a text button showing `title` on one line.
fn button_width(ui: &egui::Ui, title: &str) -> f32 {
    let galley = egui::WidgetText::from(title).into_galley(
        ui,
        Some(egui::TextWrapMode::Extend),
        f32::INFINITY,
        egui::TextStyle::Button,
    );
    galley.size().x + 2.0 * ui.spacing().button_padding.x
}

/// In a wrapping toolbar, starts a new row when a control `width` wide no
/// longer fits in this one. egui wraps widgets on its own, but not a child
/// `Ui` such as the one that disables a menu button, which would overflow.
fn wrap_for(ui: &mut egui::Ui, width: f32) {
    let row_started = ui.cursor().left() > ui.max_rect().left();
    if row_started && ui.available_size_before_wrap().x < width {
        ui.end_row();
    }
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
    // The tag, as the sidebar marks labels; filled, like the star, while the
    // conversation carries any. Its size never changes, so a count arriving
    // with the detail cannot move the card.
    let mut button = theme::IconButton::new(theme::Icon::Label, &title);
    if applied > 0 {
        button = button.selected(true);
    }

    ui.separator();
    theme::icon_menu(ui, enabled, button, |ui| {
        ui.set_min_width(MENU_MIN_WIDTH);
        egui::ScrollArea::vertical()
            .max_height(MENU_MAX_HEIGHT)
            .show(ui, |ui| {
                for label in labels {
                    let carried = label.custom_id().is_some_and(|id| carried_ids.contains(id));
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
                }
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
            let header = message_header(ui, message, preview, expanded);
            if header.clicked() {
                messages.push(Message::ToggleMessageExpanded(message.id.clone()));
            }
            header.context_menu(|ui| address_menu(ui, message, messages));
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

/// A header's right-click menu, which copies the addresses it names without
/// their names, as the composer's fields take them.
fn address_menu(ui: &mut egui::Ui, message: &MailMessage, messages: &mut Vec<Message>) {
    let sender = &message.sender.address;
    if !sender.is_empty() && ui.button("Copy sender's address").clicked() {
        messages.push(Message::CopyAddresses(vec![sender.clone()]));
        ui.close();
    }
    let recipients: Vec<String> = [&message.to, &message.cc, &message.bcc]
        .into_iter()
        .flatten()
        .map(|recipient| recipient.address.clone())
        .filter(|address| !address.is_empty())
        .collect();
    if !recipients.is_empty() && ui.button("Copy recipients' addresses").clicked() {
        messages.push(Message::CopyAddresses(recipients));
        ui.close();
    }
}

fn reply_toolbar(ui: &mut egui::Ui, message_id: Option<&str>, messages: &mut Vec<Message>) {
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), REPLY_TOOLBAR_HEIGHT),
        // A narrow reader moves a button to the next row rather than letting
        // the row widen the card past the pane, which would cut off the body.
        Layout::left_to_right(Align::Center)
            .with_main_align(Align::Max)
            .with_main_wrap(true),
        |ui| {
            // Core addresses replies from the message being answered.
            for (label, kind) in [
                ("Reply", AnswerKind::Reply),
                ("Reply all", AnswerKind::ReplyAll),
                ("Forward", AnswerKind::Forward),
            ] {
                if ui
                    .add_enabled(message_id.is_some(), theme::compact_button(label))
                    .clicked()
                    && let Some(message_id) = message_id
                {
                    messages.push(Message::Answer {
                        message_id: message_id.to_owned(),
                        kind,
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

/// How a header writes its sender.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SenderLine {
    /// `Name <address>` on one line: a collapsed card with a named sender.
    NameAndAddress,
    /// One text: the address, or a name when there is no address to show.
    Alone,
    /// The name, and under it the address beside the signature icon: an
    /// open card with a named sender.
    NameOverAddress,
}

impl SenderLine {
    fn of(message: &MailMessage, expanded: bool) -> Self {
        let named = message.sender.name.is_some() && !message.sender.address.is_empty();
        match (named, expanded) {
            (true, false) => Self::NameAndAddress,
            (true, true) => Self::NameOverAddress,
            (false, _) => Self::Alone,
        }
    }

    /// Whether the address has a line of its own, under the name.
    fn separate_address(self) -> bool {
        self == Self::NameOverAddress
    }
}

/// The whole header is one disclosure control. Copy selection is enabled only
/// below it, so clicking a sender or preview reliably expands the message. It
/// toggles the card as part of the reader, not as a link, so the pointer
/// stays an arrow, and only the chevron and the signature icon explain
/// themselves.
fn message_header(
    ui: &mut egui::Ui,
    message: &MailMessage,
    preview: &str,
    expanded: bool,
) -> egui::Response {
    let line = SenderLine::of(message, expanded);
    let recipients = if expanded {
        recipient_lines(message)
    } else {
        Vec::new()
    };
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(
            ui.available_width(),
            message_header_height(expanded, line, recipients.len()),
        ),
        Sense::click(),
    );
    paint_message_chevron(ui, rect, expanded, Some(&response));

    let painter = ui.painter_at(rect);
    let time = message
        .time
        .map(|time| format_message_time(time, &Local))
        .unwrap_or_default();
    // The date keeps the width it needs, up to half the header, so in a
    // narrow reader it is shortened instead of drawn over the sender.
    let date = theme::truncated_galley(
        &painter,
        &time,
        FontId::proportional(theme::FOOTNOTE_SIZE),
        theme::colors(ui).muted,
        (rect.width() - HEADER_TEXT_INSET) * 0.5,
    );
    let copy_rect = header_copy_rect(rect, date.size().x);
    let (icon_rect, status) = paint_sender(ui, &painter, message, line, copy_rect);
    paint_header_details(ui, &painter, &recipients, preview, line, copy_rect);
    painter.galley(
        egui::pos2(rect.right() - date.size().x, rect.top() + 2.0),
        date,
        theme::colors(ui).muted,
    );

    let identity = sender_identity(message, line);
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::CollapsingHeader,
            true,
            format!("{identity}, {status}"),
        )
    });
    header_hover(ui, response, icon_rect, expanded, status)
}

/// Paints the signature icon and the sender as `line` lays it out, and
/// returns where the icon is and the status it stands for.
fn paint_sender(
    ui: &egui::Ui,
    painter: &egui::Painter,
    message: &MailMessage,
    line: SenderLine,
    copy_rect: egui::Rect,
) -> (egui::Rect, &'static str) {
    let sender = message.sender.display_name().unwrap_or("(Unknown sender)");
    let identity_top =
        copy_rect.left_top() + egui::vec2(0.0, if line.separate_address() { 20.0 } else { 0.0 });
    let icon_rect = egui::Rect::from_min_size(identity_top, egui::Vec2::splat(SIGNATURE_ICON_SIZE));
    let (icon, status, color) = message_status(ui, message);
    egui::Image::new(icon.source())
        .tint(color)
        .paint_at(ui, icon_rect);
    let sender_inset = if line.separate_address() {
        0.0
    } else {
        SIGNATURE_ICON_INSET
    };
    let sender_origin = copy_rect.left_top() + egui::vec2(sender_inset, 0.0);
    let sender_width = (copy_rect.width() - sender_inset).max(0.0);
    match line {
        // The lock vouches for the address, never for the name the sender
        // chose, so a collapsed card keeps the address beside that name.
        SenderLine::NameAndAddress => paint_name_and_address(
            ui,
            painter,
            sender_origin,
            sender,
            &message.sender.address,
            sender_width,
        ),
        SenderLine::Alone | SenderLine::NameOverAddress => theme::paint_truncated_text(
            painter,
            sender_origin,
            sender,
            FontId::proportional(theme::ROW_TEXT_SIZE),
            ui.visuals().strong_text_color(),
            sender_width,
        ),
    }
    if line.separate_address() {
        header_detail(
            painter,
            identity_top + egui::vec2(SIGNATURE_ICON_INSET, 0.0),
            &message.sender.address,
            theme::colors(ui).muted,
            (copy_rect.width() - SIGNATURE_ICON_INSET).max(0.0),
        );
    }
    (icon_rect, status)
}

/// Paints what follows the sender: an open card's recipient lines, or, with
/// none, a collapsed card's preview.
fn paint_header_details(
    ui: &egui::Ui,
    painter: &egui::Painter,
    recipients: &[String],
    preview: &str,
    line: SenderLine,
    copy_rect: egui::Rect,
) {
    if !recipients.is_empty() {
        let top = recipients_top(line);
        for (index, recipient_line) in recipients.iter().enumerate() {
            header_detail(
                painter,
                copy_rect.left_top() + egui::vec2(0.0, top + index as f32 * RECIPIENT_LINE_PITCH),
                recipient_line,
                theme::colors(ui).muted,
                copy_rect.width(),
            );
        }
    } else {
        theme::paint_truncated_text(
            painter,
            copy_rect.left_top() + egui::vec2(0.0, 22.0),
            preview,
            FontId::proportional(theme::ROW_TEXT_SIZE),
            theme::colors(ui).muted,
            copy_rect.width(),
        );
    }
}

/// The sender as screen readers hear it: a name always comes with its
/// address, as on screen.
fn sender_identity(message: &MailMessage, line: SenderLine) -> String {
    let sender = message.sender.display_name().unwrap_or("(Unknown sender)");
    match line {
        SenderLine::Alone => sender.to_owned(),
        SenderLine::NameAndAddress | SenderLine::NameOverAddress => {
            format!("{sender} <{}>", message.sender.address)
        }
    }
}

/// The header's tooltip: the signature icon names its status, and the
/// chevron the toggle. Elsewhere the header says nothing.
fn header_hover(
    ui: &egui::Ui,
    response: egui::Response,
    icon_rect: egui::Rect,
    expanded: bool,
    status: &str,
) -> egui::Response {
    if ui.rect_contains_pointer(icon_rect) {
        response.on_hover_text(status)
    } else if ui.rect_contains_pointer(chevron_area(response.rect)) {
        response.on_hover_text(if expanded { "Collapse" } else { "Expand" })
    } else {
        response
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
        FontId::proportional(ADDRESS_SIZE),
        muted,
        width * 2.0 / 3.0,
    );
    let name = theme::truncated_galley(
        painter,
        name,
        FontId::proportional(theme::ROW_TEXT_SIZE),
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

/// A header's height: an open card lists its recipients under the sender,
/// and under the sender's address when `line` gives that a line of its own,
/// in `recipient_lines` lines.
fn message_header_height(expanded: bool, line: SenderLine, recipient_lines: usize) -> f32 {
    if expanded {
        let more = recipient_lines.saturating_sub(1) as f32;
        recipients_top(line) + 27.0 + more * RECIPIENT_LINE_PITCH
    } else {
        44.0
    }
}

/// From one recipient line of an open header to the next: the same step as
/// from the sender's address down to "To:".
const RECIPIENT_LINE_PITCH: f32 = 19.0;

/// An open header's recipient lines: To always, Cc and Bcc when the message
/// has them, so a copy never reads as one of the people it was sent to.
fn recipient_lines(message: &MailMessage) -> Vec<String> {
    let mut lines = vec![format!("To: {}", recipients_label(&message.to))];
    for (field, people) in [("Cc", &message.cc), ("Bcc", &message.bcc)] {
        if !people.is_empty() {
            lines.push(format!("{field}: {}", recipients_label(people)));
        }
    }
    lines
}

/// How far below an open header's top its "To:" line starts.
fn recipients_top(line: SenderLine) -> f32 {
    if line.separate_address() { 39.0 } else { 20.0 }
}

/// Paints a header's chevron, brighter while the pointer is over the header
/// whose `response` it is. A loading card has none.
fn paint_message_chevron(
    ui: &egui::Ui,
    rect: egui::Rect,
    expanded: bool,
    response: Option<&egui::Response>,
) {
    let icon_center = egui::pos2(rect.left() + 8.0, rect.top() + 11.0);
    let points = chevron_points(icon_center, expanded);
    let color = if response.is_some_and(egui::Response::hovered) {
        ui.visuals().strong_text_color()
    } else {
        theme::colors(ui).muted
    };
    ui.painter()
        .line_segment([points[0], points[1]], Stroke::new(1.5, color));
    ui.painter()
        .line_segment([points[1], points[2]], Stroke::new(1.5, color));
}

/// The column left of the header's text, where the chevron sits.
fn chevron_area(rect: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_max(
        rect.left_top(),
        egui::pos2(rect.left() + HEADER_TEXT_INSET, rect.top() + 22.0),
    )
}

/// Where a header's text goes: right of the chevron, short of the date.
fn header_copy_rect(rect: egui::Rect, date_width: f32) -> egui::Rect {
    egui::Rect::from_min_max(
        egui::pos2(rect.left() + HEADER_TEXT_INSET, rect.top()),
        egui::pos2(rect.right() - date_width - HEADER_DATE_GAP, rect.bottom()),
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
        FontId::proportional(theme::FOOTNOTE_SIZE),
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
                egui::Label::new(egui::RichText::new(content).size(theme::BODY_SIZE))
                    .selectable(true)
                    .wrap(),
            );
        }
        MessageBody::Rich(body) => rich_body(ui, body, &message.id, reader, reading, messages),
        MessageBody::Unreadable => {
            ui.label(
                egui::RichText::new(UNREADABLE_NOTICE)
                    .size(theme::BODY_SIZE)
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
                            egui::Label::new(egui::RichText::new(marker).size(theme::BODY_SIZE))
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
                        egui::Label::new(
                            egui::RichText::new(content)
                                .monospace()
                                .size(theme::BODY_SIZE),
                        )
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
                                .size(theme::BODY_SIZE - 2.0)
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
            let response = ui.add(egui::Link::new(job));
            if response.clicked() {
                messages.push(Message::LinkClicked(link.to_string()));
            }
            response.context_menu(|ui| link_menu(ui, link, messages));
        } else {
            ui.add(egui::Label::new(job).selectable(true).wrap());
        }
    }
}

/// A link's right-click menu. Opening it still asks first, when Settings say
/// so; copying its address takes the reader nowhere.
fn link_menu(ui: &mut egui::Ui, link: &str, messages: &mut Vec<Message>) {
    if ui.button("Open link…").clicked() {
        messages.push(Message::LinkClicked(link.to_owned()));
        ui.close();
    }
    if ui.button("Copy link").clicked() {
        messages.push(Message::CopyLinkAddress(link.to_owned()));
        ui.close();
    }
}

/// Builds span styling; `TextFormat` allows padded code backgrounds.
fn span_layout(ui: &egui::Ui, span: &RichSpan, heading: Option<u8>) -> LayoutJob {
    let size = heading.map(heading_size).unwrap_or(theme::BODY_SIZE);
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
mod tests;
