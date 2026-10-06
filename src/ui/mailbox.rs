use chrono::{DateTime, Datelike, Local, TimeZone};
use eframe::egui::AtomExt as _;
use eframe::egui::{self, Align, Align2, Color32, CornerRadius, FontId, Layout, Sense, Stroke};

use super::{UiState, UndoNotice, compose, reader, settings, theme};
use crate::app::{App, ListStatus, Mailbox, Message, UndoMove, panel_ratios, panel_widths};
use crate::mail::{ConversationSummary, CustomKind, Folder, MailFolder};
use crate::settings::{ComposePlacement, Settings};

const SIDEBAR_PANEL_ID: &str = "mailbox-sidebar";
const CONVERSATION_PANEL_ID: &str = "conversation-list";

pub(super) fn reset_panel_sizes(context: &egui::Context) {
    // egui's remembered widths take precedence over each panel's default_size.
    context.data_mut(|data| {
        for id in [SIDEBAR_PANEL_ID, CONVERSATION_PANEL_ID] {
            data.remove::<egui::containers::panel::PanelState>(egui::Id::new(id));
        }
    });
}

pub(super) fn show(
    root: &mut egui::Ui,
    app: &App,
    email: Option<&str>,
    signing_out: bool,
    settings_draft: Option<&mut Settings>,
    state: &mut UiState,
    messages: &mut Vec<Message>,
) {
    let mailbox = app.mailbox().expect("authenticated mailbox");
    let context = root.ctx().clone();
    let window_width = root.available_width();
    let widths = panel_widths(app.panels(), window_width);
    let inset = theme::titlebar_inset(&context);

    let sidebar = egui::Panel::left(SIDEBAR_PANEL_ID)
        .default_size(widths.sidebar)
        .size_range(200.0..=(window_width - 400.0).max(200.0))
        .resizable(true)
        .frame(theme::top_panel_frame(theme::colors(root).sidebar, inset))
        .show(root, |ui| {
            sidebar(ui, app, mailbox, email, signing_out, messages)
        });

    if let Some(draft) = settings_draft {
        egui::CentralPanel::default()
            .frame(theme::top_panel_frame(theme::colors(root).panel, inset))
            .show(root, |ui| {
                settings::show(ui, app.settings(), draft, messages)
            });

        let mut actual = app.panels();
        actual.sidebar = sidebar.response.rect.width() / window_width;
        if (actual.sidebar - app.panels().sidebar).abs() > 0.002 {
            messages.push(Message::PanelsResized(actual));
        }
        return;
    }

    let remaining = (window_width - sidebar.response.rect.width()).max(400.0);
    let conversations = egui::Panel::left(CONVERSATION_PANEL_ID)
        .default_size(widths.conversations)
        .size_range(200.0..=(remaining - 200.0).max(200.0))
        .resizable(true)
        .frame(theme::top_panel_frame(theme::colors(root).panel, inset))
        .show(root, |ui| conversation_pane(ui, mailbox, state, messages));

    egui::CentralPanel::default()
        .frame(theme::top_panel_frame(theme::colors(root).panel, inset))
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
        if settings_button(ui, app.showing_settings() && !signing_out).clicked() {
            messages.push(Message::ShowSettings(true));
        }
        if let Some(error) = app.error_message() {
            ui.label(
                egui::RichText::new(error)
                    .small()
                    .color(theme::colors(ui).danger),
            );
        }
        let account_label = if app.is_demo() {
            "demo@example.com"
        } else {
            email.unwrap_or("Proton Mail account")
        };
        ui.label(
            egui::RichText::new(account_label)
                .size(12.0)
                .color(theme::colors(ui).muted),
        );
    });
}

fn settings_button(ui: &mut egui::Ui, selected: bool) -> egui::Response {
    let width = ui.available_width();
    let hover = theme::colors(ui).sidebar_button_hover;
    ui.scope(|ui| {
        let widgets = &mut ui.visuals_mut().widgets;
        let outline = Stroke::new(1.0, theme::ACCENT);
        widgets.inactive.weak_bg_fill = if selected {
            theme::ACCENT
        } else {
            Color32::TRANSPARENT
        };
        widgets.hovered.weak_bg_fill = if selected { theme::ACCENT_HOVER } else { hover };
        widgets.active.weak_bg_fill = theme::ACCENT_PRESSED;
        if selected {
            widgets.inactive.fg_stroke.color = Color32::WHITE;
            widgets.hovered.fg_stroke.color = Color32::WHITE;
        }
        widgets.active.fg_stroke.color = Color32::WHITE;
        widgets.inactive.bg_stroke = outline;
        widgets.hovered.bg_stroke = outline;
        widgets.active.bg_stroke = outline;

        ui.add(egui::Button::new("Settings").min_size(egui::vec2(width, 32.0)))
    })
    .inner
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
        egui::RichText::new(folder.name()).color(theme::colors(ui).selected_folder_text)
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
        let text = egui::RichText::new(count.to_string());
        button = button.right_text(if selected {
            text.color(theme::colors(ui).selected_folder_text)
        } else {
            text
        });
    }
    if selected {
        button = button
            .fill(theme::colors(ui).accent_soft)
            .stroke(Stroke::NONE);
    }
    let layout = button.atom_ui(ui);
    let icon_color = if icon == theme::Icon::Label {
        theme::ACCENT
    } else if selected {
        theme::colors(ui).selected_folder_text
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
            theme::colors(ui).selected_folder_text,
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

    let empty = mailbox.is_empty_visible();
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
            theme::card(ui)
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

struct RowAnchor {
    id: String,
    index: usize,
    conversation: Option<String>,
}

/// Two visible rows are enough to retain a nearby survivor when inspection
/// replaces the first one. No snapshot of the complete listing is retained.
pub(super) struct ConversationScroll {
    scroll_id: egui::Id,
    folder: Folder,
    query: String,
    search: Option<String>,
    offset: f32,
    anchors: [Option<RowAnchor>; 2],
}

impl ConversationScroll {
    fn capture(
        mailbox: &Mailbox,
        scroll_id: egui::Id,
        offset: f32,
        previous: Option<&Self>,
    ) -> Self {
        let previous = previous.filter(|saved| saved.same_view(mailbox, scroll_id));
        let first = (offset / (ROW_HEIGHT + ROW_GAP)).floor() as usize;
        let first = first.min(mailbox.visible_count().saturating_sub(1));
        Self {
            scroll_id,
            folder: mailbox.folder().clone(),
            query: mailbox.search_query().to_owned(),
            search: mailbox.search_results().map(str::to_owned),
            offset,
            anchors: std::array::from_fn(|extra| {
                let index = first + extra;
                let row = mailbox.visible_row(index)?;
                // Keep a message's identity while its grouped parent is
                // temporarily displayed, so later inspection can restore it.
                let retained = previous.and_then(|saved| {
                    saved.anchors.iter().flatten().find(|anchor| {
                        anchor.id != row.id
                            && anchor.conversation.as_deref() == Some(row.id.as_str())
                            && !mailbox.has_row(&anchor.id)
                    })
                });
                Some(RowAnchor {
                    id: retained.map_or_else(|| row.id.clone(), |anchor| anchor.id.clone()),
                    index,
                    conversation: mailbox.row_conversation(row).map(str::to_owned),
                })
            }),
        }
    }

    fn same_view(&self, mailbox: &Mailbox, scroll_id: egui::Id) -> bool {
        self.scroll_id == scroll_id
            && &self.folder == mailbox.folder()
            && self.query == mailbox.search_query()
            && self.search.as_deref() == mailbox.search_results()
    }

    fn offset_after_update(&self, mailbox: &Mailbox, scroll_id: egui::Id) -> Option<f32> {
        // An untouched list starts at the top and remains there as inspection
        // turns its first conversation into individual messages.
        if !self.same_view(mailbox, scroll_id) || self.offset == 0.0 {
            return None;
        }
        let first = self.anchors[0].as_ref()?;
        // Ordinary appends and idle frames need only one indexed lookup.
        if mailbox
            .visible_row(first.index)
            .is_some_and(|row| row.id == first.id)
        {
            return None;
        }
        self.anchors.iter().flatten().find_map(|anchor| {
            let index = mailbox.visible_anchor_index(&anchor.id, anchor.conversation.as_deref())?;
            let delta = (index as f32 - anchor.index as f32) * (ROW_HEIGHT + ROW_GAP);
            Some((self.offset + delta).max(0.0))
        })
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
    let visible_count = mailbox.visible_count();
    let total = visible_count + usize::from(ends_with_load_more(mailbox));

    ui.scope(|ui| {
        // Read by `show_rows` to work out where each row goes, so it is set
        // on this `Ui` and not inside the closure.
        ui.spacing_mut().item_spacing.y = ROW_GAP;
        ui.spacing_mut().scroll = theme::panel_scroll_style();
        let scroll_id = ui.make_persistent_id(egui::IdSalt::new("conversation-scroll"));
        if state.conversation_scroll.is_none() {
            // A new mailbox must not inherit native scroll or animation state
            // left in this window by a previous signed-in session.
            egui::scroll_area::State::default().store(ui.ctx(), scroll_id);
        }
        let mut scroll = egui::ScrollArea::vertical()
            .id_salt("conversation-scroll")
            .auto_shrink([false, false]);

        // Synthesized rectangles can reveal rows outside the rendered range.
        let reveal = state.reveal_conversation.as_deref().and_then(|wanted| {
            mailbox
                .visible_conversations()
                .position(|row| row.id == wanted)
        });

        // Explicit keyboard navigation takes precedence over restoring the
        // viewport after a background listing update.
        if reveal.is_none()
            && let Some(offset) = state
                .conversation_scroll
                .as_ref()
                .and_then(|saved| saved.offset_after_update(mailbox, scroll_id))
        {
            scroll = scroll.vertical_scroll_offset(offset);
        }

        let output = scroll.show_rows(ui, ROW_HEIGHT, total, |ui, range| {
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
                let Some(conversation) = mailbox.visible_row(index) else {
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
        state.conversation_scroll = Some(ConversationScroll::capture(
            mailbox,
            output.id,
            output.state.offset.y,
            state.conversation_scroll.as_ref(),
        ));
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
        theme::colors(ui).accent_soft
    } else {
        Color32::TRANSPARENT
    };
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, ROW_HEIGHT), Sense::click());
    let hovered_fill = if response.hovered() && !selected {
        theme::colors(ui).row_hover
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
        theme::colors(ui).muted,
        text_width,
    );

    let meta_font = FontId::proportional(11.0);
    painter.text(
        egui::pos2(rect.right() - ROW_HORIZONTAL_PADDING, sender_center_y),
        Align2::RIGHT_CENTER,
        time,
        meta_font.clone(),
        theme::colors(ui).muted,
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
            theme::colors(ui).muted,
        )
        .left();
    if conversation.has_attachments {
        theme::paint_clip(
            &painter,
            egui::pos2(facts_left - 9.0, subject_center_y),
            theme::colors(ui).muted,
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
        ui.label(egui::RichText::new(text).color(theme::colors(ui).muted));
    });
}

pub(super) fn detail(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .small()
            .color(theme::colors(ui).muted),
    );
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
    use crate::mail::{ConversationDetail, ConversationPage, MailboxError, SummaryKind};

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
        context
            .all_styles_mut(|style| style.scroll_animation = egui::style::ScrollAnimation::none());
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
                    let waiting =
                        pagination_frame(&context, &mailbox, &mut state, Default::default());
                    assert_eq!(waiting.offset, before.offset);
                    assert_eq!(mailbox.reader_state(), &reader);
                    mailbox.finish_search(&next, result);
                } else {
                    let next = mailbox.load_more(10).unwrap();
                    let waiting =
                        pagination_frame(&context, &mailbox, &mut state, Default::default());
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
            assert_eq!(after.offset, before.offset + ROW_HEIGHT + ROW_GAP);
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
            set_list_offset(&context, &state, parent as f32 * (ROW_HEIGHT + ROW_GAP));
            let before = pagination_frame(&context, &mailbox, &mut state, Default::default());
            for request in 20..24 {
                let refresh = mailbox.refresh(request).unwrap();
                let waiting = pagination_frame(&context, &mailbox, &mut state, Default::default());
                assert_eq!(waiting.offset, before.offset);
                let mut page = pagination_page(0, 50, 120);
                page.conversations[parent as usize].unread = request % 2 == 0;
                page.conversations[parent as usize].starred = request % 2 != 0;
                mailbox.finish_page(refresh.id, Ok(page));
                let refreshed =
                    pagination_frame(&context, &mailbox, &mut state, Default::default());
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
        set_list_offset(&context, &state, 21.0 * (ROW_HEIGHT + ROW_GAP));
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
    fn settings_button_responds_to_hover_press_and_selection_in_both_themes() {
        fn fills(shape: &egui::epaint::Shape, found: &mut Vec<Color32>) {
            match shape {
                egui::epaint::Shape::Rect(rect) => found.push(rect.fill),
                egui::epaint::Shape::Vec(shapes) => {
                    for shape in shapes {
                        fills(shape, found);
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
                    let mut hover_fill = Color32::TRANSPARENT;
                    let output = context.run_ui(input, |ui| {
                        ui.set_width(240.0);
                        hover_fill = theme::colors(ui).sidebar_button_hover;
                        button = settings_button(ui, selected).rect;
                    });
                    let mut colors = Vec::new();
                    for clipped in &output.shapes {
                        fills(&clipped.shape, &mut colors);
                    }
                    output.drop_without_applying_deltas();
                    (button, colors, hover_fill)
                };

                let (button, idle, hover_fill) = render(egui::RawInput::default());
                let idle_fill = if selected {
                    theme::ACCENT
                } else {
                    Color32::TRANSPARENT
                };
                assert!(idle.contains(&idle_fill), "{appearance:?}, {selected}");
                let position = button.center();
                let hover = || egui::RawInput {
                    events: vec![egui::Event::PointerMoved(position)],
                    ..Default::default()
                };
                render(hover());
                let (_, hovered, _) = render(hover());
                let expected_hover = if selected {
                    theme::ACCENT_HOVER
                } else {
                    hover_fill
                };
                assert!(
                    hovered.contains(&expected_hover),
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
                    pressed.contains(&theme::ACCENT_PRESSED),
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
                show(
                    root,
                    &app,
                    None,
                    false,
                    Some(&mut draft),
                    &mut state,
                    &mut messages,
                );
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
