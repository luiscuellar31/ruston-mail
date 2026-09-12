//! The settings page.
//!
//! Only choices with behaviour behind them appear here. Sizes and the open
//! folder are remembered on their own, without anything to set.

use iced::widget::{button, checkbox, column, container, row, scrollable, text};
use iced::{Element, Fill};

use super::mailbox::{PANE_PADDING, SPACING};
use super::{DETAIL_SIZE, detail_text};
use crate::app::Message;
use crate::settings::Settings;

const PAGE_WIDTH: f32 = 620.0;

pub(super) fn view(settings: &Settings) -> Element<'_, Message> {
    let header = row![
        text("Settings").size(24).width(Fill),
        button(text("Done").size(DETAIL_SIZE))
            .padding([4, 8])
            .style(button::secondary)
            .on_press(Message::ShowSettings(false)),
    ]
    .spacing(SPACING);

    let reading = column![
        text("Reading").size(16),
        checkbox(settings.mark_read_on_open)
            .label("Mark mail as read when I open it")
            .on_toggle(Message::SetMarkReadOnOpen),
        detail_text("With this off, mail stays unread until you mark it yourself."),
    ]
    .spacing(6);

    let safety = column![
        text("Links and images").size(16),
        checkbox(settings.confirm_links)
            .label("Ask before opening a link")
            .on_toggle(Message::SetConfirmLinks),
        detail_text(
            "The prompt shows the real destination, which is how a link that \
             reads like your bank gives itself away."
        ),
        detail_text(
            "Images in mail are never downloaded. That cannot be turned off yet, \
             and it is what keeps a sender from learning you opened their mail."
        ),
    ]
    .spacing(6);

    let remembered = column![
        text("Remembered on their own").size(16),
        detail_text(
            "The window size, the width of the panes and the folder you were \
             last reading come back the way you left them."
        ),
    ]
    .spacing(6);

    scrollable(
        container(column![header, reading, safety, remembered].spacing(24))
            .max_width(PAGE_WIDTH)
            .padding(PANE_PADDING),
    )
    .height(Fill)
    .into()
}
