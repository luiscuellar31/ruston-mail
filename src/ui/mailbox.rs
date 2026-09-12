use chrono::{DateTime, Datelike, Local, TimeZone};
use iced::widget::text::Wrapping;
use iced::widget::{
    Column, button, column, container, pane_grid, row, scrollable, space, text, text_input,
};
use iced::{Center, Element, Fill, Theme};

use super::{DETAIL_SIZE, detail_text};
use crate::app::{
    App, CONVERSATION_LIST, DIVIDER_GRAB, DIVIDER_WIDTH, ListStatus, MIN_PANEL_WIDTH, Mailbox,
    Message, Panel, SEARCH_INPUT, UndoMove,
};
use crate::mail::{ConversationSummary, MailFolder};

pub(super) const PANE_PADDING: f32 = 16.0;
pub(super) const SPACING: f32 = 8.0;
const UNREAD_MARKER_WIDTH: f32 = 12.0;

pub(super) fn view<'a>(
    app: &'a App,
    mailbox: &'a Mailbox,
    email: Option<&'a str>,
    signing_out: bool,
) -> Element<'a, Message> {
    let panels = pane_grid(app.panels(), move |_pane, panel, _maximized| {
        let content = match panel {
            Panel::Sidebar => sidebar(app, mailbox, email, signing_out),
            Panel::Conversations => conversation_pane(mailbox),
            Panel::Reader => {
                super::reader::view(mailbox, app.mailbox_actions_available(), app.pending_link())
            }
        };
        pane_grid::Content::new(content).style(pane_background)
    })
    .spacing(DIVIDER_WIDTH)
    .min_size(MIN_PANEL_WIDTH)
    .on_resize(DIVIDER_GRAB, Message::PanelResized);

    // Panels paint the normal background, so the gaps between them show this
    // color as thin dividers, like the previous fixed rules.
    container(panels).style(divider_background).into()
}

fn pane_background(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(theme.palette().background.into()),
        ..container::Style::default()
    }
}

fn divider_background(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(theme.extended_palette().background.strong.color.into()),
        ..container::Style::default()
    }
}

fn sidebar<'a>(
    app: &'a App,
    mailbox: &'a Mailbox,
    email: Option<&'a str>,
    signing_out: bool,
) -> Element<'a, Message> {
    let folders = Column::with_children(
        MailFolder::ALL
            .into_iter()
            .map(|folder| folder_button(mailbox, folder)),
    )
    .spacing(2);

    let (account, logout_label) = if app.is_demo() {
        ("Demo mode · fictional mail", "Exit demo")
    } else {
        (email.unwrap_or("Proton Mail account"), "Sign out")
    };
    let logout = button(text(if signing_out {
        "Signing out…"
    } else {
        logout_label
    }))
    .width(Fill)
    .style(button::secondary)
    .on_press_maybe((!signing_out).then_some(Message::Logout));

    let settings = button(text("Settings"))
        .width(Fill)
        .style(button::text)
        .on_press(Message::ShowSettings(true));

    let mut footer = column![detail_text(account), settings, logout].spacing(SPACING);
    if let Some(error) = app.error_message() {
        footer = footer.push(text(error).size(DETAIL_SIZE).style(text::danger));
    }

    container(
        column![
            text("Ruston Mail").size(24),
            folders,
            space().height(Fill),
            footer
        ]
        .spacing(16),
    )
    .width(Fill)
    .height(Fill)
    .padding(PANE_PADDING)
    .into()
}

fn folder_button(mailbox: &Mailbox, folder: MailFolder) -> Element<'_, Message> {
    let selected = folder == mailbox.folder();
    let unread = mailbox
        .counts()
        .and_then(|counts| counts.unread(folder))
        .filter(|&unread| unread > 0);

    let mut label = row![text(folder.name()).width(Fill)].spacing(SPACING);
    if let Some(unread) = unread {
        label = label.push(text(unread.to_string()));
    }

    button(label)
        .width(Fill)
        .style(move |theme, status| {
            if selected {
                button::primary(theme, status)
            } else {
                button::text(theme, status)
            }
        })
        .on_press(Message::SelectFolder(folder))
        .into()
}

fn conversation_pane(mailbox: &Mailbox) -> Element<'_, Message> {
    let refresh_label = if matches!(mailbox.status(), ListStatus::Refreshing(_)) {
        "Refreshing…"
    } else {
        "Refresh"
    };
    let header = row![
        text(mailbox.folder().name()).size(22).width(Fill),
        button(text(refresh_label))
            .style(button::secondary)
            .on_press_maybe((!mailbox.is_busy()).then_some(Message::RefreshMailbox)),
    ]
    .spacing(SPACING)
    .align_y(Center);

    let input = text_input("Search mail…", mailbox.search_query())
        .id(SEARCH_INPUT)
        .on_input(Message::SearchChanged);
    let mut search = column![].spacing(4);
    if mailbox.search_query().is_empty() {
        search = search.push(input);
    } else {
        search = search
            .push(
                row![
                    input,
                    button(text("Clear"))
                        .style(button::secondary)
                        .on_press(Message::SearchChanged(String::new())),
                ]
                .spacing(SPACING),
            )
            // Search never reaches the server, so it says what it covers.
            .push(detail_text(search_scope_label(mailbox.loaded_count())));
    }

    let no_visible_conversations = mailbox.visible_conversations().next().is_none();

    let body = match (mailbox.status(), no_visible_conversations) {
        (ListStatus::Loading(_), _) => centered(text("Loading conversations…").into()),
        (ListStatus::Failed(error), true) => centered(
            column![
                text(error.message()),
                button(text("Try again")).on_press(Message::RefreshMailbox),
            ]
            .spacing(SPACING)
            .align_x(Center)
            .into(),
        ),
        // Search only covers what is loaded, so the way out of an empty
        // result is loading more. Saying so beats a dead end.
        (_, true) if mailbox.is_searching() => {
            let mut empty = column![
                text("No matches in the conversations loaded so far.")
                    .style(text::secondary)
                    .wrapping(Wrapping::WordOrGlyph)
            ]
            .spacing(SPACING)
            .align_x(Center);
            if let Some(more) = load_more(mailbox) {
                empty = empty.push(more);
            }
            centered(empty.into())
        }
        (_, true) => {
            centered(text(format!("No conversations in {}.", mailbox.folder().name())).into())
        }
        _ => conversation_list(mailbox),
    };

    let mut pane = column![header, search].spacing(12);
    if let Some(undo) = mailbox.undo() {
        pane = pane.push(undo_bar(undo));
    }

    container(pane.push(body).spacing(12))
        .width(Fill)
        .height(Fill)
        .padding(PANE_PADDING)
        .into()
}

fn conversation_list(mailbox: &Mailbox) -> Element<'_, Message> {
    let now = Local::now();
    let selected = mailbox.selected_conversation();
    let mut list = Column::with_children(mailbox.visible_conversations().map(|conversation| {
        conversation_row(
            conversation,
            selected == Some(conversation.id.as_str()),
            &now,
        )
    }))
    .spacing(2);

    if let Some(more) = load_more(mailbox) {
        list = list.push(container(more).center_x(Fill).padding(SPACING));
    }

    let mut content = column![].spacing(SPACING);
    if let ListStatus::Failed(error) = mailbox.status() {
        content = content.push(text(error.message()).style(text::danger));
    }
    // The gap keeps the scrollbar from covering row details.
    content
        .push(
            scrollable(list)
                .id(CONVERSATION_LIST)
                .height(Fill)
                .spacing(SPACING),
        )
        .into()
}

fn conversation_row<'a>(
    conversation: &'a ConversationSummary,
    selected: bool,
    now: &DateTime<Local>,
) -> Element<'a, Message> {
    let correspondents = conversation
        .correspondents
        .as_deref()
        .unwrap_or("(Unknown)");
    let subject = conversation.subject.as_deref().unwrap_or("(No subject)");
    let time = conversation
        .time
        .map(|time| format_time(time, now))
        .unwrap_or_default();

    // Unread uses a marker rather than bold: with the default sans-serif family,
    // bold text rendered in an unrelated fallback face on macOS, so weight is
    // not a reliable cue across platforms.
    let marker = container(
        text(if conversation.unread { "●" } else { "" })
            .size(DETAIL_SIZE)
            .style(move |theme| {
                if selected {
                    text::default(theme)
                } else {
                    text::primary(theme)
                }
            }),
    )
    .width(UNREAD_MARKER_WIDTH);

    let mut details = row![].spacing(6).align_y(Center);
    if conversation.message_count > 1 {
        details = details.push(text(conversation.message_count.to_string()).size(DETAIL_SIZE));
    }
    if conversation.has_attachments {
        details = details.push(text("📎").size(DETAIL_SIZE));
    }
    if conversation.starred {
        details = details.push(text("★").size(DETAIL_SIZE));
    }

    let lines = column![
        row![single_line(correspondents), text(time).size(DETAIL_SIZE)].spacing(SPACING),
        row![single_line(subject), details].spacing(SPACING),
    ]
    .spacing(4);

    button(row![marker, lines].spacing(4).align_y(Center))
        .width(Fill)
        .padding([8, 10])
        .style(move |theme, status| {
            if selected {
                button::primary(theme, status)
            } else {
                button::text(theme, status)
            }
        })
        .on_press(Message::SelectConversation(conversation.id.clone()))
        .into()
}

/// Offers to take back the last move, until the next action replaces it.
fn undo_bar(undo: &UndoMove) -> Element<'_, Message> {
    container(
        row![
            text(format!("Moved to {}.", undo.to.name()))
                .size(DETAIL_SIZE)
                .width(Fill),
            button(text("Undo").size(DETAIL_SIZE))
                .padding([2, 8])
                .style(button::secondary)
                .on_press(Message::UndoMove),
        ]
        .spacing(SPACING)
        .align_y(Center),
    )
    .padding([4, 8])
    .style(container::rounded_box)
    .into()
}

/// Says how far a search reaches, since it only reads loaded conversations.
fn search_scope_label(loaded: usize) -> String {
    match loaded {
        1 => "Searching the 1 conversation loaded so far.".to_owned(),
        loaded => format!("Searching the {loaded} conversations loaded so far."),
    }
}

/// The control that extends the loaded list, when there is more to load.
fn load_more(mailbox: &Mailbox) -> Option<Element<'_, Message>> {
    if matches!(mailbox.status(), ListStatus::LoadingMore(_)) {
        return Some(text("Loading more…").into());
    }

    mailbox.has_more().then(|| {
        button(text("Load more"))
            .style(button::secondary)
            .on_press_maybe((!mailbox.is_busy()).then_some(Message::LoadMoreConversations))
            .into()
    })
}

fn centered(content: Element<'_, Message>) -> Element<'_, Message> {
    container(content).center(Fill).into()
}

/// A clipped, non-wrapping line so long values never push other row content.
fn single_line(content: &str) -> Element<'_, Message> {
    container(text(content).wrapping(Wrapping::None))
        .width(Fill)
        .clip(true)
        .into()
}

/// Formats a Unix timestamp relative to `now`: time today, month and day this
/// year, full date otherwise.
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
    fn search_scope_says_how_far_it_reaches() {
        assert_eq!(
            search_scope_label(1),
            "Searching the 1 conversation loaded so far."
        );
        assert_eq!(
            search_scope_label(50),
            "Searching the 50 conversations loaded so far."
        );
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
}
