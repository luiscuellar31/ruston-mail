use chrono::{DateTime, Datelike, Local, TimeZone};
use iced::widget::text::Wrapping;
use iced::widget::{Column, button, column, container, row, rule, scrollable, space, text};
use iced::{Center, Element, Fill, FillPortion};

use crate::app::{App, ListStatus, Mailbox, Message};
use crate::mail::{ConversationSummary, MailFolder};

const SIDEBAR_WIDTH: f32 = 220.0;
pub(super) const PANE_PADDING: f32 = 16.0;
pub(super) const SPACING: f32 = 8.0;
pub(super) const DETAIL_SIZE: f32 = 12.0;
const UNREAD_MARKER_WIDTH: f32 = 12.0;

pub(super) fn view<'a>(
    app: &'a App,
    mailbox: &'a Mailbox,
    email: Option<&'a str>,
    signing_out: bool,
) -> Element<'a, Message> {
    row![
        sidebar(app, mailbox, email, signing_out),
        rule::vertical(1),
        conversation_pane(mailbox),
        rule::vertical(1),
        super::reader::view(mailbox),
    ]
    .height(Fill)
    .into()
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

    let mut footer = column![
        text(account).size(DETAIL_SIZE).style(text::secondary),
        logout
    ]
    .spacing(SPACING);
    if let Some(error) = app.error_message() {
        footer = footer.push(text(error).size(DETAIL_SIZE).style(text::danger));
    }

    container(
        column![
            text("Ruston").size(24),
            folders,
            space().height(Fill),
            footer
        ]
        .spacing(16),
    )
    .width(SIDEBAR_WIDTH)
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

    let body = match (mailbox.status(), mailbox.conversations().is_empty()) {
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
        (_, true) => {
            centered(text(format!("No conversations in {}.", mailbox.folder().name())).into())
        }
        _ => conversation_list(mailbox),
    };

    container(column![header, body].spacing(12))
        .width(FillPortion(3))
        .height(Fill)
        .padding(PANE_PADDING)
        .into()
}

fn conversation_list(mailbox: &Mailbox) -> Element<'_, Message> {
    let now = Local::now();
    let selected = mailbox.selected_conversation();
    let mut list = Column::with_children(mailbox.conversations().iter().map(|conversation| {
        conversation_row(
            conversation,
            selected == Some(conversation.id.as_str()),
            &now,
        )
    }))
    .spacing(2);

    if matches!(mailbox.status(), ListStatus::LoadingMore(_)) {
        list = list.push(
            container(text("Loading more…"))
                .center_x(Fill)
                .padding(SPACING),
        );
    } else if mailbox.has_more() {
        list = list.push(
            container(
                button(text("Load more"))
                    .style(button::secondary)
                    .on_press_maybe((!mailbox.is_busy()).then_some(Message::LoadMoreConversations)),
            )
            .center_x(Fill)
            .padding(SPACING),
        );
    }

    let mut content = column![].spacing(SPACING);
    if let ListStatus::Failed(error) = mailbox.status() {
        content = content.push(text(error.message()).style(text::danger));
    }
    // The gap keeps the scrollbar from covering row details.
    content
        .push(scrollable(list).height(Fill).spacing(SPACING))
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
