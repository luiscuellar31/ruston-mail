mod app;
mod mail;
mod ui;

use app::App;
use mail::ProtonMailService;

fn main() -> iced::Result {
    ui::run(|| App::new(ProtonMailService::default()))
}
