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
        ui.heading(
            egui::RichText::new(mailbox.folder().name())
                .size(21.0)
                .strong(),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            refresh_button(ui, app, mailbox, messages);
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
mod tests {
    use chrono::Utc;

    use super::*;
    use crate::mail::{ConversationDetail, ConversationPage, MailboxError, SummaryKind};
    use crate::ui::test_support::visible_text_rect;

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
                app.update(Message::ConversationsLoaded(
                    1,
                    Ok(pagination_page(0, 50, 120)),
                ));
                app.update(Message::SearchChanged(query.into()));
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
                let first_row = visible_text_rect(&shapes, "p0")
                    .expect("loaded mail must be visible below search");
                assert!(first_row.top() > search.bottom());
                assert!(first_row.bottom() <= size.y);
                if !query.is_empty() {
                    let clear =
                        visible_text_rect(&shapes, "Clear").expect("Clear must remain visible");
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
        app.update(Message::FoldersLoaded(
            1,
            Ok((0..30)
                .map(|n| Folder::label(format!("label-{n}"), format!("Label {n}")))
                .collect()),
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
                app.update(Message::SearchChanged("needle".into()));
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
                        let (messages, shapes) = mailbox_frame(
                            &context,
                            &app,
                            &mut state,
                            size,
                            None,
                            Default::default(),
                        );
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
                                        visible_text_rect(&shapes, label).unwrap_or_else(
                                            || panic!("{label} not visible at zoom {zoom}")
                                        )
                                    ),
                                    "{label} clipped at zoom {zoom}"
                                );
                            }
                        } else {
                            assert!(
                                window
                                    .contains_rect(visible_text_rect(&shapes, "Clear").expect(
                                        "search clear action must fit in the compact pane"
                                    ))
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
        app.update(Message::ConversationsLoaded(
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
        app.update(Message::ConversationsLoaded(
            1,
            Ok(pagination_page(0, 5, 5)),
        ));
        app.update(Message::SearchChanged("invoice".into()));
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
            egui::containers::panel::PanelState::load(
                &context,
                egui::Id::new(CONVERSATION_PANEL_ID)
            )
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
                    egui::epaint::Shape::Text(text) if text.galley.text() == wanted => {
                        Some(text.pos.x)
                    }
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
        context
            .all_styles_mut(|style| style.scroll_animation = egui::style::ScrollAnimation::none());
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
        app.update(Message::ConversationsLoaded(
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
        app.update(Message::ConversationsLoaded(
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

        app.update(Message::SelectConversation("demo-3".into()));
        state.compact_view = CompactView::Reader;
        mailbox_frame(&context, &app, &mut state, size, None, Default::default());
        assert_eq!(state.compact_view, CompactView::Reader);
    }

    #[test]
    fn the_offer_to_undo_sits_under_the_list_it_was_made_from() {
        let (mut app, _) = App::boot(true, Settings::default());
        app.update(Message::ConversationsLoaded(
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
        app.update(Message::SelectConversation("demo-3".into()));
        app.update(Message::ApplyAction(crate::mail::MailAction::MoveTo(
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
        app.update(Message::ConversationsLoaded(
            1,
            Ok(ConversationPage {
                conversations: vec![pagination_row("demo-3", 1000)],
                total: 1,
                inspect_candidates: Vec::new(),
            }),
        ));
        app.update(Message::SelectConversation("demo-3".into()));
        app.update(Message::ApplyAction(crate::mail::MailAction::MoveTo(
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
        app.update(Message::ConversationsLoaded(
            1,
            Ok(ConversationPage {
                conversations: vec![pagination_row("demo-3", 1000)],
                total: 1,
                inspect_candidates: Vec::new(),
            }),
        ));
        app.update(Message::SelectConversation("demo-3".into()));
        app.update(Message::ApplyAction(crate::mail::MailAction::MoveTo(
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
                folder_button(ui, &mailbox, current, Folder::INBOX, &mut Vec::new());
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
            row: None,
        };
        let mut state = UiState::default();

        assert_eq!(
            undo_notice_remaining(&mut state, &offer, 10.0),
            Some(std::time::Duration::from_secs(6))
        );
        assert_eq!(
            undo_notice_remaining(&mut state, &offer, 15.0),
            Some(std::time::Duration::from_secs(1))
        );
        assert_eq!(undo_notice_remaining(&mut state, &offer, 16.0), None);

        let next = UndoMove {
            row_id: "second".into(),
            ..offer
        };
        assert_eq!(
            undo_notice_remaining(&mut state, &next, 16.0),
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
                folder_button(
                    ui,
                    &mailbox,
                    Some(mailbox.folder()),
                    label.clone(),
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
}
