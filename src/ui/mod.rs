use iced::widget::{column, container, row, text};
use iced::{Element, Fill, Never, Size};

use crate::app::App;
use crate::mail::ConnectionStatus;

const WINDOW_SIZE: Size = Size::new(1_100.0, 700.0);
const SIDEBAR_WIDTH: f32 = 240.0;
const SIDEBAR_PADDING: f32 = 28.0;
const CONTENT_PADDING: f32 = 48.0;
const CONTENT_SPACING: f32 = 20.0;

pub fn run(boot: impl Fn() -> App + 'static) -> iced::Result {
    iced::application(boot, (), view)
        .title("Ruston")
        .window_size(WINDOW_SIZE)
        .resizable(true)
        .run()
}

fn view(app: &App) -> Element<'_, Never> {
    let status = match app.connection_status() {
        ConnectionStatus::Connected => "Connected to Proton Mail",
        ConnectionStatus::Disconnected => "No Proton Mail account connected",
    };

    let sidebar = container(
        column![
            text("Ruston").size(32),
            text("Mail").size(18),
            text(status).size(14),
        ]
        .spacing(CONTENT_SPACING),
    )
    .width(SIDEBAR_WIDTH)
    .height(Fill)
    .padding(SIDEBAR_PADDING)
    .style(container::dark);

    let content = container(
        column![
            text("Welcome to Ruston").size(30),
            text("A native desktop client for Proton Mail."),
            container(
                column![
                    text("Early development").size(20),
                    text("Authentication and mailbox features are not available yet."),
                ]
                .spacing(12),
            )
            .padding(24)
            .style(container::rounded_box),
        ]
        .spacing(CONTENT_SPACING),
    )
    .width(Fill)
    .height(Fill)
    .padding(CONTENT_PADDING);

    row![sidebar, content].into()
}
