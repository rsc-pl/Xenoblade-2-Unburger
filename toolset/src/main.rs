mod backend;
mod config;
mod jobs;
mod gui;
mod translation;

fn main() -> iced::Result {
    gui::run()
}
