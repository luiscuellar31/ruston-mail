use eframe::egui::AtomExt as _;
use eframe::egui::{self, Align, Align2, Color32, CornerRadius, FontId, Layout, Sense, Stroke};
use jiff::ZonedDateTime;

use super::{BottomNotice, TimedNotice, UiState, compose, reader, settings, theme};
use crate::accounts::MAX_ACCOUNTS;
use crate::app::{
    App, AuthState, DeviceIndex, ListAttempt, ListStatus, MIN_PANEL_WIDTH, Mailbox, Message,
    Notice, SettingsSection, compact_layout, panel_ratios, panel_widths, widths_beside_rail,
};
use crate::mail::{
    ConversationSummary, CustomKind, Folder, ListFilter, MailFolder, MailboxError, PlaceLook,
};
use crate::settings::{ComposePlacement, Panels, Settings};

const SIDEBAR_PANEL_ID: &str = "mailbox-sidebar";
const CONVERSATION_PANEL_ID: &str = "conversation-list";
/// The columns shown while a saved session opens, kept apart from the
/// mailbox's so their fixed widths never become its own.
const OPENING_SIDEBAR_ID: &str = "opening-sidebar";
/// The sidebar folded into a strip of icons, a panel of its own so the
/// sidebar's width is kept for when it unfolds.
const RAIL_PANEL_ID: &str = "mailbox-rail";
const OPENING_CONVERSATIONS_ID: &str = "opening-conversations";
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
        let settings = show_compact(root, app, mailbox, settings_draft, state, messages);
        bottom_notice(&context, app, mailbox, state, settings, messages);
        return;
    }
    let folded = app.settings().sidebar_folded;
    let rail = rail_width(theme::text_scale(&context));
    let widths = if folded {
        widths_beside_rail(app.panels(), window_width, rail)
    } else {
        panel_widths(app.panels(), window_width)
    };
    let inset = theme::titlebar_inset(&context);

    let sidebar = if folded {
        egui::Panel::left(RAIL_PANEL_ID)
            .default_size(rail)
            .size_range(rail..=rail)
            .resizable(false)
            .frame(rail_frame(root, inset))
            .show(root, |ui| folded_sidebar(ui, app, mailbox, messages))
    } else {
        egui::Panel::left(SIDEBAR_PANEL_ID)
            .default_size(widths.sidebar)
            .size_range(
                MIN_PANEL_WIDTH..=(window_width - 2.0 * MIN_PANEL_WIDTH).max(MIN_PANEL_WIDTH),
            )
            .resizable(true)
            .frame(theme::top_panel_frame(theme::colors(root).sidebar, inset))
            .show(root, |ui| {
                sidebar(ui, app, mailbox, SidebarPlace::Column, messages);
            })
    };

    // The list, and where a notice sits: at the foot of the list, or of
    // Settings' page, above its buttons.
    let (conversations, notice_place) = match settings_draft {
        Some(draft) => {
            let page = egui::CentralPanel::default()
                .frame(theme::top_panel_frame(theme::colors(root).panel, inset))
                .show(root, |ui| {
                    settings_pane(ui, app, draft, state, false, messages)
                })
                .inner;
            (None, page)
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
                .show(root, |ui| reading_pane(ui, app, mailbox, state, messages));
            (
                Some(conversations.response.rect),
                conversations.response.rect,
            )
        }
    };

    bottom_notice(&context, app, mailbox, state, Some(notice_place), messages);
    let conversations = conversations.map(|list| list.width());

    let stored = app.panels();
    let sidebar = sidebar.response.rect.width();
    let actual = match conversations {
        // Folded, the sidebar keeps the share it unfolds to, and only the
        // list can have been dragged.
        Some(conversations) if folded => Panels {
            conversations: panel_ratios(sidebar, conversations, window_width).conversations,
            ..stored
        },
        Some(conversations) => panel_ratios(sidebar, conversations, window_width),
        None if folded => stored,
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

/// The mailbox's columns while a saved session opens, as skeletons with the
/// widths they keep: the sidebar, the list, and an empty reader. A window
/// too narrow for them shows the list's alone.
pub(super) fn show_opening(root: &mut egui::Ui, app: &App) {
    let window_width = root.available_width();
    let inset = theme::titlebar_inset(root.ctx());
    if !compact_layout(window_width) {
        let folded = app.settings().sidebar_folded;
        let widths = if folded {
            widths_beside_rail(
                app.panels(),
                window_width,
                rail_width(theme::text_scale(root.ctx())),
            )
        } else {
            panel_widths(app.panels(), window_width)
        };
        egui::Panel::left(OPENING_SIDEBAR_ID)
            .default_size(widths.sidebar)
            .size_range(widths.sidebar..=widths.sidebar)
            .resizable(false)
            .frame(theme::top_panel_frame(theme::colors(root).sidebar, inset))
            .show(root, |ui| {
                // A folded sidebar opens as an empty strip.
                if !folded {
                    sidebar_skeleton(ui);
                }
            });
        egui::Panel::left(OPENING_CONVERSATIONS_ID)
            .default_size(widths.conversations)
            .size_range(widths.conversations..=widths.conversations)
            .resizable(false)
            .frame(theme::top_panel_frame(theme::colors(root).panel, inset))
            .show(root, conversation_skeleton);
    }
    egui::CentralPanel::default()
        .frame(theme::top_panel_frame(theme::colors(root).panel, inset))
        .show(root, |ui| {
            if compact_layout(window_width) {
                conversation_skeleton(ui);
            }
        });
}

/// The sidebar while a saved session opens: its name, and skeletons where
/// its button and Proton's folders go.
fn sidebar_skeleton(ui: &mut egui::Ui) {
    const NAME_WIDTHS: [f32; 4] = [0.42, 0.3, 0.5, 0.36];
    ui.heading(
        egui::RichText::new("Ruston Mail")
            .size(theme::text_size(ui, theme::APP_NAME_SIZE))
            .strong(),
    );
    ui.add_space(12.0);
    let fill = theme::skeleton_fill(ui);
    let width = ui.available_width();
    let scale = theme::text_scale(ui.ctx());
    let (button, _) = ui.allocate_exact_size(
        egui::vec2(width, NEW_MESSAGE_HEIGHT * scale),
        Sense::hover(),
    );
    theme::paint_skeleton(ui.painter(), button, fill);
    ui.add_space(12.0);
    for index in 0..MailFolder::ALL.len() {
        let (row, _) =
            ui.allocate_exact_size(egui::vec2(width, FOLDER_HEIGHT * scale), Sense::hover());
        let name = egui::Rect::from_min_size(
            egui::pos2(row.left() + 10.0, row.center().y - 5.0 * scale),
            egui::vec2(width * NAME_WIDTHS[index % NAME_WIDTHS.len()], 10.0 * scale),
        );
        theme::paint_skeleton(ui.painter(), name, fill);
    }
}

/// One column at a time. With Settings shown, returns where its page
/// scrolls, above its buttons.
fn show_compact(
    root: &mut egui::Ui,
    app: &App,
    mailbox: &Mailbox,
    settings_draft: Option<&mut Settings>,
    state: &mut UiState,
    messages: &mut Vec<Message>,
) -> Option<egui::Rect> {
    let inset = theme::titlebar_inset(root.ctx());
    let writing = app.settings().compose_placement == ComposePlacement::ReadingPane
        && app.compose().is_some_and(|draft| !draft.folded());
    let reader_available =
        writing || mailbox.selected_conversation().is_some() || !mailbox.selected_rows().is_empty();
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
                return Some(settings_pane(ui, app, draft, state, true, messages));
            }
            match state.compact_view {
                CompactView::Folders => {
                    sidebar(ui, app, mailbox, SidebarPlace::Page, messages);
                }
                CompactView::Mail => compact_mail(ui, app, mailbox, state, messages),
                CompactView::Reader => reading_pane(ui, app, mailbox, state, messages),
            }
            None
        })
        .inner
}

/// The list on its own, as one column shows it. The reader, which says why
/// an action failed, is a tab away there, so the list says it too.
fn compact_mail(
    ui: &mut egui::Ui,
    app: &App,
    mailbox: &Mailbox,
    state: &mut UiState,
    messages: &mut Vec<Message>,
) {
    if let Some(error) = mailbox.action_error() {
        ui.label(egui::RichText::new(error.action_message()).color(theme::colors(ui).danger));
        ui.add_space(4.0);
    }
    conversation_pane(ui, app, mailbox, state, messages);
}

/// Settings' page, in place of the list and the reader, with its sections
/// in a row above it when no sidebar lists them. Returns where it scrolls,
/// above its buttons.
fn settings_pane(
    ui: &mut egui::Ui,
    app: &App,
    draft: &mut Settings,
    state: &UiState,
    sections_row: bool,
    messages: &mut Vec<Message>,
) -> egui::Rect {
    settings::show(
        ui,
        app,
        draft,
        messages,
        state.notification_status,
        sections_row,
    )
}

fn reading_pane(
    ui: &mut egui::Ui,
    app: &App,
    mailbox: &Mailbox,
    state: &mut UiState,
    messages: &mut Vec<Message>,
) {
    let in_pane = app.settings().compose_placement == ComposePlacement::ReadingPane;
    if in_pane && let Some(writing) = app.compose().filter(|draft| !draft.folded()) {
        compose::show_in_pane(ui, writing, messages);
    } else {
        // A draft folded while a conversation was chosen waits above it.
        if in_pane && let Some(folded) = app.compose() {
            compose::folded_strip(ui, folded, messages);
        }
        let view = reader::ReaderView {
            mailbox,
            places: app.folders(),
            actions_available: app.mailbox_actions_available(),
            saves: reader::AttachmentSaves {
                saving: app.saving_attachment(),
                failed: app.failed_save(),
                images: Some(app.inline_images()),
            },
            reading: app.settings().reading(),
        };
        reader::show(ui, &view, state, messages);
    }
}

/// Where the sidebar shows: beside the list, where it can fold into a strip
/// of icons, or on its own, as one column shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SidebarPlace {
    Column,
    Page,
}

fn sidebar(
    ui: &mut egui::Ui,
    app: &App,
    mailbox: &Mailbox,
    place: SidebarPlace,
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
            // A bottom panel needs one measured frame before its height is
            // known. Schedule that frame even if the pointer is stationary,
            // but only while the height is new: a block too tall to fit at
            // all would otherwise ask for every frame.
            let height = account.response.rect.height();
            let measured = ui.id().with("sidebar-account-height");
            let remeasured = ui.ctx().data_mut(|data| {
                let changed = data.get_temp::<f32>(measured) != Some(height);
                data.insert_temp(measured, height);
                changed
            });
            if remeasured && account.response.rect.bottom() > available.bottom() + 1.0 {
                ui.ctx().request_repaint();
            }
            ui.spacing_mut().scroll = theme::panel_scroll_style();
            egui::ScrollArea::vertical()
                .id_salt("sidebar-scroll")
                .min_scrolled_height(0.0)
                .auto_shrink([false, false])
                .show(ui, |ui| sidebar_content(ui, app, mailbox, place, messages))
        },
    )
    .inner
}

fn sidebar_content(
    ui: &mut egui::Ui,
    app: &App,
    mailbox: &Mailbox,
    place: SidebarPlace,
    messages: &mut Vec<Message>,
) {
    let fold = theme::icon_button_size(ui);
    let title = ui
        .scope(|ui| {
            // Beside the list, the title leaves the button that folds the
            // sidebar its room at the right.
            if place == SidebarPlace::Column {
                ui.set_max_width(ui.available_width() - fold.x - ui.spacing().item_spacing.x);
            }
            ui.heading(
                egui::RichText::new("Ruston Mail")
                    .size(theme::text_size(ui, theme::APP_NAME_SIZE))
                    .strong(),
            )
        })
        .inner;
    if place == SidebarPlace::Column {
        let button = egui::Rect::from_center_size(
            egui::pos2(ui.max_rect().right() - fold.x / 2.0, title.rect.center().y),
            fold,
        );
        // Placed by hand, in a child that takes no room from the sidebar: a
        // button counted in its width would widen it on every frame.
        if ui
            .new_child(egui::UiBuilder::new().max_rect(button))
            .add(theme::IconButton::new(theme::Icon::Sidebar, "Hide sidebar"))
            .clicked()
        {
            messages.push(Message::FoldSidebar(true));
        }
    }
    ui.add_space(12.0);

    // While Settings is open, the sidebar lists its sections instead, as
    // Proton's web client does.
    if app.showing_settings() {
        settings_places(ui, app, messages);
        return;
    }
    if new_message_button(ui).clicked() {
        messages.push(Message::OpenCompose);
    }
    ui.add_space(12.0);

    let current = Some(mailbox.folder());
    for folder in MailFolder::ALL.into_iter().map(Folder::System) {
        folder_button(ui, mailbox, current, folder, PlaceLook::default(), messages);
    }

    // The account's own folders and labels each come under a heading, as
    // in Proton's web client, so a folder of its own is not read as one of
    // Proton's.
    for (heading, kind) in [
        ("FOLDERS", CustomKind::Folder),
        ("LABELS", CustomKind::Label),
    ] {
        let places: Vec<_> = app
            .folders()
            .iter()
            .filter(|folder| folder.kind() == Some(kind))
            .cloned()
            .collect();
        if !places.is_empty() {
            ui.add_space(14.0);
            detail(ui, heading);
            for place in places {
                let look = app.place_look(&place);
                if look.depth == 0 {
                    folder_button(ui, mailbox, current, place, look, messages);
                } else {
                    // A subfolder sits under its parent, indented a step
                    // for each folder it is in.
                    let mut row = ui.available_rect_before_wrap();
                    row.min.x += f32::from(look.depth) * SUBFOLDER_INDENT;
                    ui.scope_builder(egui::UiBuilder::new().max_rect(row), |ui| {
                        folder_button(ui, mailbox, current, place, look, messages);
                    });
                }
            }
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
        let account_label = if app.is_demo_process() {
            "demo@example.com"
        } else {
            email.unwrap_or("Proton Mail account")
        };
        let label = egui::RichText::new(account_label)
            .size(theme::text_size(ui, theme::CAPTION_SIZE))
            .color(theme::colors(ui).muted);
        if app.is_demo_process() || signing_out {
            ui.add(egui::Label::new(label).truncate())
                .on_hover_text(account_label);
        } else {
            accounts_menu(ui, app, label, messages).on_hover_text(account_label);
        }
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
            egui::Button::new(logout_label).min_size(egui::vec2(
                ui.available_width(),
                theme::text_size(ui, ACCOUNT_BUTTON_HEIGHT),
            )),
        );
        if sending {
            logout = logout.on_disabled_hover_text("Available once the message is sent");
        }
        if logout.clicked() {
            messages.push(Message::Logout);
        }
    });
}

/// The open account's address, which opens the accounts to switch to and a
/// way to add another, up to [`MAX_ACCOUNTS`]. A chevron after it says so.
fn accounts_menu(
    ui: &mut egui::Ui,
    app: &App,
    label: egui::RichText,
    messages: &mut Vec<Message>,
) -> egui::Response {
    let chevron = egui::Atom::custom(
        egui::Id::new(ACCOUNT_CHEVRON_ATOM_ID),
        egui::Vec2::splat(ACCOUNT_CHEVRON_SIZE),
    );
    let button = egui::Button::new((label, chevron))
        .frame(false)
        .wrap_mode(egui::TextWrapMode::Truncate);
    // Leaving would hide whether a message being sent has left.
    let sending = app.compose().is_some_and(crate::app::Compose::in_flight);
    let accounts = app.accounts();
    let response = egui::containers::menu::MenuButton::from_button(button)
        .ui(ui, |ui| {
            for account in accounts.all() {
                let open = account.profile == accounts.active_profile();
                let name = account.address.as_deref().unwrap_or("Proton Mail account");
                // Where the command line finds the same session.
                let item = ui
                    .add_enabled(open || !sending, egui::Button::selectable(open, name))
                    .on_hover_text(format!("ruston-cli --profile {}", account.profile));
                if item.clicked() && !open {
                    messages.push(Message::SwitchAccount(account.profile.clone()));
                    ui.close();
                }
            }
            ui.separator();
            let add = ui.add_enabled(
                app.can_add_account() && !sending,
                egui::Button::new("Add account"),
            );
            let add = if app.can_add_account() {
                add
            } else {
                add.on_disabled_hover_text(format!("Up to {MAX_ACCOUNTS} accounts"))
            };
            if add.clicked() {
                messages.push(Message::AddAccount);
                ui.close();
            }
        })
        .0;
    paint_menu_chevron(ui, &response);
    response
}

/// Paints the chevron in the room the address leaves at the end of its
/// button, which has no frame to pad it; brighter while pointed at or open.
fn paint_menu_chevron(ui: &egui::Ui, response: &egui::Response) {
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(response));
    let color = if open || response.hovered() {
        ui.visuals().strong_text_color()
    } else {
        theme::colors(ui).muted
    };
    let center = egui::pos2(
        response.rect.right() - ACCOUNT_CHEVRON_SIZE / 2.0,
        response.rect.center().y,
    );
    ui.painter().line(
        reader::chevron_points(center, true).to_vec(),
        Stroke::new(1.5, color),
    );
}

/// A neutral button like the one under it; while Settings is open, the soft
/// violet of the open folder.
fn settings_button(ui: &mut egui::Ui, selected: bool) -> egui::Response {
    let button = egui::Button::new(place_text(ui, "Settings", selected)).min_size(egui::vec2(
        ui.available_width(),
        theme::text_size(ui, ACCOUNT_BUTTON_HEIGHT),
    ));
    let button = chosen_place(ui, button, selected);
    ui.add(button)
}

/// The height Settings and the button under it share.
const ACCOUNT_BUTTON_HEIGHT: f32 = 32.0;

/// Words on a place of the sidebar, in the chosen place's color when it is
/// the one open.
fn place_text(ui: &egui::Ui, text: impl Into<String>, chosen: bool) -> egui::RichText {
    let text = egui::RichText::new(text);
    if chosen {
        text.color(theme::colors(ui).selected_folder_text)
    } else {
        text
    }
}

/// `button` with the soft violet of the place that is open, when it is.
fn chosen_place<'a>(ui: &egui::Ui, button: egui::Button<'a>, chosen: bool) -> egui::Button<'a> {
    if chosen {
        button
            .fill(theme::colors(ui).accent_soft)
            .stroke(Stroke::NONE)
    } else {
        button
    }
}

const NEW_MESSAGE_LABEL_ID: &str = "new-message-label";
const FOLDER_NAME_ATOM_ID: &str = "folder-name";
const ACCOUNT_CHEVRON_ATOM_ID: &str = "account-chevron";
/// The room the accounts menu's chevron takes after the address.
const ACCOUNT_CHEVRON_SIZE: f32 = 12.0;

fn new_message_button(ui: &mut egui::Ui) -> egui::AtomLayoutResponse {
    let width = ui.available_width();
    ui.scope(|ui| {
        theme::primary_visuals(ui);
        egui::Button::new((
            egui::Atom::grow(),
            egui::RichText::new("New message")
                .size(theme::text_size(ui, theme::BODY_SIZE))
                .color(theme::ON_ACCENT)
                .atom_id(egui::Id::new(NEW_MESSAGE_LABEL_ID)),
            egui::Atom::grow(),
        ))
        .min_size(egui::vec2(width, theme::text_size(ui, NEW_MESSAGE_HEIGHT)))
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
    look: PlaceLook,
    messages: &mut Vec<Message>,
) {
    let icon = folder_icon(&folder);
    let unread = mailbox
        .counts()
        .and_then(|counts| counts.unread(&folder))
        .filter(|count| *count > 0);
    // A label's icon has the label's color.
    let icon_color = (icon == theme::Icon::Label).then(|| label_color(look));
    let chosen = current == Some(&folder);
    if place_button(ui, icon, icon_color, folder.name(), unread, chosen).clicked() {
        messages.push(Message::SelectFolder(folder));
    }
}

/// One place of the sidebar: its icon, in `icon_color` or else its text's,
/// its name, and how many unread it holds at the right. The chosen one has
/// the soft violet and its name in bold. A screen reader hears the name,
/// and the unread.
fn place_button(
    ui: &mut egui::Ui,
    icon: theme::Icon,
    icon_color: Option<Color32>,
    name: &str,
    unread: Option<u32>,
    selected: bool,
) -> egui::Response {
    let text = place_text(ui, name, selected).atom_id(egui::Id::new(FOLDER_NAME_ATOM_ID));
    let mut button = egui::Button::new((theme::icon_atom(), text))
        .gap(ui.spacing().icon_spacing + 3.0)
        .wrap_mode(egui::TextWrapMode::Truncate)
        .min_size(egui::vec2(
            ui.available_width(),
            theme::text_size(ui, FOLDER_HEIGHT),
        ))
        .selected(selected)
        .frame(true)
        .frame_when_inactive(selected);
    if let Some(count) = unread {
        button = button.right_text(place_text(ui, count.to_string(), selected));
    }
    let layout = place_layout(ui, chosen_place(ui, button, selected));
    let icon_color = icon_color.unwrap_or_else(|| {
        if selected {
            theme::colors(ui).selected_folder_text
        } else {
            ui.style().interact(&layout.response).fg_stroke.color
        }
    });
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
            name,
            egui::TextStyle::Button.resolve(ui.style()),
            theme::colors(ui).selected_folder_text,
            rect.width() - offset,
        );
    }
    let response = layout.response;
    response.widget_info(|| {
        let label = unread.map_or_else(
            || name.to_owned(),
            |count| format!("{name}, {count} unread"),
        );
        egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, label)
    });
    response
}

/// Lays out a place of the sidebar. egui's grey hover is brighter than the
/// soft violet of the selection; a place under the pointer gets a quieter
/// tint of it.
fn place_layout(ui: &mut egui::Ui, button: egui::Button<'_>) -> egui::AtomLayoutResponse {
    ui.scope(|ui| {
        let colors = theme::colors(ui);
        let widgets = &mut ui.visuals_mut().widgets;
        widgets.hovered.weak_bg_fill = colors.sidebar_hover;
        widgets.hovered.bg_stroke = Stroke::NONE;
        widgets.active.weak_bg_fill = colors.accent_soft;
        widgets.active.bg_stroke = Stroke::NONE;
        button.atom_ui(ui)
    })
    .inner
}

/// Settings' sections, in place of the folders while Settings is open, under
/// the way back to the mailbox where New message is.
fn settings_places(ui: &mut egui::Ui, app: &App, messages: &mut Vec<Message>) {
    if back_to_mail_button(ui).clicked() {
        messages.push(Message::ShowSettings(false));
    }
    ui.add_space(12.0);
    let open = app.settings_section();
    for section in SettingsSection::ALL {
        let (icon, title) = (settings::icon(section), settings::title(section));
        if place_button(ui, icon, None, title, None, section == open).clicked() {
            messages.push(Message::ShowSettingsSection(section));
        }
    }
}

/// The way back to the mailbox, as wide and tall as New message: an arrow
/// and "Mail", as Proton's web client has it, and "Back to mail" to a screen
/// reader, as the button at the foot of Settings says.
fn back_to_mail_button(ui: &mut egui::Ui) -> egui::Response {
    let width = ui.available_width();
    let layout = egui::Button::new((
        egui::Atom::grow(),
        theme::icon_atom(),
        egui::RichText::new("Mail").size(theme::text_size(ui, theme::BODY_SIZE)),
        egui::Atom::grow(),
    ))
    .min_size(egui::vec2(width, theme::text_size(ui, NEW_MESSAGE_HEIGHT)))
    .atom_ui(ui);
    let color = ui.style().interact(&layout.response).fg_stroke.color;
    theme::paint_atom_icon(ui, &layout, theme::Icon::Back, color);
    let response = layout.response;
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Back to mail"));
    response
}

/// Settings' sections in the folded sidebar, under the way back to the
/// mailbox, each named on hover and to screen readers.
fn settings_rail(ui: &mut egui::Ui, app: &App, messages: &mut Vec<Message>) {
    if rail_place(ui, theme::Icon::Back, "Back to mail", false, false).clicked() {
        messages.push(Message::ShowSettings(false));
    }
    ui.add_space(12.0);
    let open = app.settings_section();
    for section in SettingsSection::ALL {
        let (icon, title) = (settings::icon(section), settings::title(section));
        if rail_place(ui, icon, title, section == open, false).clicked() {
            messages.push(Message::ShowSettingsSection(section));
        }
    }
}

/// How wide the folded sidebar is: its icons, as tall as a place of the
/// sidebar, with room either side.
fn rail_width(scale: f32) -> f32 {
    FOLDER_HEIGHT * scale + 2.0 * f32::from(RAIL_PADDING)
}

/// The folded sidebar's frame: the sidebar's, with less room either side.
fn rail_frame(ui: &egui::Ui, inset: f32) -> egui::Frame {
    let mut frame = theme::top_panel_frame(theme::colors(ui).sidebar, inset);
    frame.inner_margin.left = RAIL_PADDING;
    frame.inner_margin.right = RAIL_PADDING;
    frame
}

/// The sidebar folded into a strip of icons, top to bottom: the way to
/// unfold it, writing a new message, Proton's folders, the account's
/// folders and labels behind a menu each, and Settings at the foot; while
/// Settings is open, the way back to the mailbox and its sections in place
/// of writing and the folders. Each says its name on hover and to screen
/// readers.
fn folded_sidebar(ui: &mut egui::Ui, app: &App, mailbox: &Mailbox, messages: &mut Vec<Message>) {
    egui::Panel::bottom("rail-settings")
        .frame(egui::Frame::NONE.inner_margin(egui::Margin {
            top: theme::PANEL_PADDING,
            ..Default::default()
        }))
        .show(ui, |ui| {
            if rail_place(
                ui,
                theme::Icon::Settings,
                "Settings",
                app.showing_settings(),
                false,
            )
            .clicked()
            {
                messages.push(Message::ShowSettings(true));
            }
        });
    ui.spacing_mut().scroll = theme::panel_scroll_style();
    egui::ScrollArea::vertical()
        .id_salt("rail-scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if rail_place(ui, theme::Icon::Sidebar, "Show sidebar", false, false).clicked() {
                messages.push(Message::FoldSidebar(false));
            }
            ui.add_space(12.0);
            if app.showing_settings() {
                settings_rail(ui, app, messages);
                return;
            }
            if rail_new_message(ui).clicked() {
                messages.push(Message::OpenCompose);
            }
            ui.add_space(12.0);
            let current = Some(mailbox.folder());
            for folder in MailFolder::ALL.into_iter().map(Folder::System) {
                let unread = mailbox
                    .counts()
                    .and_then(|counts| counts.unread(&folder))
                    .filter(|count| *count > 0);
                let name = unread.map_or_else(
                    || folder.name().to_owned(),
                    |count| format!("{}, {count} unread", folder.name()),
                );
                let icon = folder_icon(&folder);
                let chosen = current == Some(&folder);
                if rail_place(ui, icon, &name, chosen, unread.is_some()).clicked() {
                    messages.push(Message::SelectFolder(folder));
                }
            }
            for (kind, icon, name) in [
                (CustomKind::Folder, theme::Icon::Folder, "Folders"),
                (CustomKind::Label, theme::Icon::Label, "Labels"),
            ] {
                let places: Vec<&Folder> = app
                    .folders()
                    .iter()
                    .filter(|folder| folder.kind() == Some(kind))
                    .collect();
                if places.is_empty() {
                    continue;
                }
                let chosen = current.is_some_and(|open| places.contains(&open));
                let button = rail_place(ui, icon, name, chosen, false);
                theme::menu_below(ui, &button, |ui| {
                    for place in places {
                        let depth = f32::from(app.place_look(place).depth);
                        ui.horizontal(|ui| {
                            ui.add_space(depth * SUBFOLDER_INDENT);
                            let open = current == Some(place);
                            if ui
                                .add(egui::Button::selectable(open, place.name()))
                                .clicked()
                            {
                                messages.push(Message::SelectFolder(place.clone()));
                            }
                        });
                    }
                });
            }
        });
}

/// One icon of the folded sidebar, chosen as a place of the sidebar is, with
/// a dot when it holds unread mail.
fn rail_place(
    ui: &mut egui::Ui,
    icon: theme::Icon,
    name: &str,
    chosen: bool,
    unread: bool,
) -> egui::Response {
    let side = theme::text_size(ui, FOLDER_HEIGHT);
    let button = egui::Button::new(theme::icon_atom())
        .min_size(egui::vec2(side, side))
        .selected(chosen)
        .frame(true)
        .frame_when_inactive(chosen);
    let layout = place_layout(ui, chosen_place(ui, button, chosen));
    let color = if chosen {
        theme::colors(ui).selected_folder_text
    } else {
        ui.style().interact(&layout.response).fg_stroke.color
    };
    theme::paint_atom_icon(ui, &layout, icon, color);
    let response = layout.response;
    if unread {
        ui.painter().circle_filled(
            response.rect.right_top() + egui::vec2(-RAIL_DOT_INSET, RAIL_DOT_INSET),
            LABEL_DOT_RADIUS,
            theme::ACCENT,
        );
    }
    response
        .widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, chosen, name));
    response.on_hover_text(name)
}

/// New message, folded into its icon, in the accent as the sidebar's button.
fn rail_new_message(ui: &mut egui::Ui) -> egui::Response {
    let side = theme::text_size(ui, FOLDER_HEIGHT);
    let layout = ui
        .scope(|ui| {
            theme::primary_visuals(ui);
            egui::Button::new(theme::icon_atom())
                .min_size(egui::vec2(side, side))
                .stroke(Stroke::NONE)
                .atom_ui(ui)
        })
        .inner;
    theme::paint_atom_icon(ui, &layout, theme::Icon::Compose, theme::ON_ACCENT);
    let response = layout.response;
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "New message"));
    response.on_hover_text("New message")
}

/// The color a label's icon wears: the one it has in Proton, as Proton's own
/// apps show it, or the accent for one Ruston cannot read.
fn label_color(look: PlaceLook) -> Color32 {
    look.color
        .map_or(theme::ACCENT, |[r, g, b]| Color32::from_rgb(r, g, b))
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
    // What the server found comes from every folder, not the one open.
    let title = if mailbox.shown_search_query().is_some() {
        "Search results"
    } else {
        mailbox.folder().name()
    };
    ui.horizontal(|ui| {
        // Refresh takes its place first; a long name is shortened in the
        // width left, with the whole name on hover, instead of widening the
        // list or running under the button.
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            refresh_button(ui, app, mailbox, messages);
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(title)
                            .heading()
                            .size(theme::text_size(ui, theme::LIST_TITLE_SIZE))
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
        // Reserve the filter and the clear action before the field consumes
        // the remaining width.
        unread_filter(ui, mailbox, messages);
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
    // The line stays while nothing is typed, so the first key does not push
    // the list down; text too long for it is shortened, whole on hover.
    let scope = if mailbox.search_query().is_empty() {
        String::new()
    } else if let Some(why) = mailbox.device_index() {
        device_index_label(why).to_owned()
    } else {
        search_scope_label(
            mailbox.shown_search_query(),
            mailbox.shown_search_is_on_device(),
            mailbox.loaded_count(),
        )
    };
    ui.add(
        egui::Label::new(
            egui::RichText::new(scope)
                .small()
                .color(theme::colors(ui).muted),
        )
        .truncate(),
    );
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
            let found_nothing = if mailbox.shown_search_is_on_device() {
                "This device's search index has no mail matching that."
            } else {
                "Proton found no mail matching that."
            };
            state_message(ui, found_nothing, muted);
        }),
        (_, true) if mailbox.has_search_text() => empty_state(ui, |ui| {
            state_message(ui, "No matches in the conversations loaded so far.", muted);
            detail(ui, "Press Enter to search all of your mail.");
            load_more(ui, mailbox, messages);
        }),
        (_, true) if mailbox.filter() == ListFilter::Unread => empty_state(ui, |ui| {
            let message = format!("No unread conversations in {}.", mailbox.folder().name());
            state_message(ui, &message, muted);
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
            conversation_list(ui, mailbox, &row_labels(app), state, messages);
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

/// Shows only the folder's unread conversations, or all of them again;
/// selected while it does.
fn unread_filter(ui: &mut egui::Ui, mailbox: &Mailbox, messages: &mut Vec<Message>) {
    let unread_only = mailbox.filter() == ListFilter::Unread;
    if ui
        .add(egui::Button::new("Unread").selected(unread_only))
        .on_hover_text("Show only unread conversations")
        .clicked()
    {
        messages.push(Message::ShowOnly(if unread_only {
            ListFilter::All
        } else {
            ListFilter::Unread
        }));
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
    if ui.add(theme::compact_button(ui, "Try again")).clicked() {
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

const NOTICE_MARGIN: f32 = 20.0;

#[cfg(target_os = "macos")]
const REVEAL_LABEL: &str = "Show in Finder";
#[cfg(target_os = "windows")]
const REVEAL_LABEL: &str = "Show in Explorer";
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const REVEAL_LABEL: &str = "Show in folder";

/// Where the pointer is, for the bottom notice's time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pointer {
    Away,
    /// Resting on the notice, which keeps it until the pointer leaves.
    OnNotice,
}

/// The notice at the foot of `place`: the list it is about, rather than
/// across the divider beside it, or Settings' page, above its buttons; with
/// neither on screen, at the foot of the window. It offers to take back the
/// last move or, with none to take back, confirms a message sent or a file
/// saved, or announces a newer release.
fn bottom_notice(
    context: &egui::Context,
    app: &App,
    mailbox: &Mailbox,
    state: &mut UiState,
    place: Option<egui::Rect>,
    messages: &mut Vec<Message>,
) {
    let notice = match (mailbox.undo(), app.notice()) {
        (Some(offer), _) => BottomNotice::Undo(Box::new(offer.clone())),
        (None, Some(notice)) => BottomNotice::Confirmation(notice.clone()),
        (None, None) => {
            state.bottom_notice = None;
            return;
        }
    };
    let id = egui::Id::new("bottom-notice");
    let now = context.input(|input| input.time);
    // The notice is where it was laid out last frame.
    let pointer = match (
        context.memory(|memory| memory.area_rect(id)),
        context.pointer_hover_pos(),
    ) {
        (Some(rect), Some(position)) if rect.contains(position) => Pointer::OnNotice,
        _ => Pointer::Away,
    };
    let lasts = std::time::Duration::from_secs(app.settings().notice_seconds);
    let Some(remaining) = notice_remaining(state, &notice, now, pointer, lasts) else {
        messages.push(match notice {
            BottomNotice::Undo(offer) => Message::DismissUndo(*offer),
            BottomNotice::Confirmation(notice) => Message::DismissNotice(notice),
        });
        return;
    };
    context.request_repaint_after(remaining);

    let area = egui::Area::new(id);
    let area = match place {
        Some(place) => area
            .pivot(Align2::CENTER_BOTTOM)
            .fixed_pos(place.center_bottom() - egui::vec2(0.0, NOTICE_MARGIN)),
        None => area.anchor(Align2::CENTER_BOTTOM, egui::vec2(0.0, -NOTICE_MARGIN)),
    };
    // Words that would run past their place, or the window, wrap instead.
    let room = place.unwrap_or_else(|| context.content_rect()).width() - 2.0 * NOTICE_MARGIN;
    let _ = area.order(egui::Order::Foreground).show(context, |ui| {
        ui.set_max_width(room);
        theme::card(ui)
            .stroke(Stroke::new(1.0, theme::ACCENT))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| notice_content(ui, &notice, mailbox, messages));
            });
    });
}

/// What a bottom notice says, and what it offers to do.
fn notice_content(
    ui: &mut egui::Ui,
    notice: &BottomNotice,
    mailbox: &Mailbox,
    messages: &mut Vec<Message>,
) {
    match notice {
        BottomNotice::Undo(offer) => {
            notice_words(
                ui,
                match offer.rows.len() {
                    1 => format!("Moved to {}.", offer.to.name()),
                    moved => format!("Moved {moved} conversations to {}.", offer.to.name()),
                },
            );
            // The move back waits while the list loads or loads more.
            let ready = mailbox.takes_actions();
            if ui
                .add_enabled(ready, theme::compact_button(ui, "Undo"))
                .clicked()
            {
                messages.push(Message::UndoMove);
            }
        }
        BottomNotice::Confirmation(Notice::Sent) => {
            notice_words(ui, "Message sent.");
        }
        BottomNotice::Confirmation(Notice::UpdateAvailable(version)) => {
            notice_words(ui, format!("Ruston Mail {version} is available."));
            settings::download_button(ui, messages);
        }
        BottomNotice::Confirmation(Notice::Saved(path)) => {
            let name = path.file_name().map_or_else(
                || path.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            );
            notice_words(ui, format!("Saved {name}.")).on_hover_text(path.display().to_string());
            if ui.add(theme::compact_button(ui, REVEAL_LABEL)).clicked() {
                messages.push(Message::RevealAttachment(path.clone()));
            }
        }
    }
}

/// Adds a notice's `words` where its button follows. Words that fit on one
/// line share it with the button, both centered on a line as high as the
/// button; longer words wrap with their own spacing, and the button goes
/// below them.
fn notice_words(ui: &mut egui::Ui, words: impl Into<egui::WidgetText>) -> egui::Response {
    let words = words.into();
    let one_line = words
        .clone()
        .into_galley(
            ui,
            Some(egui::TextWrapMode::Extend),
            f32::INFINITY,
            egui::FontSelection::Default,
        )
        .size()
        .x
        <= ui.available_width();
    // A wrapping row starts shorter than the button, and would center a
    // label's first line on that shorter height.
    if one_line {
        ui.set_row_height(theme::compact_button_height(ui));
    }
    let response = ui.label(words);
    if !one_line {
        ui.end_row();
    }
    response
}

/// How long `notice` stays from `now`: its own time, `lasts`, from when it
/// first showed, started again while the pointer rests on it. `None` once it
/// is up.
fn notice_remaining(
    state: &mut UiState,
    notice: &BottomNotice,
    now: f64,
    pointer: Pointer,
    lasts: std::time::Duration,
) -> Option<std::time::Duration> {
    let timed = match &mut state.bottom_notice {
        Some(timed) if timed.notice == *notice => timed,
        other => other.insert(TimedNotice {
            notice: notice.clone(),
            expires_at: now + lasts.as_secs_f64(),
        }),
    };
    if pointer == Pointer::OnNotice {
        timed.expires_at = now + lasts.as_secs_f64();
    }
    let remaining = timed.expires_at - now;
    (remaining > 0.0).then(|| std::time::Duration::from_secs_f64(remaining))
}

const FOLDER_HEIGHT: f32 = 31.0;
const NEW_MESSAGE_HEIGHT: f32 = 34.0;
/// How far each folder a subfolder sits in moves it right.
const SUBFOLDER_INDENT: f32 = 14.0;
/// Virtualized rows must draw at this height, times the scale text is drawn
/// at, to stay aligned with scrolling.
const ROW_HEIGHT: f32 = 60.0;
const ROW_GAP: f32 = 10.0;
const ROW_HORIZONTAL_PADDING: f32 = 18.0;
/// Room at a row's right for its time and marks; its text ends before it.
const ROW_META_WIDTH: f32 = 70.0;
/// The corners of a row, and of the outline drawn while its menu is open.
const ROW_CORNER_RADIUS: CornerRadius = CornerRadius::same(8);
/// The room either side of the folded sidebar's icons.
const RAIL_PADDING: i8 = 10;
/// How far a folder's unread dot sits from its icon's corner.
const RAIL_DOT_INSET: f32 = 6.0;
/// The most labels a row shows as dots; it counts the rest.
const ROW_LABEL_DOTS: usize = 3;
/// A label's dot on a row, the size of the unread one, and the room between
/// two of them.
const LABEL_DOT_RADIUS: f32 = 3.5;
const LABEL_DOT_GAP: f32 = 3.0;
/// How far the paperclip reaches either side of its middle.
const CLIP_HALF_WIDTH: f32 = 2.5;
/// The room between the marks at the end of a subject line, and before them.
const MARK_GAP: f32 = 6.0;
/// The width of the search field's outline, inside its padding.
const SEARCH_OUTLINE: i8 = 1;
const ROW_LINE_CENTER_OFFSET: f32 = 12.5;

/// A row's height when text is drawn at `scale`.
fn row_height(scale: f32) -> f32 {
    ROW_HEIGHT * scale
}

/// From one row's top to the next one's when text is drawn at `scale`: the
/// step the virtualized list places and scrolls its rows by.
fn row_pitch(scale: f32) -> f32 {
    row_height(scale) + ROW_GAP
}

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
    /// The step between rows the offset was taken at.
    pitch: f32,
    anchors: [Option<RowAnchor>; 2],
}

impl ConversationScroll {
    fn capture(
        mailbox: &Mailbox,
        scroll_id: egui::Id,
        offset: f32,
        pitch: f32,
        previous: Option<&Self>,
    ) -> Self {
        let previous = previous.filter(|saved| saved.same_view(mailbox, scroll_id));
        let first = (offset / pitch).floor() as usize;
        let first = first.min(mailbox.visible_count().saturating_sub(1));
        Self {
            scroll_id,
            folder: mailbox.folder().clone(),
            search_query: mailbox.search_query().to_owned(),
            shown_search_query: mailbox.shown_search_query().map(str::to_owned),
            offset,
            pitch,
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
            let delta = (index as f32 - anchor.index as f32) * self.pitch;
            Some((self.offset + delta).max(0.0))
        })
    }
}

fn conversation_list(
    ui: &mut egui::Ui,
    mailbox: &Mailbox,
    labels: &[RowLabel],
    state: &mut UiState,
    messages: &mut Vec<Message>,
) {
    let now = ZonedDateTime::now();
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
            // viewport after a background listing update. Another folder or
            // search starts at its top, not where the last one was left.
            if reveal.is_none()
                && let Some(saved) = state.conversation_scroll.as_ref()
            {
                if !saved.same_view(mailbox, scroll_id) {
                    scroll = scroll.vertical_scroll_offset(0.0);
                } else if let Some(offset) = saved.offset_after_update(mailbox, scroll_id) {
                    scroll = scroll.vertical_scroll_offset(offset);
                }
            }

            let scale = theme::text_scale(ui.ctx());
            let output = scroll.show_rows(ui, row_height(scale), total, |ui, range| {
                if let Some(index) = reveal {
                    scroll_row_into_view(ui, range.start, index);
                }

                for index in range {
                    let Some(conversation) = mailbox.visible_row(index) else {
                        load_more(ui, mailbox, messages);
                        continue;
                    };
                    let chosen = mailbox.is_chosen(&conversation.id);
                    list_row(ui, mailbox, conversation, labels, chosen, &now, messages);
                }
            });
            state.conversation_scroll = Some(ConversationScroll::capture(
                mailbox,
                output.id,
                output.state.offset.y,
                row_pitch(scale),
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
    let scale = theme::text_scale(ui.ctx());
    let content_top = ui.max_rect().top() - first as f32 * row_pitch(scale);
    let row_rect = egui::Rect::from_min_size(
        egui::pos2(
            ui.max_rect().left(),
            content_top + index as f32 * row_pitch(scale),
        ),
        egui::vec2(ui.available_width(), row_height(scale)),
    );
    ui.scroll_to_rect(row_rect, None);
}

/// One conversation of the list: its row, the selection a click asks for,
/// and its menu. As native lists do, Command (Ctrl elsewhere) adds a row to
/// those chosen or takes it out, Shift chooses the rows from the last one
/// clicked, the menu of a row chosen with others acts on all of them, and the
/// row a menu belongs to is outlined while the menu is open.
fn list_row(
    ui: &mut egui::Ui,
    mailbox: &Mailbox,
    conversation: &ConversationSummary,
    labels: &[RowLabel],
    selected: bool,
    now: &ZonedDateTime,
    messages: &mut Vec<Message>,
) {
    let response = ui
        .push_id(&conversation.id, |ui| {
            conversation_row(ui, conversation, labels, selected, now)
        })
        .inner;
    if response.clicked() {
        let held = click_modifiers(ui);
        let id = conversation.id.clone();
        messages.push(if held.shift {
            Message::SelectThrough(id)
        } else if held.command {
            Message::ToggleSelected(id)
        } else {
            Message::SelectConversation(id)
        });
    }
    let menu = response.context_menu(|ui| {
        if mailbox.selected_together(&conversation.id) {
            reader::selection_menu(ui, mailbox, messages);
        } else {
            reader::conversation_menu(ui, mailbox, conversation, messages);
        }
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

/// The keys held as this pass's click was made: those the main button went up
/// with, since a key let go in the same pass is no longer held at its end. A
/// click from the keyboard has only what is held now.
fn click_modifiers(ui: &egui::Ui) -> egui::Modifiers {
    ui.input(|input| {
        input
            .events
            .iter()
            .rev()
            .find_map(|event| match event {
                egui::Event::PointerButton {
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers,
                    ..
                } => Some(*modifiers),
                _ => None,
            })
            .unwrap_or(input.modifiers)
    })
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

/// The lines of a row drawn in `rect`, with text drawn at `scale`.
fn row_lines(rect: egui::Rect, scale: f32) -> RowLines {
    let meta_right = rect.right() - ROW_HORIZONTAL_PADDING;
    RowLines {
        sender_y: rect.center().y - ROW_LINE_CENTER_OFFSET * scale,
        subject_y: rect.center().y + ROW_LINE_CENTER_OFFSET * scale,
        text_left: rect.left() + ROW_HORIZONTAL_PADDING,
        text_right: meta_right - ROW_META_WIDTH * scale,
        meta_right,
    }
}

/// One of the account's labels, as a conversation row marks it.
struct RowLabel<'a> {
    id: &'a str,
    name: &'a str,
    color: Color32,
}

/// The account's labels, in the sidebar's order, as rows mark them.
fn row_labels(app: &App) -> Vec<RowLabel<'_>> {
    app.folders()
        .iter()
        .filter_map(|folder| match folder {
            Folder::Custom {
                id,
                name,
                kind: CustomKind::Label,
            } => Some(RowLabel {
                id,
                name,
                color: label_color(app.place_look(folder)),
            }),
            _ => None,
        })
        .collect()
}

/// Paints a dot in each label's color, the first ones up to
/// [`ROW_LABEL_DOTS`], and how many more there are, ending at `right` on the
/// line at `y`. Where they went, or `None` without a label.
fn paint_label_dots(
    painter: &egui::Painter,
    labels: &[&RowLabel],
    right: f32,
    y: f32,
    font: FontId,
    muted: Color32,
) -> Option<egui::Rect> {
    if labels.is_empty() {
        return None;
    }
    let mut painted = egui::Rect::NOTHING;
    let mut left = right;
    let more = labels.len().saturating_sub(ROW_LABEL_DOTS);
    if more > 0 {
        let count = painter.text(
            egui::pos2(left, y),
            Align2::RIGHT_CENTER,
            format!("+{more}"),
            font,
            muted,
        );
        painted = painted.union(count);
        left = count.left() - LABEL_DOT_GAP;
    }
    // The first label ends up leftmost, as names read.
    for label in labels.iter().take(ROW_LABEL_DOTS).rev() {
        let center = egui::pos2(left - LABEL_DOT_RADIUS, y);
        painter.circle_filled(center, LABEL_DOT_RADIUS, label.color);
        painted = painted.union(egui::Rect::from_center_size(
            center,
            egui::Vec2::splat(2.0 * LABEL_DOT_RADIUS),
        ));
        left = center.x - LABEL_DOT_RADIUS - LABEL_DOT_GAP;
    }
    Some(painted)
}

fn conversation_row(
    ui: &mut egui::Ui,
    conversation: &ConversationSummary,
    labels: &[RowLabel],
    selected: bool,
    now: &ZonedDateTime,
) -> egui::Response {
    let correspondents = conversation
        .correspondents
        .as_deref()
        .unwrap_or("(Unknown)");
    // The account's labels this conversation carries, in the sidebar's order.
    let marks: Vec<&RowLabel> = labels
        .iter()
        .filter(|label| conversation.labels.iter().any(|id| id == label.id))
        .collect();
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
    let scale = theme::text_scale(ui.ctx());
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, row_height(scale)), Sense::click());
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

    let lines = row_lines(rect, scale);

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
        FontId::proportional(theme::ROW_TEXT_SIZE * scale),
        sender_color,
        text_width,
    );

    let meta_font = FontId::proportional(theme::FOOTNOTE_SIZE * scale);
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
    let mut marks_left = facts_left;
    if conversation.has_attachments {
        let clip = egui::pos2(facts_left - 9.0, lines.subject_y);
        theme::paint_clip(&painter, clip, theme::colors(ui).muted);
        marks_left = clip.x - CLIP_HALF_WIDTH;
    }
    let dots = paint_label_dots(
        &painter,
        &marks,
        marks_left - MARK_GAP,
        lines.subject_y,
        FontId::proportional(theme::FOOTNOTE_SIZE * scale),
        theme::colors(ui).muted,
    );
    // The subject gives way to the dots, never the other way round.
    let subject_right = dots.map_or(lines.text_right, |dots| {
        (dots.left() - MARK_GAP).min(lines.text_right)
    });
    theme::paint_truncated_text_aligned(
        &painter,
        egui::pos2(lines.text_left, lines.subject_y),
        Align2::LEFT_CENTER,
        subject,
        FontId::proportional(theme::ROW_TEXT_SIZE * scale),
        subject_color,
        subject_right - lines.text_left,
    );

    let label_names: Vec<&str> = marks.iter().map(|label| label.name).collect();
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::Button,
            true,
            selected,
            row_description(conversation, correspondents, subject, &time, &label_names),
        )
    });
    // The dots' names, for the pointer resting on them.
    let response = match dots {
        Some(dots) if ui.rect_contains_pointer(dots.expand(LABEL_DOT_GAP)) => {
            response.on_hover_text_at_pointer(label_names.join(", "))
        }
        _ => response,
    };
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
    labels: &[&str],
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
    if !labels.is_empty() {
        parts.push(format!("labeled {}", labels.join(", ")));
    }
    parts.join(", ")
}

fn conversation_skeleton(ui: &mut egui::Ui) {
    let fill = theme::skeleton_fill(ui);
    let available = ui.available_height();
    let pitch = row_pitch(theme::text_scale(ui.ctx()));
    let rows = (((available + ROW_GAP) / pitch).floor() as usize).clamp(1, 12);
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
    let scale = theme::text_scale(ui.ctx());
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, row_height(scale)), Sense::hover());
    let painter = ui.painter_at(rect);
    let lines = row_lines(rect, scale);
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
        11.0 * scale,
    );
    bar(
        lines.subject_y,
        text_width * SUBJECT_WIDTHS[index % SUBJECT_WIDTHS.len()],
        9.0 * scale,
    );
    theme::paint_skeleton(
        &painter,
        egui::Rect::from_center_size(
            egui::pos2(lines.meta_right - 18.0 * scale, lines.sender_y),
            egui::vec2(36.0 * scale, 8.0 * scale),
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

fn search_scope_label(results: Option<&str>, on_device: bool, loaded: usize) -> String {
    match (results, loaded) {
        (Some(query), _) if on_device => format!(
            "Proton could not be reached: showing what this device's search index found for “{query}”."
        ),
        (Some(query), _) => format!("Showing what the server found for “{query}”."),
        (None, 1) => "Narrowing 1 loaded conversation · Enter searches all mail".to_owned(),
        (None, loaded) => {
            format!("Narrowing {loaded} loaded conversations · Enter searches all mail")
        }
    }
}

/// Why a search Proton could not answer has nothing from this device.
fn device_index_label(why: DeviceIndex) -> &'static str {
    match why {
        DeviceIndex::Missing => {
            "Proton could not be reached, and this device has no search index to look in."
        }
        DeviceIndex::Unreadable => {
            "Proton could not be reached, and this device's search index could not be read."
        }
    }
}

/// When a row's mail came, as someone reading at `now` would say it, in the
/// zone `now` is in. A time no date can hold shows nothing.
fn format_time(timestamp: i64, now: &ZonedDateTime) -> String {
    let Ok(time) = jiff::Timestamp::from_second(timestamp) else {
        return String::new();
    };
    let time = time.to_zoned_datetime(now.time_zone().clone());
    let today = now.date();
    if today.yesterday().ok() == Some(time.date()) {
        return "Yesterday".to_owned();
    }
    let format = if time.date() == today {
        "%H:%M"
    } else if time.year() == now.year() {
        "%b %-d"
    } else {
        "%Y-%m-%d"
    };
    time.strftime(format).to_string()
}

#[cfg(test)]
mod tests;
