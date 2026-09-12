use chrono::{Local, TimeZone};
use iced::font::{self, Weight};
use iced::widget::text::{Span, Wrapping};
use iced::widget::{
    Column, button, column, container, rich_text, row, rule, scrollable, span, text,
};
use iced::{Element, Fill, Font, Padding};

use super::APP_FONT;
use super::mailbox::{DETAIL_SIZE, PANE_PADDING, SPACING};
use crate::app::{ConversationReader, Mailbox, Message, PendingLink, ReaderState};
use crate::mail::{
    BlockKind, ConversationDetail, ConversationSummary, MailAddress, MailMessage, MessageBody,
    RichBlock, RichBody, RichSpan,
};

const SUBJECT_SIZE: f32 = 22.0;
const MESSAGE_SPACING: f32 = 12.0;
const TOGGLE_WIDTH: f32 = 16.0;
/// Words of the body shown in a collapsed message's header.
const PREVIEW_WORDS: usize = 40;
const BLOCK_SPACING: f32 = 10.0;
const LIST_MARKER_WIDTH: f32 = 28.0;
const LIST_INDENT: f32 = 18.0;
const QUOTE_INDENT: f32 = 12.0;
/// Deeper quotes share this level's box so indentation stays readable.
const MAX_QUOTE_DEPTH: u8 = 4;

pub(super) fn view<'a>(
    mailbox: &'a Mailbox,
    actions_available: bool,
    pending_link: Option<&'a PendingLink>,
) -> Element<'a, Message> {
    let content = match mailbox.reader_state() {
        ReaderState::Empty => placeholder("Select a conversation to read it."),
        ReaderState::Loading { .. } => placeholder("Loading conversation…"),
        ReaderState::Failed { error, .. } => load_error(*error),
        ReaderState::Loaded(reader) => conversation(
            reader,
            reader.detail(),
            mailbox.selected_summary().filter(|_| actions_available),
            !mailbox.is_busy() && !mailbox.action_pending(),
            mailbox.action_error(),
        ),
    };
    let content = match pending_link {
        Some(link) => column![link_prompt(link), content].height(Fill).into(),
        None => content,
    };

    container(content).width(Fill).height(Fill).into()
}

/// Asks before opening a link, showing where it really goes.
fn link_prompt(link: &PendingLink) -> Element<'_, Message> {
    container(
        column![
            text(format!("Open a link to {}?", link.target)).wrapping(Wrapping::WordOrGlyph),
            text(link.url.as_str())
                .size(DETAIL_SIZE)
                .style(text::secondary)
                .wrapping(Wrapping::WordOrGlyph),
            row![
                button(text("Open")).on_press(Message::OpenLink),
                button(text("Copy link"))
                    .style(button::secondary)
                    .on_press(Message::CopyLink),
                button(text("Cancel"))
                    .style(button::text)
                    .on_press(Message::DismissLink),
            ]
            .spacing(SPACING),
        ]
        .spacing(6),
    )
    .width(Fill)
    .padding(PANE_PADDING)
    .style(container::bordered_box)
    .into()
}

fn load_error(error: crate::mail::MailboxError) -> Element<'static, Message> {
    container(
        column![
            text(error.conversation_message()),
            button(text("Retry")).on_press(Message::RetryConversation),
        ]
        .spacing(SPACING),
    )
    .center(Fill)
    .padding(PANE_PADDING)
    .into()
}

fn placeholder(message: &str) -> Element<'_, Message> {
    container(text(message).style(text::secondary))
        .center(Fill)
        .padding(PANE_PADDING)
        .into()
}

fn conversation<'a>(
    reader: &'a ConversationReader,
    detail: &'a ConversationDetail,
    summary: Option<&'a ConversationSummary>,
    actions_enabled: bool,
    action_error: Option<crate::mail::MailboxError>,
) -> Element<'a, Message> {
    let mut header = column![
        text(detail.subject.as_deref().unwrap_or("(No subject)"))
            .size(SUBJECT_SIZE)
            .wrapping(Wrapping::WordOrGlyph),
        text(message_count_label(detail.messages.len()))
            .size(DETAIL_SIZE)
            .style(text::secondary),
    ]
    .spacing(4);
    if let Some(summary) = summary {
        header = header.push(action_toolbar(summary, actions_enabled));
        if let Some(error) = action_error {
            header = header.push(
                text(error.action_message())
                    .size(DETAIL_SIZE)
                    .style(text::danger),
            );
        }
    }

    let mut messages = Column::new().spacing(MESSAGE_SPACING);
    if detail.messages.is_empty() {
        messages = messages.push(text("This conversation has no messages.").style(text::secondary));
    }
    for message in &detail.messages {
        messages = messages.push(message_card(message, reader.is_expanded(&message.id)));
    }

    scrollable(
        column![header, messages]
            .spacing(16)
            .padding(PANE_PADDING)
            .width(Fill),
    )
    .spacing(SPACING)
    .height(Fill)
    .into()
}

fn action_toolbar(summary: &ConversationSummary, enabled: bool) -> Element<'_, Message> {
    let (read_label, read_message) = if summary.unread {
        ("Mark read", Message::MarkSelectedRead)
    } else {
        ("Mark unread", Message::MarkSelectedUnread)
    };
    let (star_label, star_message) = if summary.starred {
        ("Unstar", Message::UnstarSelected)
    } else {
        ("Star", Message::StarSelected)
    };

    row![
        action_button("Archive", Message::ArchiveSelected, enabled),
        action_button("Spam", Message::MoveSelectedToSpam, enabled),
        action_button("Trash", Message::MoveSelectedToTrash, enabled),
        action_button(read_label, read_message, enabled),
        action_button(star_label, star_message, enabled),
    ]
    .spacing(4)
    .wrap()
    .vertical_spacing(4)
    .into()
}

fn action_button(label: &str, message: Message, enabled: bool) -> Element<'_, Message> {
    button(text(label).size(DETAIL_SIZE))
        .padding([4, 8])
        .style(button::secondary)
        .on_press_maybe(enabled.then_some(message))
        .into()
}

fn message_card(message: &MailMessage, expanded: bool) -> Element<'_, Message> {
    let sender = message.sender.display_name().unwrap_or("(Unknown sender)");
    let time = message
        .time
        .map(|time| format_message_time(time, &Local))
        .unwrap_or_default();

    let mut identity = column![text(sender).wrapping(Wrapping::WordOrGlyph)].spacing(2);
    if expanded {
        if message.sender.name.is_some() && !message.sender.address.is_empty() {
            identity = identity.push(detail_line(message.sender.address.clone()));
        }
        identity = identity.push(detail_line(format!(
            "To: {}",
            recipients_label(&message.recipients)
        )));
    } else {
        identity = identity.push(
            container(
                text(preview(&message.body))
                    .size(DETAIL_SIZE)
                    .style(text::secondary)
                    .wrapping(Wrapping::None),
            )
            .width(Fill)
            .clip(true),
        );
    }

    // The arrow shows the state in addition to the body being visible.
    let header = button(
        row![
            text(if expanded { "▾" } else { "▸" }).width(TOGGLE_WIDTH),
            identity.width(Fill),
            text(time).size(DETAIL_SIZE).style(text::secondary),
        ]
        .spacing(SPACING),
    )
    .width(Fill)
    .padding(10)
    .style(button::text)
    .on_press(Message::ToggleMessageExpanded(message.id.clone()));

    let mut card = column![header];
    if expanded {
        card = card.push(rule::horizontal(1)).push(
            container(message_body(&message.body))
                .width(Fill)
                .padding(MESSAGE_SPACING),
        );
    }

    container(card)
        .width(Fill)
        .style(container::bordered_box)
        .into()
}

fn message_body(body: &MessageBody) -> Element<'_, Message> {
    match body {
        MessageBody::PlainText(content) => text(content.as_str())
            .wrapping(Wrapping::WordOrGlyph)
            .into(),
        MessageBody::Rich(rich) => rich_body(rich),
    }
}

fn rich_body(rich: &RichBody) -> Element<'_, Message> {
    if rich.blocks.is_empty() {
        return text("This message has no text.")
            .style(text::secondary)
            .into();
    }

    blocks(&rich.blocks, 0)
}

/// Lays out blocks at one quote depth; each run of deeper blocks shares one
/// quote box, so a quoted reply reads as a single unit.
fn blocks(blocks: &[RichBlock], depth: u8) -> Element<'_, Message> {
    let mut column = Column::new().spacing(BLOCK_SPACING).width(Fill);
    let mut rest = blocks;

    while let Some(first) = rest.first() {
        if first.quote_depth > depth && depth < MAX_QUOTE_DEPTH {
            let run = rest
                .iter()
                .take_while(|block| block.quote_depth > depth)
                .count();
            column = column.push(quote_box(self::blocks(&rest[..run], depth + 1)));
            rest = &rest[run..];
        } else {
            column = column.push(block(first));
            rest = &rest[1..];
        }
    }

    column.into()
}

fn quote_box(content: Element<'_, Message>) -> Element<'_, Message> {
    container(content)
        .width(Fill)
        .padding(Padding {
            left: QUOTE_INDENT,
            ..Padding::new(SPACING)
        })
        .style(container::bordered_box)
        .into()
}

fn block(block: &RichBlock) -> Element<'_, Message> {
    match &block.kind {
        BlockKind::Paragraph(spans) => rich_line(spans, None),
        BlockKind::Heading { level, spans } => rich_line(spans, Some(*level)),
        BlockKind::ListItem {
            marker,
            depth,
            spans,
        } => row![
            text(marker.as_str()).width(LIST_MARKER_WIDTH),
            rich_line(spans, None),
        ]
        .spacing(4)
        .padding(Padding {
            left: LIST_INDENT * f32::from(depth.saturating_sub(1)),
            ..Padding::ZERO
        })
        .into(),
        BlockKind::Preformatted(content) => container(
            text(content.as_str())
                .font(Font::MONOSPACE)
                .wrapping(Wrapping::WordOrGlyph),
        )
        .width(Fill)
        .padding(SPACING)
        .style(container::rounded_box)
        .into(),
        BlockKind::Image { description } => text(format!("[Image: {description}]"))
            .size(DETAIL_SIZE)
            .style(text::secondary)
            .wrapping(Wrapping::WordOrGlyph)
            .into(),
        BlockKind::Rule => rule::horizontal(1).into(),
    }
}

fn rich_line(spans: &[RichSpan], heading: Option<u8>) -> Element<'_, Message> {
    let mut line = rich_text(
        spans
            .iter()
            .map(|piece| rich_span(piece, heading.is_some()))
            .collect::<Vec<_>>(),
    )
    .wrapping(Wrapping::WordOrGlyph)
    .width(Fill)
    .on_link_click(Message::LinkClicked);
    if let Some(level) = heading {
        line = line.size(heading_size(level));
    }

    line.into()
}

fn rich_span(piece: &RichSpan, heading: bool) -> Span<'_, String, Font> {
    let base = if piece.code {
        Font::MONOSPACE
    } else {
        APP_FONT
    };
    let font = Font {
        weight: if piece.strong || heading {
            Weight::Bold
        } else {
            base.weight
        },
        style: if piece.emphasis {
            font::Style::Italic
        } else {
            base.style
        },
        ..base
    };

    span(piece.text.as_str())
        .font(font)
        .underline(piece.link.is_some())
        .strikethrough(piece.struck)
        .link_maybe(piece.link.clone())
}

fn heading_size(level: u8) -> f32 {
    match level {
        1 => 24.0,
        2 => 20.0,
        _ => 17.0,
    }
}

fn detail_line<'a>(content: String) -> Element<'a, Message> {
    text(content)
        .size(DETAIL_SIZE)
        .style(text::secondary)
        .wrapping(Wrapping::WordOrGlyph)
        .into()
}

fn message_count_label(count: usize) -> String {
    match count {
        0 => "No messages".to_owned(),
        1 => "1 message".to_owned(),
        count => format!("{count} messages"),
    }
}

/// The first words of the body on one line.
fn preview(body: &MessageBody) -> String {
    let words: Vec<&str> = match body {
        MessageBody::PlainText(content) => content.split_whitespace().take(PREVIEW_WORDS).collect(),
        MessageBody::Rich(rich) => rich
            .text_fragments()
            .flat_map(str::split_whitespace)
            .take(PREVIEW_WORDS)
            .collect(),
    };

    words.join(" ")
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

/// A complete local date and time, e.g. `Sep 11, 2026 at 09:42`.
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
    fn message_counts_are_pluralized() {
        assert_eq!(message_count_label(0), "No messages");
        assert_eq!(message_count_label(1), "1 message");
        assert_eq!(message_count_label(7), "7 messages");
    }
}
