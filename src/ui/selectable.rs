//! Read-only selectable text.
//!
//! Iced 0.14 cannot select the text of a `text` widget, so a body the user
//! wants to copy is shown in a `text_editor` instead: it selects with the
//! mouse, by word, by line and with Ctrl/Cmd+A, and copies with Ctrl/Cmd+C.
//! Edits never reach the buffer; the app layer drops them.
//!
//! Replace the body of this module once Iced can select plain text.

use iced::widget::text::Wrapping;
use iced::widget::{container, text_editor};
use iced::{Border, Color, Element, Fill, Theme};

use super::reader::{BODY_LINE_HEIGHT, BODY_SIZE};
use crate::app::Message;

pub(super) fn read_only<'a>(
    message_id: &str,
    content: &'a text_editor::Content,
) -> Element<'a, Message> {
    // Several bodies can be on screen at once, so each interaction carries the
    // message it belongs to.
    let message_id = message_id.to_owned();

    // The editor draws its own padding and frame; the container around it
    // already spaces the body, so both are removed here.
    container(
        text_editor(content)
            .padding(0)
            .size(BODY_SIZE)
            .line_height(BODY_LINE_HEIGHT)
            .wrapping(Wrapping::WordOrGlyph)
            .style(plain)
            .on_action(move |action| Message::SelectText(message_id.clone(), action)),
    )
    .width(Fill)
    .into()
}

/// Styles the editor as plain content rather than an input field.
fn plain(theme: &Theme, _status: text_editor::Status) -> text_editor::Style {
    let palette = theme.extended_palette();

    text_editor::Style {
        background: Color::TRANSPARENT.into(),
        border: Border::default(),
        placeholder: palette.background.strong.color,
        value: theme.palette().text,
        selection: palette.primary.weak.color,
    }
}
