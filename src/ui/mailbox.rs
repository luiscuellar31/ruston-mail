use chrono::{DateTime, Datelike, Local, TimeZone};
use eframe::egui::AtomExt as _;
use eframe::egui::{self, Align, Align2, Color32, CornerRadius, FontId, Layout, Sense, Stroke};

use super::{UiState, UndoNotice, compose, reader, settings, theme};
use crate::app::{
    App, AuthState, ListAttempt, ListStatus, MIN_PANEL_WIDTH, Mailbox, Message, UndoMove,
    compact_layout, panel_ratios, panel_widths,
};
use crate::mail::{ConversationSummary, CustomKind, Folder, MailFolder, MailboxError};
use crate::settings::{ComposePlacement, Panels, Settings};

const SIDEBAR_PANEL_ID: &str = "mailbox-sidebar";
const CONVERSATION_PANEL_ID: &str = "conversation-list";
/// Drawing the saved panel ratios measures back ratios a little off them, so
/// a smaller difference is not a resize: saving it would rewrite the settings
/// on frames where nothing was dragged.
const PANEL_RATIO_TOLERANCE: f32 = 0.002;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) enum CompactView {
    Folders,
    #[default]
    Mail,
    Reader,
}

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
    settings_draft: Option<&mut Settings>,
    state: &mut UiState,
    messages: &mut Vec<Message>,
) {
    let mailbox = app.mailbox().expect("authenticated mailbox");
    let context = root.ctx().clone();
    let window_width = root.available_width();
    if compact_layout(window_width) {
        show_compact(root, app, settings_draft, state, messages);
        undo_notice(&context, mailbox, state, None, messages);
        return;
    }
    let widths = panel_widths(app.panels(), window_width);
    let inset = theme::titlebar_inset(&context);

    let sidebar = egui::Panel::left(SIDEBAR_PANEL_ID)
        .default_size(widths.sidebar)
        .size_range(MIN_PANEL_WIDTH..=(window_width - 2.0 * MIN_PANEL_WIDTH).max(MIN_PANEL_WIDTH))
        .resizable(true)
        .frame(theme::top_panel_frame(theme::colors(root).sidebar, inset))
        .show(root, |ui| {
            sidebar(ui, app, mailbox, messages);
        });

    let conversations = match settings_draft {
        Some(draft) => {
            egui::CentralPanel::default()
                .frame(theme::top_panel_frame(theme::colors(root).panel, inset))
                .show(root, |ui| {
                    settings::show(
                        ui,
                        app.settings(),
                        draft,
                        messages,
                        state.notification_status,
                    )
                });
            None
        }
        None => {
            let remaining =
                (window_width - sidebar.response.rect.width()).max(2.0 * MIN_PANEL_WIDTH);
            let conversations = egui::Panel::left(CONVERSATION_PANEL_ID)
                .default_size(widths.conversations)
                .size_range(MIN_PANEL_WIDTH..=(remaining - MIN_PANEL_WIDTH).max(MIN_PANEL_WIDTH))
                .resizable(true)
                .frame(theme::top_panel_frame(theme::colors(root).panel, inset))
                .show(root, |ui| {
                    conversation_pane(ui, app, mailbox, state, messages)
                });

            egui::CentralPanel::default()
                .frame(theme::top_panel_frame(theme::colors(root).panel, inset))
                .show(root, |ui| reading_pane(ui, app, state, messages));
            Some(conversations.response.rect)
        }
    };

    undo_notice(&context, mailbox, state, conversations, messages);
    let conversations = conversations.map(|list| list.width());

    let stored = app.panels();
    let sidebar = sidebar.response.rect.width();
    let actual = match conversations {
        Some(conversations) => panel_ratios(sidebar, conversations, window_width),
        // Settings covers the conversation list, so only the sidebar can
        // have been dragged.
        None => Panels {
            sidebar: panel_ratios(sidebar, widths.conversations, window_width).sidebar,
            ..stored
        },
    };
    if (actual.sidebar - stored.sidebar).abs() > PANEL_RATIO_TOLERANCE
        || (actual.conversations - stored.conversations).abs() > PANEL_RATIO_TOLERANCE
    {
        messages.push(Message::PanelsResized(actual));
    }
}

fn show_compact(
    root: &mut egui::Ui,
    app: &App,
    settings_draft: Option<&mut Settings>,
    state: &mut UiState,
    messages: &mut Vec<Message>,
) {
    let mailbox = app.mailbox().expect("authenticated mailbox");
    let inset = theme::titlebar_inset(root.ctx());
    let writing = app.settings().compose_placement == ComposePlacement::ReadingPane
        && app.compose().is_some();
    let reader_available = writing || mailbox.selected_conversation().is_some();
    if state.compact_view == CompactView::Reader && !reader_available {
        state.compact_view = CompactView::Mail;
    }
    egui::Panel::top("compact-navigation")
        .frame(theme::top_panel_frame(theme::colors(root).sidebar, inset))
        .show(root, |ui| {
            ui.horizontal_wrapped(|ui| {
                for (view, label) in [
                    (CompactView::Folders, "Folders"),
                    (CompactView::Mail, "Mail"),
                    (
                        CompactView::Reader,
                        if writing { "Message" } else { "Reading" },
                    ),
                ] {
                    let enabled = view != CompactView::Reader || reader_available;
                    if ui
                        .add_enabled(
                            enabled,
                            egui::Button::new(label)
                                .selected(settings_draft.is_none() && state.compact_view == view),
                        )
                        .clicked()
                    {
                        state.compact_view = view;
                        if settings_draft.is_some() {
                            // Leaving Settings still goes through the existing unsaved-change guard.
                            messages.push(Message::ShowSettings(false));
                        }
                    }
                }
            });
        });
    let fill = if settings_draft.is_none() && state.compact_view == CompactView::Folders {
        theme::colors(root).sidebar
    } else {
        theme::colors(root).panel
    };
    egui::CentralPanel::default()
        .frame(theme::panel_frame(fill))
        .show(root, |ui| {
            if let Some(draft) = settings_draft {
                settings::show(
                    ui,
                    app.settings(),
                    draft,
                    messages,
                    state.notification_status,
                );
            } else {
                match state.compact_view {
                    CompactView::Folders => {
                        sidebar(ui, app, mailbox, messages);
                    }
                    CompactView::Mail => conversation_pane(ui, app, mailbox, state, messages),
                    CompactView::Reader => reading_pane(ui, app, state, messages),
                }
            }
        });
}

fn reading_pane(ui: &mut egui::Ui, app: &App, state: &mut UiState, messages: &mut Vec<Message>) {
    if app.settings().compose_placement == ComposePlacement::ReadingPane
        && let Some(writing) = app.compose()
    {
        compose::show_in_pane(ui, writing, messages);
    } else {
        let view = reader::ReaderView {
            mailbox: app.mailbox().expect("authenticated mailbox"),
            places: app.folders(),
            actions_available: app.mailbox_actions_available(),
            saving_attachment: app.saving_attachment(),
            saved_attachment: app.saved_attachment(),
            reading: app.settings().reading(),
        };
        reader::show(ui, &view, state, messages);
    }
}

fn sidebar(
    ui: &mut egui::Ui,
    app: &App,
    mailbox: &Mailbox,
    messages: &mut Vec<Message>,
) -> egui::scroll_area::ScrollAreaOutput<()> {
    ui.scope_builder(
        egui::UiBuilder::new().id(egui::Id::new("mailbox-folder-content")),
        |ui| {
            let available = ui.available_rect_before_wrap();
            let account = egui::Panel::bottom("sidebar-account-actions")
                .frame(egui::Frame::NONE.inner_margin(egui::Margin {
                    top: theme::PANEL_PADDING,
                    ..Default::default()
                }))
                .show(ui, |ui| sidebar_account(ui, app, messages));
            if account.response.rect.bottom() > available.bottom() + 1.0 {
                // A bottom panel needs one measured frame before its height is
                // known. Schedule that frame even if the pointer is stationary.
                ui.ctx().request_repaint();
            }
            ui.spacing_mut().scroll = theme::panel_scroll_style();
            egui::ScrollArea::vertical()
                .id_salt("sidebar-scroll")
                .min_scrolled_height(0.0)
                .auto_shrink([false, false])
                .show(ui, |ui| sidebar_content(ui, app, mailbox, messages))
        },
    )
    .inner
}

fn sidebar_content(ui: &mut egui::Ui, app: &App, mailbox: &Mailbox, messages: &mut Vec<Message>) {
    ui.heading(
        egui::RichText::new("Ruston Mail")
            .size(theme::APP_NAME_SIZE)
            .strong(),
    );
    ui.add_space(12.0);

    if new_message_button(ui).clicked() {
        messages.push(Message::OpenCompose);
    }
    ui.add_space(12.0);

    // Settings covers the mail panes, so it alone shows as chosen.
    let current = (!app.showing_settings()).then(|| mailbox.folder());
    for folder in MailFolder::ALL.into_iter().map(Folder::System).chain(
        app.folders()
            .iter()
            .filter(|folder| folder.kind() == Some(CustomKind::Folder))
            .cloned(),
    ) {
        folder_button(ui, mailbox, current, folder, messages);
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
            folder_button(ui, mailbox, current, label, messages);
        }
    }
}

fn sidebar_account(ui: &mut egui::Ui, app: &App, messages: &mut Vec<Message>) {
    // The mailbox is shown only while signed in, or while signing out.
    let (email, signing_out) = match app.auth_state() {
        AuthState::Authenticated { email } => (email.as_deref(), false),
        AuthState::SigningOut => (None, true),
        _ => (None, false),
    };
    ui.vertical(|ui| {
        let account_label = if app.is_demo() {
            "demo@example.com"
        } else {
            email.unwrap_or("Proton Mail account")
        };
        ui.add(
            egui::Label::new(
                egui::RichText::new(account_label)
                    .size(12.0)
                    .color(theme::colors(ui).muted),
            )
            .truncate(),
        )
        .on_hover_text(account_label);
        if let Some(error) = app.error_message() {
            ui.label(egui::RichText::new(error).color(theme::colors(ui).danger));
        }
        if settings_button(ui, app.showing_settings() && !signing_out).clicked() {
            messages.push(Message::ShowSettings(true));
        }
        let logout_label = if signing_out {
            "Signing out…"
        } else if app.is_demo() {
            "Exit demo"
        } else {
            "Sign out"
        };
        // Signing out would hide whether a message being sent has left.
        let sending = app.compose().is_some_and(crate::app::Compose::in_flight);
        let mut logout = ui.add_enabled(
            !signing_out && !sending,
            egui::Button::new(logout_label).min_size(egui::vec2(ui.available_width(), 32.0)),
        );
        if sending {
            logout = logout.on_disabled_hover_text("Available once the message is sent");
        }
        if logout.clicked() {
            messages.push(Message::Logout);
        }
    });
}

/// A neutral button like the one under it; while Settings is open, the soft
/// violet of the open folder.
fn settings_button(ui: &mut egui::Ui, selected: bool) -> egui::Response {
    let width = ui.available_width();
    let mut label = egui::RichText::new("Settings");
    if selected {
        label = label.color(theme::colors(ui).selected_folder_text);
    }
    let mut button = egui::Button::new(label).min_size(egui::vec2(width, 32.0));
    if selected {
        button = button
            .fill(theme::colors(ui).accent_soft)
            .stroke(Stroke::NONE);
    }
    ui.add(button)
}

const NEW_MESSAGE_LABEL_ID: &str = "new-message-label";
const FOLDER_NAME_ATOM_ID: &str = "folder-name";

fn new_message_button(ui: &mut egui::Ui) -> egui::AtomLayoutResponse {
    let width = ui.available_width();
    ui.scope(|ui| {
        theme::primary_visuals(ui);
        egui::Button::new((
            egui::Atom::grow(),
            egui::RichText::new("New message")
                .size(15.0)
                .color(theme::ON_ACCENT)
                .atom_id(egui::Id::new(NEW_MESSAGE_LABEL_ID)),
            egui::Atom::grow(),
        ))
        .min_size(egui::vec2(width, 34.0))
        .stroke(Stroke::NONE)
        .atom_ui(ui)
    })
    .inner
}

/// One place in the sidebar. `current` is the place to mark as chosen, if
/// any.
fn folder_button(
    ui: &mut egui::Ui,
    mailbox: &Mailbox,
    current: Option<&Folder>,
    folder: Folder,
    messages: &mut Vec<Message>,
) {
    let selected = current == Some(&folder);
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
    let layout = ui
        .scope(|ui| {
            // egui's grey hover is brighter than the soft violet of the
            // selection; a place under the pointer gets a quieter tint of it.
            let colors = theme::colors(ui);
            let widgets = &mut ui.visuals_mut().widgets;
            widgets.hovered.weak_bg_fill = colors.sidebar_hover;
            widgets.hovered.bg_stroke = Stroke::NONE;
            widgets.active.weak_bg_fill = colors.accent_soft;
            widgets.active.bg_stroke = Stroke::NONE;
            button.atom_ui(ui)
        })
        .inner;
    let icon_color = if icon == theme::Icon::Label {
        theme::ACCENT
    } else if selected {
        theme::colors(ui).selected_folder_text
    } else {
        ui.style().interact(&layout.response).fg_stroke.color
    };
    theme::paint_atom_icon(ui, &layout, icon, icon_color);
    if selected && let Some(rect) = layout.rect(egui::Id::new(FOLDER_NAME_ATOM_ID)) {
        // The bundled font has one weight. A second pass one physical pixel
        // to the right reads as bold at every display density.
        let offset = 1.0 / ui.ctx().pixels_per_point();
        let painter = ui.painter().with_clip_rect(rect);
        theme::paint_truncated_text_aligned(
            &painter,
            rect.left_center() + egui::vec2(offset, 0.0),
            Align2::LEFT_CENTER,
            folder.name(),
            egui::TextStyle::Button.resolve(ui.style()),
            theme::colors(ui).selected_folder_text,
            rect.width() - offset,
        );
    }
    let response = layout.response;
    let spoken_name = folder.name().to_owned();
    response.widget_info(|| {
        let label = unread.map_or_else(
            || spoken_name.clone(),
            |count| format!("{spoken_name}, {count} unread"),
        );
        egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, label)
    });
    if response.clicked() {
        messages.push(Message::SelectFolder(folder));
    }
}

pub(super) fn folder_icon(folder: &Folder) -> theme::Icon {
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
    app: &App,
    mailbox: &Mailbox,
    state: &mut UiState,
    messages: &mut Vec<Message>,
) {
    ui.horizontal(|ui| {
        // Refresh takes its place first; a long name is shortened in the
        // width left, with the whole name on hover, instead of widening the
        // list or running under the button.
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            refresh_button(ui, app, mailbox, messages);
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(mailbox.folder().name())
                            .heading()
                            .size(21.0)
                            .strong(),
                    )
                    .truncate(),
                );
            });
        });
    });
    ui.add_space(6.0);

    let mut query = mailbox.search_query().to_owned();
    // Center controls within one row, leaving the remaining height for mail.
    let search_size = egui::vec2(ui.available_width(), ui.spacing().interact_size.y);
    ui.allocate_ui_with_layout(search_size, Layout::right_to_left(Align::Center), |ui| {
        // Reserve the clear action before the field consumes the remaining width.
        if !query.is_empty() && ui.button("Clear").clicked() {
            messages.push(Message::SearchChanged(String::new()));
        }
        // The text starts where the rows' text does, inside the outline.
        let frame = theme::field_frame(ui).inner_margin(egui::Margin::symmetric(
            ROW_HORIZONTAL_PADDING as i8 - SEARCH_OUTLINE,
            8 - SEARCH_OUTLINE,
        ));
        let response = ui.add(
            theme::text_field(ui, &mut query)
                .id(egui::Id::new("mail-search"))
                .hint_text("Search mail…")
                .frame(frame)
                .desired_width(f32::INFINITY),
        );
        // The hint is only a placeholder, so the field gets a name of its own.
        ui.ctx()
            .accesskit_node_builder(response.id, |node| node.set_label("Search mail"));
        if state.focus_search {
            response.request_focus();
            state.focus_search = false;
        }
        if response.changed() {
            messages.push(Message::SearchChanged(query.clone()));
        }
        // A single-line field gives up focus on the Enter that submits it.
        if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
            messages.push(Message::SearchSubmitted);
        }
    });
    if !mailbox.search_query().is_empty() {
        detail(
            ui,
            &search_scope_label(mailbox.shown_search_query(), mailbox.loaded_count()),
        );
    }
    ui.add_space(4.0);

    let empty = mailbox.is_empty_visible();
    let muted = theme::colors(ui).muted;
    match (mailbox.status(), empty) {
        (ListStatus::Loading(_), _) => conversation_skeleton(ui),
        (ListStatus::Failed(error, attempt), true) => empty_state(ui, |ui| {
            state_message(ui, error.folder_message(), theme::colors(ui).danger);
            try_again(ui, attempt, messages);
        }),
        (_, true) if mailbox.shown_search_query().is_some() => empty_state(ui, |ui| {
            state_message(ui, "Proton found no mail matching that.", muted);
        }),
        (_, true) if mailbox.has_search_text() => empty_state(ui, |ui| {
            state_message(ui, "No matches in the conversations loaded so far.", muted);
            detail(ui, "Press Enter to search all of your mail.");
            load_more(ui, mailbox, messages);
        }),
        (_, true) => empty_state(ui, |ui| {
            let message = format!("No conversations in {}.", mailbox.folder().name());
            state_message(ui, &message, muted);
        }),
        (status, false) => {
            // With rows on screen a failure would leave no trace in the list.
            if let ListStatus::Failed(error, attempt) = status {
                failure_line(ui, error, attempt, messages);
            }
            conversation_list(ui, mailbox, state, messages);
        }
    }
}

/// Refresh, which says when a refresh someone asked for is on its way. The
/// app's own check every minute leaves it as it is, and it keeps the width
/// of its longer title, so nothing beside it moves.
fn refresh_button(ui: &mut egui::Ui, app: &App, mailbox: &Mailbox, messages: &mut Vec<Message>) {
    let quiet = app.refreshing_on_its_own();
    let shown = matches!(mailbox.status(), ListStatus::Refreshing(_)) && !quiet;
    let widest = egui::WidgetText::from("Refreshing…").into_galley(
        ui,
        Some(egui::TextWrapMode::Extend),
        f32::INFINITY,
        egui::TextStyle::Button,
    );
    let button = egui::Button::new(if shown { "Refreshing…" } else { "Refresh" }).min_size(
        egui::vec2(widest.size().x + 2.0 * ui.spacing().button_padding.x, 0.0),
    );
    // Pressed during the app's own refresh, it shows that one instead.
    if ui
        .add_enabled(!mailbox.is_busy() || quiet, button)
        .clicked()
    {
        messages.push(Message::RefreshMailbox);
    }
}

/// A list with nothing to show: what happened, centered, with anything that
/// can be done about it under it.
fn empty_state(ui: &mut egui::Ui, content: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(24.0);
    ui.vertical_centered(content);
}

fn state_message(ui: &mut egui::Ui, text: &str, color: Color32) {
    ui.add(egui::Label::new(egui::RichText::new(text).color(color)).wrap());
}

/// Asks again for what failed: the folder, its next page, or the search.
fn try_again(ui: &mut egui::Ui, attempt: ListAttempt, messages: &mut Vec<Message>) {
    if ui.add(theme::compact_button("Try again")).clicked() {
        messages.push(match attempt {
            ListAttempt::Folder => Message::RefreshMailbox,
            ListAttempt::More => Message::LoadMoreConversations,
            ListAttempt::Search => Message::SearchSubmitted,
        });
    }
}

/// One quiet line above rows that a failed request left in place.
fn failure_line(
    ui: &mut egui::Ui,
    error: MailboxError,
    attempt: ListAttempt,
    messages: &mut Vec<Message>,
) {
    // A password store to unlock needs its explanation; any other failure
    // keeps it for the tooltip.
    let text = if error == MailboxError::SessionUnavailable {
        error.folder_message()
    } else {
        match attempt {
            ListAttempt::Folder => "Couldn't refresh.",
            ListAttempt::More => "Couldn't load more.",
            ListAttempt::Search => "The search failed.",
        }
    };
    ui.horizontal_wrapped(|ui| {
        ui.add(
            egui::Label::new(
                egui::RichText::new(text)
                    .small()
                    .color(theme::colors(ui).danger),
            )
            .wrap(),
        )
        .on_hover_text(error.folder_message());
        try_again(ui, attempt, messages);
    });
    ui.add_space(4.0);
}

const UNDO_NOTICE_SECONDS: f64 = 6.0;
const UNDO_NOTICE_MARGIN: f32 = 20.0;

/// The offer to take back the last move. It sits at the foot of the list it
/// was made from, `list`, rather than across the divider beside it; without
/// a list on screen, at the foot of the window.
fn undo_notice(
    context: &egui::Context,
    mailbox: &Mailbox,
    state: &mut UiState,
    list: Option<egui::Rect>,
    messages: &mut Vec<Message>,
) {
    let Some(offer) = mailbox.undo() else {
        state.undo_notice = None;
        return;
    };
    let now = context.input(|input| input.time);
    let Some(remaining) = undo_notice_remaining(state, offer, now) else {
        messages.push(Message::DismissUndo(offer.clone()));
        return;
    };
    context.request_repaint_after(remaining);

    let area = egui::Area::new(egui::Id::new("undo-move-notice"));
    let area = match list {
        Some(list) => area
            .pivot(Align2::CENTER_BOTTOM)
            .fixed_pos(list.center_bottom() - egui::vec2(0.0, UNDO_NOTICE_MARGIN)),
        None => area.anchor(Align2::CENTER_BOTTOM, egui::vec2(0.0, -UNDO_NOTICE_MARGIN)),
    };
    let _ = area.order(egui::Order::Foreground).show(context, |ui| {
        theme::card(ui)
            .stroke(Stroke::new(1.0, theme::ACCENT))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(format!("Moved to {}.", offer.to.name()));
                    // The move back waits while the list loads or loads more.
                    let ready = mailbox.takes_actions();
                    if ui
                        .add_enabled(ready, theme::compact_button("Undo"))
                        .clicked()
                    {
                        messages.push(Message::UndoMove);
                    }
                });
            });
    });
}

fn undo_notice_remaining(
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

const FOLDER_HEIGHT: f32 = 31.0;
/// Virtualized rows must draw at this height to stay aligned with scrolling.
const ROW_HEIGHT: f32 = 60.0;
const ROW_GAP: f32 = 10.0;
/// From one row's top to the next one's: the step the virtualized list
/// places and scrolls its rows by.
const ROW_PITCH: f32 = ROW_HEIGHT + ROW_GAP;
const ROW_HORIZONTAL_PADDING: f32 = 18.0;
/// Room at a row's right for its time and marks; its text ends before it.
const ROW_META_WIDTH: f32 = 70.0;
/// The corners of a row, and of the outline drawn while its menu is open.
const ROW_CORNER_RADIUS: CornerRadius = CornerRadius::same(8);
/// The width of the search field's outline, inside its padding.
const SEARCH_OUTLINE: i8 = 1;
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
    search_query: String,
    shown_search_query: Option<String>,
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
        let first = (offset / ROW_PITCH).floor() as usize;
        let first = first.min(mailbox.visible_count().saturating_sub(1));
        Self {
            scroll_id,
            folder: mailbox.folder().clone(),
            search_query: mailbox.search_query().to_owned(),
            shown_search_query: mailbox.shown_search_query().map(str::to_owned),
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
            && self.search_query == mailbox.search_query()
            && self.shown_search_query.as_deref() == mailbox.shown_search_query()
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
            let delta = (index as f32 - anchor.index as f32) * ROW_PITCH;
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

    ui.scope_builder(
        egui::UiBuilder::new().id(egui::Id::new("mailbox-list-content")),
        |ui| {
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
                    scroll_row_into_view(ui, range.start, index);
                }

                for index in range {
                    let Some(conversation) = mailbox.visible_row(index) else {
                        load_more(ui, mailbox, messages);
                        continue;
                    };
                    let chosen = selected == Some(conversation.id.as_str());
                    list_row(ui, mailbox, conversation, chosen, &now, messages);
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
        },
    );
}

/// Scrolls the list just far enough to show the row at `index`, as a native
/// list does: not at all while the row is in view. `first` is the row that
/// `show_rows` draws at the top of `ui`.
fn scroll_row_into_view(ui: &mut egui::Ui, first: usize, index: usize) {
    let content_top = ui.max_rect().top() - first as f32 * ROW_PITCH;
    let row_rect = egui::Rect::from_min_size(
        egui::pos2(ui.max_rect().left(), content_top + index as f32 * ROW_PITCH),
        egui::vec2(ui.available_width(), ROW_HEIGHT),
    );
    ui.scroll_to_rect(row_rect, None);
}

/// One conversation of the list: its row, the selection a click asks for,
/// and its menu. As native lists do, the row a menu belongs to is outlined
/// while the menu is open.
fn list_row(
    ui: &mut egui::Ui,
    mailbox: &Mailbox,
    conversation: &ConversationSummary,
    selected: bool,
    now: &DateTime<Local>,
    messages: &mut Vec<Message>,
) {
    let response = ui
        .push_id(&conversation.id, |ui| {
            conversation_row(ui, conversation, selected, now)
        })
        .inner;
    if response.clicked() {
        messages.push(Message::SelectConversation(conversation.id.clone()));
    }
    let menu = response.context_menu(|ui| {
        reader::conversation_menu(ui, mailbox, conversation, messages);
    });
    if menu.is_some() {
        ui.painter().rect_stroke(
            response.rect,
            ROW_CORNER_RADIUS,
            Stroke::new(1.0, theme::ACCENT),
            egui::StrokeKind::Inside,
        );
    }
}

/// Whether the virtualized list needs a trailing load-more row.
fn ends_with_load_more(mailbox: &Mailbox) -> bool {
    matches!(mailbox.status(), ListStatus::LoadingMore(_)) || mailbox.has_more()
}

/// Where a row's text goes, for the row and its skeleton alike.
struct RowLines {
    /// The middle of the sender line.
    sender_y: f32,
    /// The middle of the subject line.
    subject_y: f32,
    /// Where both lines start.
    text_left: f32,
    /// Where both lines end, before the time and marks.
    text_right: f32,
    /// Where the time and marks end.
    meta_right: f32,
}

fn row_lines(rect: egui::Rect) -> RowLines {
    let meta_right = rect.right() - ROW_HORIZONTAL_PADDING;
    RowLines {
        sender_y: rect.center().y - ROW_LINE_CENTER_OFFSET,
        subject_y: rect.center().y + ROW_LINE_CENTER_OFFSET,
        text_left: rect.left() + ROW_HORIZONTAL_PADDING,
        text_right: meta_right - ROW_META_WIDTH,
        meta_right,
    }
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
        ROW_CORNER_RADIUS,
        hovered_fill,
        Stroke::NONE,
        egui::StrokeKind::Inside,
    );

    let lines = row_lines(rect);

    if conversation.unread {
        // On the sender line: at the row's centre it reads as belonging to
        // neither line.
        ui.painter().circle_filled(
            egui::pos2(rect.left() + 7.0, lines.sender_y),
            3.5,
            theme::ACCENT,
        );
    }

    let text_width = lines.text_right - lines.text_left;
    let painter = ui.painter_at(rect);
    let (sender_color, subject_color) = row_text_colors(ui, conversation.unread);
    theme::paint_truncated_text_aligned(
        &painter,
        egui::pos2(lines.text_left, lines.sender_y),
        Align2::LEFT_CENTER,
        correspondents,
        FontId::proportional(theme::ROW_TEXT_SIZE),
        sender_color,
        text_width,
    );
    theme::paint_truncated_text_aligned(
        &painter,
        egui::pos2(lines.text_left, lines.subject_y),
        Align2::LEFT_CENTER,
        subject,
        FontId::proportional(theme::ROW_TEXT_SIZE),
        subject_color,
        text_width,
    );

    let meta_font = FontId::proportional(theme::FOOTNOTE_SIZE);
    painter.text(
        egui::pos2(lines.meta_right, lines.sender_y),
        Align2::RIGHT_CENTER,
        &time,
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
            egui::pos2(lines.meta_right, lines.subject_y),
            Align2::RIGHT_CENTER,
            facts.join(&format!(" {} ", theme::DOT)),
            meta_font,
            theme::colors(ui).muted,
        )
        .left();
    if conversation.has_attachments {
        theme::paint_clip(
            &painter,
            egui::pos2(facts_left - 9.0, lines.subject_y),
            theme::colors(ui).muted,
        );
    }

    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::Button,
            true,
            selected,
            row_description(conversation, correspondents, subject, &time),
        )
    });
    // A row of a list rather than a link, so it keeps the arrow, as native
    // lists do.
    response
}

/// What a screen reader says for a row: everything the row shows, the
/// marks it draws included, in reading order.
fn row_description(
    conversation: &ConversationSummary,
    correspondents: &str,
    subject: &str,
    time: &str,
) -> String {
    let mut parts = Vec::new();
    if conversation.unread {
        parts.push("Unread".to_owned());
    }
    parts.push(correspondents.to_owned());
    parts.push(subject.to_owned());
    if !time.is_empty() {
        parts.push(time.to_owned());
    }
    if conversation.starred {
        parts.push("starred".to_owned());
    }
    if conversation.message_count > 1 {
        parts.push(format!("{} messages", conversation.message_count));
    }
    if conversation.has_attachments {
        parts.push("attachment".to_owned());
    }
    parts.join(", ")
}

fn conversation_skeleton(ui: &mut egui::Ui) {
    let fill = theme::skeleton_fill(ui);
    let available = ui.available_height();
    let rows = (((available + ROW_GAP) / ROW_PITCH).floor() as usize).clamp(1, 12);
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

/// The sender and subject colors of a row. The bundled font has one weight,
/// so unread mail stands out by brightness: both lines one step brighter
/// than in mail already read, besides the dot.
fn row_text_colors(ui: &egui::Ui, unread: bool) -> (Color32, Color32) {
    let visuals = ui.visuals();
    if unread {
        (visuals.strong_text_color(), visuals.text_color())
    } else {
        (visuals.text_color(), theme::colors(ui).muted)
    }
}

fn skeleton_conversation_row(ui: &mut egui::Ui, index: usize, fill: Color32) -> egui::Response {
    const SENDER_WIDTHS: [f32; 4] = [0.62, 0.48, 0.72, 0.56];
    const SUBJECT_WIDTHS: [f32; 4] = [0.44, 0.66, 0.52, 0.36];

    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, ROW_HEIGHT), Sense::hover());
    let painter = ui.painter_at(rect);
    let lines = row_lines(rect);
    let text_width = (lines.text_right - lines.text_left).max(24.0);
    let bar = |center_y: f32, width: f32, height: f32| {
        theme::paint_skeleton(
            &painter,
            egui::Rect::from_min_size(
                egui::pos2(lines.text_left, center_y - height * 0.5),
                egui::vec2(width, height),
            ),
            fill,
        );
    };
    bar(
        lines.sender_y,
        text_width * SENDER_WIDTHS[index % SENDER_WIDTHS.len()],
        11.0,
    );
    bar(
        lines.subject_y,
        text_width * SUBJECT_WIDTHS[index % SUBJECT_WIDTHS.len()],
        9.0,
    );
    theme::paint_skeleton(
        &painter,
        egui::Rect::from_center_size(
            egui::pos2(lines.meta_right - 18.0, lines.sender_y),
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
mod tests;
