use std::path::Path;

use eframe::egui::{self, Align, Layout};

use super::theme::{self, Emphasis};
use crate::app::{App, Message, SettingsSection, UpdateStatus};
use crate::mail::{BodyFormat, MailFolder};
use crate::settings::{
    Appearance, ComposePlacement, MAX_NOTICE_SECONDS, MIN_NOTICE_SECONDS, Settings, StartFolder,
    TEXT_SCALES,
};

/// The section of Settings that is open, with Apply and Back to mail at its
/// foot. `sections_row` lists the sections above it, for a window whose
/// sidebar does not. Returns where the page scrolls, above those buttons,
/// which a notice must leave uncovered.
pub(super) fn show(
    root: &mut egui::Ui,
    app: &App,
    draft: &mut Settings,
    messages: &mut Vec<Message>,
    notification_status: super::notifications::Status,
    sections_row: bool,
) -> egui::Rect {
    let current = app.settings();
    let section = app.settings_section();
    egui::Panel::bottom("settings-actions")
        .frame(
            theme::panel_frame(theme::colors(root).panel).outer_margin(egui::Margin {
                top: theme::PANEL_PADDING,
                ..Default::default()
            }),
        )
        .show(root, |ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Back to mail").clicked() {
                    messages.push(Message::ShowSettings(false));
                }

                let changes = preference_changes(current, draft);
                if ui
                    .add_enabled(
                        !changes.is_empty(),
                        theme::primary(egui::Button::new("Apply")),
                    )
                    .clicked()
                {
                    messages.extend(changes);
                }
            });
        });

    root.scope(|ui| {
        ui.spacing_mut().scroll = theme::panel_scroll_style();
        // Each section keeps where it was scrolled to.
        egui::ScrollArea::vertical()
            .id_salt(("settings-page", section))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.heading(
                    egui::RichText::new("Settings")
                        .size(theme::text_size(ui, theme::PAGE_TITLE_SIZE))
                        .strong(),
                );
                ui.add_space(18.0);
                if sections_row {
                    sections(ui, section, messages);
                    ui.add_space(14.0);
                }
                page(
                    ui,
                    section,
                    draft,
                    messages,
                    notification_status,
                    app.third_party_notices(),
                    app.update_status(),
                );
            });
    })
    .response
    .rect
}

/// What each section is called, in the navigation and on its card.
pub(super) fn title(section: SettingsSection) -> &'static str {
    match section {
        SettingsSection::Reading => "Reading",
        SettingsSection::Writing => "Writing",
        SettingsSection::Appearance => "Appearance",
        SettingsSection::General => "General",
        SettingsSection::Advanced => "Advanced",
        SettingsSection::Keyboard => "Keyboard",
        SettingsSection::About => "About",
    }
}

/// The icon each section has in the sidebar, as each folder has one.
pub(super) fn icon(section: SettingsSection) -> theme::Icon {
    match section {
        SettingsSection::Reading => theme::Icon::Reading,
        SettingsSection::Writing => theme::Icon::Compose,
        SettingsSection::Appearance => theme::Icon::Palette,
        SettingsSection::General => theme::Icon::Toggles,
        SettingsSection::Advanced => theme::Icon::Sliders,
        SettingsSection::Keyboard => theme::Icon::Keyboard,
        SettingsSection::About => theme::Icon::About,
    }
}

/// The sections as a row of buttons, the open one chosen, where no sidebar
/// lists them.
fn sections(ui: &mut egui::Ui, open: SettingsSection, messages: &mut Vec<Message>) {
    ui.horizontal_wrapped(|ui| {
        for section in SettingsSection::ALL {
            if ui
                .add(egui::Button::selectable(section == open, title(section)))
                .clicked()
            {
                messages.push(Message::ShowSettingsSection(section));
            }
        }
    });
}

#[derive(Clone, Copy)]
pub(super) enum ExitDecision {
    KeepEditing,
    Discard,
    Apply,
}

pub(super) fn confirm_exit(context: &egui::Context) -> Option<ExitDecision> {
    theme::confirmation(
        context,
        "unsaved-settings",
        "Unsaved settings",
        "Apply your changes before leaving Settings?",
        &[
            (
                "Keep editing",
                Emphasis::Secondary,
                ExitDecision::KeepEditing,
            ),
            ("Discard changes", Emphasis::Danger, ExitDecision::Discard),
            ("Apply and continue", Emphasis::Primary, ExitDecision::Apply),
        ],
        ExitDecision::KeepEditing,
    )
}

/// The card of one section.
fn page(
    ui: &mut egui::Ui,
    section: SettingsSection,
    settings: &mut Settings,
    messages: &mut Vec<Message>,
    notification_status: super::notifications::Status,
    third_party_notices: Option<&Path>,
    update_status: Option<UpdateStatus>,
) {
    match section {
        SettingsSection::Reading => reading(ui, settings),
        SettingsSection::Writing => writing(ui, settings),
        SettingsSection::Appearance => appearance(ui, settings, messages),
        SettingsSection::General => general(ui, settings, notification_status),
        SettingsSection::Advanced => advanced(ui, settings),
        SettingsSection::Keyboard => keyboard(ui),
        SettingsSection::About => {
            about(ui, settings, messages, third_party_notices, update_status);
        }
    }
}

/// What opens with a conversation, how much of what its sender wrote shows,
/// and what a message may open or show beyond its own text.
fn reading(ui: &mut egui::Ui, settings: &mut Settings) {
    section(ui, title(SettingsSection::Reading), |ui| {
        ui.add(theme::Checkbox::new(
            &mut settings.mark_read_on_open,
            "Mark mail as read when I open it",
        ));
        description(
            ui,
            "With this off, mail stays unread until you mark it yourself.",
        );
        ui.add_space(6.0);

        ui.add(theme::Checkbox::new(
            &mut settings.expand_all_messages,
            "Open every message in a conversation",
        ));
        description(
            ui,
            "With this off, only the newest opens and the rest wait behind their headers.",
        );
        ui.add_space(6.0);

        ui.add(theme::Checkbox::new(
            &mut settings.show_quoted_text,
            "Show quoted text without unfolding it",
        ));
        description(
            ui,
            "Quoted passages are the thread repeated under a reply, so they stay folded by default.",
        );
        ui.add_space(6.0);

        ui.add(theme::Checkbox::new(
            &mut settings.show_hidden_text,
            "Show text the sender hid",
        ));
        description(
            ui,
            "Senders hide text such as a newsletter's preview line, or a letter slipped into a word to get past filters. Shown, it is grey.",
        );
        ui.add_space(6.0);

        ui.add(theme::Checkbox::new(
            &mut settings.sender_text_styles,
            "Use the sender's text styles",
        ));
        description(
            ui,
            "Bold, italic, struck-through and centered text, and links drawn as buttons, which newsletters set with styles rather than tags. With this off, only the tags count.",
        );
        ui.add_space(6.0);

        ui.add(theme::Checkbox::new(
            &mut settings.data_tables,
            "Show tables of data in columns",
        ));
        description(
            ui,
            "Receipts, invoices and timetables keep their columns. Tables that only lay a newsletter out read as text either way.",
        );
        ui.add_space(6.0);

        ui.add(theme::Checkbox::new(
            &mut settings.confirm_links,
            "Ask before opening a link",
        ));
        description(
            ui,
            "The prompt shows the real destination, which helps expose misleading links.",
        );
        ui.add_space(6.0);

        ui.add(theme::Checkbox::new(
            &mut settings.show_inline_images,
            "Show pictures a message carries within it",
        ));
        description(
            ui,
            "They come with the message, so nothing is fetched to show them. Pictures on the web are never downloaded, which protects your privacy from tracking pixels.",
        );
    });
}

/// Where a message is written and how it is sent.
fn writing(ui: &mut egui::Ui, settings: &mut Settings) {
    section(ui, title(SettingsSection::Writing), |ui| {
        ui.horizontal_wrapped(|ui| {
            for (placement, label) in [
                (ComposePlacement::ReadingPane, "Reading pane"),
                (ComposePlacement::Window, "New window"),
            ] {
                ui.selectable_value(&mut settings.compose_placement, placement, label);
            }
        });
        description(ui, "Where Write, Reply and Forward open.");
        // eframe gives only the main window an accessibility tree.
        description(
            ui,
            "Screen readers cannot read a new window yet, so with one, keep the reading pane.",
        );
        ui.add_space(10.0);

        ui.horizontal_wrapped(|ui| {
            for (format, label) in [
                (BodyFormat::PlainText, "Plain text"),
                (BodyFormat::Html, "HTML"),
            ] {
                ui.selectable_value(&mut settings.compose_format, format, label);
            }
        });
        description(
            ui,
            "How a message you write is sent. Either way you write text: a tag you type is shown as you typed it, never obeyed.",
        );
    });
}

/// How the window looks: its scheme, font and sizes.
fn appearance(ui: &mut egui::Ui, settings: &mut Settings, messages: &mut Vec<Message>) {
    section(ui, title(SettingsSection::Appearance), |ui| {
        appearance_choice(ui, settings);
        ui.add_space(8.0);
        ui.add(theme::Checkbox::new(
            &mut settings.system_font,
            "Use system interface font",
        ));
        description(ui, "Keeps text sizes. Uses Ruston's font if unavailable.");
        ui.add_space(8.0);
        let label = ui.label("Interface scale");
        egui::ComboBox::from_id_salt("interface-scale")
            .selected_text(percent_label(settings.zoom))
            .width(100.0)
            .show_ui(ui, |ui| {
                for step in ZOOM_STEPS {
                    ui.selectable_value(&mut settings.zoom, step, percent_label(step));
                }
            })
            .response
            .labelled_by(label.id);
        description(
            ui,
            "Scales everything, not the text alone, so the window keeps its proportions.",
        );
        ui.add_space(8.0);
        let label = ui.label("Text size");
        egui::ComboBox::from_id_salt("text-size")
            .selected_text(percent_label(settings.text_scale))
            .width(100.0)
            .show_ui(ui, |ui| {
                for scale in TEXT_SCALES {
                    ui.selectable_value(&mut settings.text_scale, scale, percent_label(scale));
                }
            })
            .response
            .labelled_by(label.id);
        description(
            ui,
            "Makes the text alone larger or smaller. Rows and buttons grow or shrink with it.",
        );
        ui.add_space(12.0);
        if ui.button("Reset window and pane sizes").clicked() {
            messages.push(Message::ResetLayout);
        }
        description(ui, "Restores the default layout immediately.");
    });
}

/// The folder a run opens in, and what the app tells the system and the
/// window's foot.
fn general(
    ui: &mut egui::Ui,
    settings: &mut Settings,
    notification_status: super::notifications::Status,
) {
    section(ui, title(SettingsSection::General), |ui| {
        let label = ui.label("Folder at start");
        egui::ComboBox::from_id_salt("start-folder")
            .selected_text(start_label(settings.start))
            .width(180.0)
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut settings.start,
                    StartFolder::LastRead,
                    "Where I left off",
                );
                for folder in MailFolder::ALL {
                    ui.selectable_value(
                        &mut settings.start,
                        StartFolder::Always(folder),
                        folder.name(),
                    );
                }
            })
            .response
            .labelled_by(label.id);
        description(
            ui,
            "A folder the account made cannot be pinned: it could be renamed or gone by the next run.",
        );
        ui.add_space(8.0);
        if let Some((label, what_it_does)) = UNREAD_BADGE {
            ui.add(theme::Checkbox::new(&mut settings.show_unread_badge, label));
            description(ui, what_it_does);
            ui.add_space(8.0);
        }
        ui.add(theme::Checkbox::new(
            &mut settings.desktop_notifications,
            "Show desktop notifications for new mail",
        ));
        description(
            ui,
            "Displays a system notification with the sender and subject when new mail arrives.",
        );
        if settings.desktop_notifications {
            description(ui, notification_status.description());
        }
        ui.add_space(8.0);
        let label = ui.label("Notice duration");
        egui::ComboBox::from_id_salt("notice-seconds")
            .selected_text(seconds_label(settings.notice_seconds))
            .width(120.0)
            .show_ui(ui, |ui| {
                for seconds in MIN_NOTICE_SECONDS..=MAX_NOTICE_SECONDS {
                    ui.selectable_value(
                        &mut settings.notice_seconds,
                        seconds,
                        seconds_label(seconds),
                    );
                }
            })
            .response
            .labelled_by(label.id);
        description(
            ui,
            "How long notices at the foot of the window stay, such as Message sent or the offer to undo a move. A notice also stays while the pointer rests on it.",
        );
    });
}

/// Dark, light or, where the system tells which it is in, the system's own.
/// Choosing a scheme stops following the system.
fn appearance_choice(ui: &mut egui::Ui, settings: &mut Settings) {
    ui.label("Theme");
    // winit tells the system's mode on macOS and Windows, not on Linux.
    let offer_system = !cfg!(target_os = "linux");
    ui.horizontal_wrapped(|ui| {
        // Following the system keeps the scheme last chosen, for where
        // the system tells none.
        if offer_system
            && ui
                .selectable_label(settings.follow_system_appearance, "System")
                .clicked()
        {
            settings.follow_system_appearance = true;
        }
        for (appearance, label) in [(Appearance::Dark, "Dark"), (Appearance::Light, "Light")] {
            let chosen = !settings.follow_system_appearance && settings.appearance == appearance;
            if ui.selectable_label(chosen, label).clicked() {
                settings.follow_system_appearance = false;
                settings.appearance = appearance;
            }
        }
    });
    if offer_system {
        description(ui, "System follows your computer's light or dark mode.");
    }
}

/// What the unread count on the app's icon is called on this system and what
/// it does there, for the systems that have such an icon.
const UNREAD_BADGE: Option<(&str, &str)> = if cfg!(target_os = "macos") {
    Some((
        "Show unread count badge on the Dock icon",
        "Updates the Dock icon with the number of unread messages in your inbox.",
    ))
} else if cfg!(target_os = "windows") {
    Some((
        "Show unread count on the taskbar button",
        "Marks the taskbar button with the number of unread messages in your inbox, \
         while badges are on in Windows' taskbar settings.",
    ))
} else if cfg!(target_os = "linux") {
    Some((
        "Show unread count on the dock icon",
        "Shows the number of unread messages in your inbox on docks that support it, \
         such as Ubuntu's dock and KDE's task manager.",
    ))
} else {
    None
};

/// Choices that ask Proton, or this device, for more than the defaults do.
fn advanced(ui: &mut egui::Ui, settings: &mut Settings) {
    section(ui, title(SettingsSection::Advanced), |ui| {
        ui.add(theme::Checkbox::new(
            &mut settings.check_recipient_protection,
            "Check recipient encryption while writing",
        ));
        description(
            ui,
            "When you leave a recipient field, Proton is asked for that address's keys to show whether the message will be end-to-end encrypted. With this off, nothing is asked until you send.",
        );
        ui.add_space(6.0);

        ui.add(theme::Checkbox::new(
            &mut settings.accept_international_domains,
            "Accept international domain names",
        ));
        description(
            ui,
            "Lets you write to a domain such as münchen.de. It is sent in its ASCII form, xn--mnchen-3ya.de, which the composer shows beside it: letters from another script can imitate a familiar domain. Email links still refuse such domains.",
        );
        ui.add_space(6.0);

        ui.add(theme::Checkbox::new(
            &mut settings.search_device_offline,
            "Search this device when Proton cannot be reached",
        ));
        description(
            ui,
            "A search then looks in the index that ruston-cli index builds on this device, which Ruston Mail reads without changing. That index holds your mail decrypted and unencrypted; Ruston Mail never writes it, and opening a result still needs Proton.",
        );
    });
}

/// The keys the mailbox answers to.
fn keyboard(ui: &mut egui::Ui) {
    section(ui, title(SettingsSection::Keyboard), |ui| {
        description(
            ui,
            "A key a focused field takes never reaches the mailbox, so these stay out of the way while you type.",
        );
        ui.add_space(6.0);
        let width = keys_width(ui);
        for (keys, what) in SHORTCUTS {
            shortcut(ui, keys, what, width);
        }
    });
}

/// The version running, where it comes from, and whether a newer one is out.
fn about(
    ui: &mut egui::Ui,
    settings: &mut Settings,
    messages: &mut Vec<Message>,
    third_party_notices: Option<&Path>,
    update_status: Option<UpdateStatus>,
) -> egui::Response {
    section(ui, title(SettingsSection::About), |ui| {
        ui.horizontal_top(|ui| {
            ui.add(
                egui::Image::new(egui::include_image!("../../assets/ui/ruston-mail-128.png"))
                    .fit_to_exact_size(egui::Vec2::splat(40.0))
                    .alt_text("Ruston Mail logo"),
            );
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new(concat!("Ruston Mail ", env!("CARGO_PKG_VERSION")))
                        .size(theme::text_size(ui, theme::ABOUT_TITLE_SIZE))
                        .strong(),
                );
                description(ui, "A native Proton Mail client.");
            });
        });
        ui.add_space(8.0);
        description(
            ui,
            "Built with Rust, egui, and proton-crypto. Not affiliated with Proton.",
        );
        ui.add(
            egui::AtomLayout::new((
                egui::RichText::new("Made with").color(theme::colors(ui).muted),
                egui::Image::new(egui::include_image!(
                    "../../assets/icons/bootstrap/heart.svg"
                ))
                .fit_to_exact_size(egui::Vec2::splat(14.0))
                .tint(theme::ACCENT)
                .alt_text("heart"),
                egui::RichText::new(concat!(
                    "by Luis Cuellar. ",
                    env!("CARGO_PKG_LICENSE"),
                    " License."
                ))
                .color(theme::colors(ui).muted),
            ))
            .gap(ui.spacing().item_spacing.x),
        );
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            if ui.add(theme::compact_button(ui, "Source code")).clicked() {
                messages.push(Message::OpenSourceCode);
            }
            if third_party_notices.is_some()
                && ui
                    .add(theme::compact_button(ui, "Third-party notices"))
                    .clicked()
            {
                messages.push(Message::OpenThirdPartyNotices);
            }
        });
        ui.add_space(12.0);
        ui.add(theme::Checkbox::new(
            &mut settings.check_for_updates,
            "Check for updates",
        ));
        description(
            ui,
            "As the app starts and once a day, GitHub is asked for the latest release. Nothing is downloaded: a newer version is announced, with a link to its page.",
        );
        if let Some(status) = update_status {
            update_found(ui, status, messages);
        }
    })
}

/// What the last check for updates found.
fn update_found(ui: &mut egui::Ui, status: UpdateStatus, messages: &mut Vec<Message>) {
    match status {
        UpdateStatus::Checking => description(ui, "Checking for updates…"),
        UpdateStatus::Latest => description(ui, "You have the latest version."),
        UpdateStatus::Failed => description(ui, "Could not check for updates."),
        UpdateStatus::Available(version) => {
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("Ruston Mail {version} is available."));
                download_button(ui, messages);
            });
        }
    }
}

/// Opens a newer release's page, where it is downloaded from.
pub(super) fn download_button(ui: &mut egui::Ui, messages: &mut Vec<Message>) {
    if ui
        .add(theme::compact_button(ui, "Download"))
        .on_hover_text("Opens the release page in your browser.")
        .clicked()
    {
        messages.push(Message::OpenReleasePage);
    }
}

pub(super) fn preference_changes(current: &Settings, draft: &Settings) -> Vec<Message> {
    let mut messages = Vec::new();
    if draft.mark_read_on_open != current.mark_read_on_open {
        messages.push(Message::SetMarkReadOnOpen(draft.mark_read_on_open));
    }
    if draft.confirm_links != current.confirm_links {
        messages.push(Message::SetConfirmLinks(draft.confirm_links));
    }
    if draft.expand_all_messages != current.expand_all_messages {
        messages.push(Message::SetExpandAllMessages(draft.expand_all_messages));
    }
    if draft.show_quoted_text != current.show_quoted_text {
        messages.push(Message::SetShowQuotedText(draft.show_quoted_text));
    }
    if draft.show_hidden_text != current.show_hidden_text {
        messages.push(Message::SetShowHiddenText(draft.show_hidden_text));
    }
    if draft.sender_text_styles != current.sender_text_styles {
        messages.push(Message::SetSenderTextStyles(draft.sender_text_styles));
    }
    if draft.data_tables != current.data_tables {
        messages.push(Message::SetDataTables(draft.data_tables));
    }
    if draft.show_inline_images != current.show_inline_images {
        messages.push(Message::SetShowInlineImages(draft.show_inline_images));
    }
    if draft.compose_placement != current.compose_placement {
        messages.push(Message::SetComposePlacement(draft.compose_placement));
    }
    if draft.compose_format != current.compose_format {
        messages.push(Message::SetComposeFormat(draft.compose_format));
    }
    if draft.check_recipient_protection != current.check_recipient_protection {
        messages.push(Message::SetCheckRecipientProtection(
            draft.check_recipient_protection,
        ));
    }
    if draft.accept_international_domains != current.accept_international_domains {
        messages.push(Message::SetAcceptInternationalDomains(
            draft.accept_international_domains,
        ));
    }
    if draft.search_device_offline != current.search_device_offline {
        messages.push(Message::SetSearchDeviceOffline(draft.search_device_offline));
    }
    if draft.start != current.start {
        messages.push(Message::SetStartFolder(draft.start));
    }
    if (draft.zoom - current.zoom).abs() > f32::EPSILON {
        messages.push(Message::SetZoom(draft.zoom));
    }
    if (draft.text_scale - current.text_scale).abs() > f32::EPSILON {
        messages.push(Message::SetTextScale(draft.text_scale));
    }
    let scheme_changed = draft.appearance != current.appearance;
    if scheme_changed || (current.follow_system_appearance && !draft.follow_system_appearance) {
        messages.push(Message::SetAppearance(draft.appearance));
    }
    // Choosing a scheme stops following the system, so following comes after.
    if draft.follow_system_appearance && (scheme_changed || !current.follow_system_appearance) {
        messages.push(Message::SetFollowSystemAppearance(true));
    }
    if draft.system_font != current.system_font {
        messages.push(Message::SetSystemFont(draft.system_font));
    }
    if draft.show_unread_badge != current.show_unread_badge {
        messages.push(Message::SetShowUnreadBadge(draft.show_unread_badge));
    }
    if draft.desktop_notifications != current.desktop_notifications {
        messages.push(Message::SetDesktopNotifications(
            draft.desktop_notifications,
        ));
    }
    if draft.notice_seconds != current.notice_seconds {
        messages.push(Message::SetNoticeSeconds(draft.notice_seconds));
    }
    if draft.check_for_updates != current.check_for_updates {
        messages.push(Message::SetCheckForUpdates(draft.check_for_updates));
    }
    messages
}

fn start_label(start: StartFolder) -> &'static str {
    match start {
        StartFolder::LastRead => "Where I left off",
        StartFolder::Always(folder) => folder.name(),
    }
}

fn percent_label(share: f32) -> String {
    format!("{:.0}%", share * 100.0)
}

fn seconds_label(seconds: u64) -> String {
    format!("{seconds} seconds")
}

/// Fixed usable zoom levels; loaded settings clamp other values.
const ZOOM_STEPS: [f32; 6] = [0.9, 1.0, 1.15, 1.3, 1.5, 1.75];

/// A step of the interface zoom from the keyboard or the View menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ZoomStep {
    In,
    Out,
    Reset,
}

/// The zoom a step leads to from `current`: the next level Settings offers
/// up or down, staying put at either end, or the default.
pub(super) fn zoom_step(current: f32, step: ZoomStep) -> f32 {
    const NEAR: f32 = 0.001;
    let found = match step {
        ZoomStep::In => ZOOM_STEPS.into_iter().find(|level| *level > current + NEAR),
        ZoomStep::Out => ZOOM_STEPS
            .into_iter()
            .rev()
            .find(|level| *level < current - NEAR),
        ZoomStep::Reset => Some(Settings::default().zoom),
    };
    found.unwrap_or(current)
}

/// What the mailbox already answers to. Listed here because a shortcut no one
/// can find is a shortcut no one uses; the keys themselves live in `ui::mod`.
const SHORTCUTS: [(&str, &str); 12] = [
    ("j  /  Down", "Open the next conversation"),
    ("k  /  Up", "Open the previous one"),
    (
        "Enter",
        "Open the selected conversation, or retry a failed one. In the search field, search all mail",
    ),
    (
        "Esc",
        "Close a menu, or back out one layer: link prompt, settings, search, reader",
    ),
    (COMMAND_R, "Refresh the folder"),
    (COMMAND_F, "Jump to the search field"),
    (COMMAND_COMMA, "Open or close Settings"),
    (COMMAND_N, "Write a new message"),
    (COMMAND_ENTER, "Send the message being written"),
    (COMMAND_TRASH, "Move selected conversation to Trash"),
    (FOLD_SIDEBAR, "Fold or unfold the sidebar"),
    (COMMAND_ZOOM, "Zoom in, zoom out, or back to 100%"),
];

#[cfg(target_os = "macos")]
const COMMAND_R: &str = "Cmd + R";
#[cfg(target_os = "macos")]
const COMMAND_F: &str = "Cmd + F";
#[cfg(target_os = "macos")]
const COMMAND_COMMA: &str = "Cmd + ,";
#[cfg(target_os = "macos")]
const COMMAND_N: &str = "Cmd + N";
#[cfg(target_os = "macos")]
const COMMAND_ENTER: &str = "Cmd + Enter";
#[cfg(target_os = "macos")]
const COMMAND_TRASH: &str = "Cmd + Backspace";
#[cfg(target_os = "macos")]
const COMMAND_ZOOM: &str = "Cmd + Plus / Minus / 0";
#[cfg(target_os = "macos")]
const FOLD_SIDEBAR: &str = "Ctrl + Cmd + S";

#[cfg(not(target_os = "macos"))]
const COMMAND_R: &str = "Ctrl + R";
#[cfg(not(target_os = "macos"))]
const COMMAND_F: &str = "Ctrl + F";
#[cfg(not(target_os = "macos"))]
const COMMAND_COMMA: &str = "Ctrl + ,";
#[cfg(not(target_os = "macos"))]
const COMMAND_N: &str = "Ctrl + N";
#[cfg(not(target_os = "macos"))]
const COMMAND_ENTER: &str = "Ctrl + Enter";
#[cfg(not(target_os = "macos"))]
const COMMAND_TRASH: &str = "Delete / Ctrl + Backspace";
#[cfg(not(target_os = "macos"))]
const COMMAND_ZOOM: &str = "Ctrl + Plus / Minus / 0";
#[cfg(not(target_os = "macos"))]
const FOLD_SIDEBAR: &str = "F9";

/// One key and what it does, with the keys in a column of their own so the
/// descriptions line up however wide the window is.
fn shortcut(ui: &mut egui::Ui, keys: &str, what: &str, keys_width: f32) {
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(keys_width, 0.0),
            Layout::right_to_left(Align::TOP),
            |ui| {
                ui.add(egui::Label::new(key_text(ui, keys)));
            },
        );
        ui.vertical(|ui| description(ui, what));
    });
}

fn key_text(ui: &egui::Ui, keys: &str) -> egui::RichText {
    egui::RichText::new(keys)
        .monospace()
        .size(theme::text_size(ui, theme::CAPTION_SIZE))
}

/// The width of the keys column: its widest key, or more. A key wider than
/// its column would reach past the section's edge and pull the sections
/// under it to the left.
fn keys_width(ui: &egui::Ui) -> f32 {
    SHORTCUTS
        .iter()
        .map(|(keys, _)| {
            egui::WidgetText::from(key_text(ui, keys))
                .into_galley(
                    ui,
                    Some(egui::TextWrapMode::Extend),
                    f32::INFINITY,
                    egui::TextStyle::Body,
                )
                .size()
                .x
        })
        .fold(KEYS_MIN_WIDTH, f32::max)
}

const KEYS_MIN_WIDTH: f32 = 120.0;

fn description(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).color(theme::colors(ui).muted));
}

fn section(ui: &mut egui::Ui, title: &str, content: impl FnOnce(&mut egui::Ui)) -> egui::Response {
    theme::card(ui)
        .show(ui, |ui| {
            // Cards fill the page instead of sizing to their longest line.
            ui.set_width(ui.available_width());
            ui.heading(
                egui::RichText::new(title)
                    .size(theme::text_size(ui, theme::SECTION_TITLE_SIZE))
                    .strong(),
            );
            ui.add_space(4.0);
            content(ui);
        })
        .response
}

#[cfg(test)]
mod tests;
