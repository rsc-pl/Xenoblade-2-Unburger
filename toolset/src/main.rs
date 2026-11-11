mod backend;
mod config;
mod jobs;
mod gui;

fn main() -> iced::Result {
    gui::run()
}
