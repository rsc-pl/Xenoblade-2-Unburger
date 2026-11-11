use crate::{backend, config::Settings, jobs};
use iced::widget::{button, column, container, progress_bar, row, scrollable, text, text_editor, text_input};
use iced::{Background, Border, Color, Element, Length, Subscription, Task, Theme};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

pub fn run() -> iced::Result {
    iced::application(App::new, App::update, App::view)
        .title("Xenoblade BDAT Studio")
        .theme(Theme::Dark)
        .subscription(App::subscription)
        .window_size((1340.0, 900.0))
        .centered()
        .run()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen { Pack, Extract, Settings }

#[derive(Debug, Clone, Copy)]
enum PathField { PackSource, PackDestination, ExtractSource, ExtractDestination }

#[derive(Debug, Clone)]
enum Message {
    Screen(Screen),
    Platform(bool),
    Scope(jobs::Scope),
    Layout(String),
    Source(String),
    Destination(String),
    Tool(String),
    EditPath(PathField, String),
    BrowseSource,
    BrowseDestination,
    BrowseTool,
    WizardNext,
    WizardBack,
    Start,
    Stop,
    Tick,
    ClearLogs,
    CopyConsole,
    CopyErrors,
    OpenDestination,
    FollowLatest,
    ConsoleScrolled(bool),
    ConsoleAction(text_editor::Action),
    ErrorAction(text_editor::Action),
}

struct App {
    settings: Settings,
    screen: Screen,
    switch2: bool,
    scope: jobs::Scope,
    wizard_step: usize,
    receiver: Option<mpsc::Receiver<jobs::Event>>,
    cancellation: Option<Arc<AtomicBool>>,
    running: bool,
    completed: usize,
    succeeded: usize,
    failed: usize,
    total: usize,
    console: Vec<String>,
    errors: Vec<String>,
    console_content: text_editor::Content,
    error_content: text_editor::Content,
    status: String,
    active_destination: Option<PathBuf>,
    successful_destination: Option<PathBuf>,
    console_follow: bool,
}

impl App {
    fn new() -> Self {
        let mut settings = Settings::load();
        if settings.toolset_path.is_empty() {
            if let Some(tool) = backend::locate("") { settings.toolset_path = tool.to_string_lossy().into_owned(); }
        }
        Self {
            settings,
            screen: Screen::Pack,
            switch2: false,
            scope: jobs::Scope::Batch,
            wizard_step: 0,
            receiver: None,
            cancellation: None,
            running: false,
            completed: 0, succeeded: 0, failed: 0, total: 0,
            console: Vec::new(), errors: Vec::new(),
            console_content: text_editor::Content::new(),
            error_content: text_editor::Content::new(),
            status: "Ready".into(),
            active_destination: None,
            successful_destination: None,
            console_follow: true,
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        if self.running {
            iced::time::every(Duration::from_millis(80)).map(|_| Message::Tick)
        } else {
            Subscription::none()
        }
    }

    fn save(&mut self) {
        if let Err(e) = self.settings.save() {
            self.status = format!("Cannot save settings: {e}");
        }
    }

    fn kind(&self) -> jobs::Kind {
        if self.screen == Screen::Extract { jobs::Kind::Extract } else { jobs::Kind::Pack }
    }

    fn current_paths(&self) -> &crate::config::Paths { self.settings.current(self.switch2) }

    fn source(&self) -> &str {
        let p = self.current_paths();
        if self.kind() == jobs::Kind::Pack { &p.pack_source } else { &p.extract_source }
    }
    fn destination(&self) -> &str {
        let p = self.current_paths();
        if self.kind() == jobs::Kind::Pack { &p.pack_destination } else { &p.extract_destination }
    }
    fn source_mut(&mut self) -> &mut String {
        let p = self.settings.current_mut(self.switch2);
        if self.screen == Screen::Extract { &mut p.extract_source } else { &mut p.pack_source }
    }
    fn destination_mut(&mut self) -> &mut String {
        let p = self.settings.current_mut(self.switch2);
        if self.screen == Screen::Extract { &mut p.extract_destination } else { &mut p.pack_destination }
    }

    fn update(&mut self, msg: Message) -> Task<Message> {
        match msg {
            Message::Screen(screen) if !self.running => self.screen = screen,
            Message::Platform(switch2) if !self.running => self.switch2 = switch2,
            Message::Scope(scope) if !self.running => self.scope = scope,
            Message::Layout(layout) if !self.running => { self.settings.layout = layout; self.save(); }
            Message::Source(value) if !self.running => { *self.source_mut() = value; self.save(); }
            Message::Destination(value) if !self.running => { *self.destination_mut() = value; self.save(); }
            Message::Tool(value) if !self.running => { self.settings.toolset_path = value; self.save(); }
            Message::EditPath(field, value) if !self.running => {
                let paths = self.settings.current_mut(self.switch2);
                match field {
                    PathField::PackSource => paths.pack_source = value,
                    PathField::PackDestination => paths.pack_destination = value,
                    PathField::ExtractSource => paths.extract_source = value,
                    PathField::ExtractDestination => paths.extract_destination = value,
                }
                self.save();
            }
            Message::BrowseSource if !self.running => {
                let picked = match (self.kind(), self.scope) {
                    (jobs::Kind::Extract, jobs::Scope::Single) => rfd::FileDialog::new().add_filter("BDAT", &["bdat"]).pick_file(),
                    _ => rfd::FileDialog::new().pick_folder(),
                };
                if let Some(path) = picked { *self.source_mut() = path.to_string_lossy().into_owned(); self.save(); }
            }
            Message::BrowseDestination if !self.running => {
                let picked = if self.kind() == jobs::Kind::Pack && self.scope == jobs::Scope::Single {
                    rfd::FileDialog::new().add_filter("BDAT", &["bdat"]).save_file()
                } else { rfd::FileDialog::new().pick_folder() };
                if let Some(path) = picked { *self.destination_mut() = path.to_string_lossy().into_owned(); self.save(); }
            }
            Message::BrowseTool if !self.running => {
                if let Some(path) = rfd::FileDialog::new().pick_file() {
                    self.settings.toolset_path = path.to_string_lossy().into_owned(); self.save();
                }
            }
            Message::WizardNext if !self.running => {
                if self.wizard_step < 2 {
                    self.wizard_step += 1;
                } else if backend::locate(&self.settings.toolset_path).is_some() {
                    self.settings.wizard_completed = true;
                    self.save();
                    self.status = "Configuration completed".into();
                } else {
                    self.status = "Select a valid bdat-toolset executable before continuing".into();
                }
            }
            Message::WizardBack if !self.running => { self.wizard_step = self.wizard_step.saturating_sub(1); }
            Message::Start if !self.running => self.start(),
            Message::Stop if self.running => {
                if let Some(cancel) = &self.cancellation { cancel.store(true, Ordering::Relaxed); }
                self.status = "Stopping after the current file...".into();
            }
            Message::Tick => {
                let mut finished = false;
                let mut console_changed = false;
                let mut errors_changed = false;
                let mut new_console_lines = Vec::new();
                let mut new_error_lines = Vec::new();
                let mut console_trimmed = false;
                if let Some(rx) = &self.receiver {
                    for _ in 0..300 {
                        match rx.try_recv() {
                            Ok(jobs::Event::Line(line)) => {
                                self.console.push(line.clone());
                                new_console_lines.push(line);
                                console_changed = true;
                                if self.console.len() > 6000 { self.console.drain(..1000); console_trimmed = true; }
                            }
                            Ok(jobs::Event::Failure(line)) => {
                                self.errors.push(line.clone());
                                new_error_lines.push(line);
                                errors_changed = true;
                            }
                            Ok(jobs::Event::Fatal(line)) => {
                                self.status = format!("Operation stopped: {line}");
                                self.errors.push(line.clone());
                                new_error_lines.push(line);
                                errors_changed = true;
                                finished = true;
                                break;
                            }
                            Ok(jobs::Event::Progress { completed, succeeded, failed }) => {
                                self.completed = completed;
                                self.succeeded = succeeded;
                                self.failed = failed;
                            }
                            Ok(jobs::Event::Finished { completed, succeeded, failed, log_file, error_file }) => {
                                self.completed = completed;
                                self.succeeded = succeeded;
                                self.failed = failed;
                                self.status = format!("Finished: {succeeded} successful, {failed} failed. Full and error logs have been saved.");
                                let report_line = format!("REPORTS: full log saved ({}), errors saved ({})", std::path::Path::new(&log_file).file_name().and_then(|s| s.to_str()).unwrap_or("full.log"), std::path::Path::new(&error_file).file_name().and_then(|s| s.to_str()).unwrap_or("errors.log"));
                                self.console.push(report_line.clone());
                                new_console_lines.push(report_line);
                                console_changed = true;
                                self.successful_destination = if failed == 0 && succeeded > 0 && completed == self.total {
                                    self.active_destination.clone()
                                } else { None };
                                finished = true;
                                break;
                            }
                            Err(mpsc::TryRecvError::Disconnected) => {
                                self.status = "Worker stopped unexpectedly. Check logs and the Errors tab.".into();
                                finished = true; break;
                            }
                            Err(mpsc::TryRecvError::Empty) => break,
                        }
                    }
                }
                if console_changed {
                    if console_trimmed {
                        self.console_content = text_editor::Content::with_text(&self.console.join("\n"));
                    } else {
                        append_editor_lines(&mut self.console_content, &new_console_lines, self.console_follow);
                    }
                }
                if errors_changed {
                    append_editor_lines(&mut self.error_content, &new_error_lines, false);
                }
                if finished {
                    self.running = false;
                    self.receiver = None;
                    self.cancellation = None;
                    self.active_destination = None;
                }
                if console_changed && self.console_follow {
                    return iced::widget::operation::snap_to_end("console-output");
                }
            }
            Message::ClearLogs => {
                self.console.clear();
                self.console_content = text_editor::Content::new();
            }
            Message::ConsoleAction(action) => {
                if !matches!(action, text_editor::Action::Edit(_) | text_editor::Action::Scroll { .. }) {
                    self.console_content.perform(action);
                }
            }
            Message::ErrorAction(action) => {
                if !matches!(action, text_editor::Action::Edit(_) | text_editor::Action::Scroll { .. }) {
                    self.error_content.perform(action);
                }
            }
            Message::CopyConsole => return iced::clipboard::write(self.console.join("\n")),
            Message::CopyErrors => return iced::clipboard::write(self.errors.join("\n")),
            Message::ConsoleScrolled(at_end) => self.console_follow = at_end,
            Message::FollowLatest => {
                self.console_follow = true;
                return iced::widget::operation::snap_to_end("console-output");
            }
            Message::OpenDestination => {
                if let Some(path) = &self.successful_destination {
                    self.status = match open_output_directory(path) {
                        Ok(()) => "Opened output folder".into(),
                        Err(problem) => format!("Could not open output folder: {problem}"),
                    };
                }
            }
            _ => {}
        }
        Task::none()
    }

    fn start(&mut self) {
        let toolset = match backend::locate(&self.settings.toolset_path) {
            Some(path) => match path.canonicalize() {
                Ok(absolute) => absolute,
                Err(e) => { self.status = format!("Cannot resolve bdat-toolset path: {e}"); return; }
            },
            None => { self.status = "bdat-toolset not found; configure it in Settings".into(); return; }
        };
        let request = jobs::Request {
            kind: self.kind(),
            scope: self.scope,
            layout: jobs::Layout::from_label(&self.settings.layout),
            source: PathBuf::from(self.source()),
            destination: PathBuf::from(self.destination()),
            toolset,
        };
        let plan = match jobs::plan(&request) {
            Ok(plan) => plan,
            Err(e) => { self.status = format!("Preflight failed: {e}"); return; }
        };
        self.total = plan.items.len();
        self.active_destination = Some(request.destination.clone());
        self.successful_destination = None;
        self.console_follow = true;
        self.completed = 0;
        self.succeeded = 0;
        self.failed = 0;
        self.console.clear();
        self.errors.clear();
        self.console_content = text_editor::Content::new();
        self.error_content = text_editor::Content::new();
        self.status = format!("Processing {} operations...", self.total);
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        jobs::launch(request, plan, tx, Arc::clone(&cancel));
        self.receiver = Some(rx);
        self.cancellation = Some(cancel);
        self.running = true;
    }

    fn view(&self) -> Element<'_, Message> {
        if !self.settings.wizard_completed {
            return self.wizard();
        }

        let backend_ready = backend::locate(&self.settings.toolset_path).is_some();
        let header = container(
            row![
                container(text("◆").size(27).color(rgb(111, 215, 237)))
                    .padding([0, 6]),
                column![
                    text("BDAT STUDIO").size(26).color(rgb(240, 247, 255)),
                    text(format!("XENOBLADE   /   CONVERSION WORKSPACE   •   v{}", env!("CARGO_PKG_VERSION")))
                        .size(11).color(MUTED),
                ].spacing(2),
                container(text("")).width(Length::Fill),
                container(
                    row![
                        text("●").size(13).color(if backend_ready { GOOD } else { DANGER }),
                        text(if backend_ready { "Backend connected" } else { "Backend not found" })
                            .size(12).color(TEXT),
                    ].spacing(9).align_y(iced::Alignment::Center)
                ).padding([9, 14]).style(inset_style),
            ].spacing(12).align_y(iced::Alignment::Center)
        ).padding([15, 20]).width(Length::Fill).style(header_style);

        let tabs = row![
            nav_button("01   Pack", self.screen == Screen::Pack, Message::Screen(Screen::Pack)),
            nav_button("02   Extract", self.screen == Screen::Extract, Message::Screen(Screen::Extract)),
            nav_button("Settings", self.screen == Screen::Settings, Message::Screen(Screen::Settings)),
            container(text("")).width(Length::Fill),
            text("JSON  ↔  BDAT").size(12).color(MUTED),
        ].spacing(9).align_y(iced::Alignment::Center);

        let content = if self.screen == Screen::Settings {
            self.settings_view()
        } else {
            self.operation_view()
        };

        let footer = container(
            row![
                text("●").size(12).color(if self.failed > 0 { DANGER } else if self.running { ACCENT } else { GOOD }),
                text(&self.status).size(12).color(TEXT),
            ].spacing(9).align_y(iced::Alignment::Center)
        ).padding([10, 15]).width(Length::Fill).style(inset_style);

        container(
            column![header, tabs, content, footer]
                .spacing(12)
                .padding(17)
                .width(Length::Fill)
                .height(Length::Fill)
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .style(workspace_style)
        .into()
    }

    fn wizard(&self) -> Element<'_, Message> {
        let backend_ready = backend::locate(&self.settings.toolset_path).is_some();
        let intro = column![
            text("◆   BDAT STUDIO").size(29).color(TEXT),
            text("Set up the converter and folders before the first run.")
                .size(14).color(MUTED),
        ].spacing(9);

        let step_row = row![
            step_label("01  Welcome", self.wizard_step == 0),
            step_label("02  Backend", self.wizard_step == 1),
            step_label("03  Folders", self.wizard_step == 2),
        ].spacing(10);

        let body: Element<'_, Message> = match self.wizard_step {
            0 => column![
                text("One workspace. Two Switch platforms.").size(22).color(TEXT),
                text("Pack and extract BDAT with the external bdat-toolset executable.")
                    .size(14).color(MUTED),
                text("The application source stays inside toolset/. Preferences and logs are stored in your user directories.")
                    .size(14).color(MUTED),
            ].spacing(16).into(),
            1 => column![
                text("Locate bdat-toolset").size(22).color(TEXT),
                text("The executable is detected automatically near the app or in PATH. You can choose another file.")
                    .size(13).color(MUTED),
                row![
                    text_input("Path to bdat-toolset", &self.settings.toolset_path)
                        .on_input(Message::Tool).padding(12),
                    button("Browse…").on_press(Message::BrowseTool)
                        .padding([12, 18]).style(quiet_button_style),
                ].spacing(10),
                text(if backend_ready { "●  Backend found" } else { "●  Backend not found" })
                    .size(13).color(if backend_ready { GOOD } else { DANGER }),
            ].spacing(14).into(),
            _ => column![
                text("Default folders").size(22).color(TEXT),
                text("Source and destination folders are saved separately for each platform. Empty paths can be filled in later.")
                    .size(13).color(MUTED),
                self.platform_picker(),
                settings_path("PACK SOURCE", &self.current_paths().pack_source, PathField::PackSource),
                settings_path("PACK DESTINATION", &self.current_paths().pack_destination, PathField::PackDestination),
                settings_path("EXTRACT SOURCE", &self.current_paths().extract_source, PathField::ExtractSource),
                settings_path("EXTRACT DESTINATION", &self.current_paths().extract_destination, PathField::ExtractDestination),
            ].spacing(11).into(),
        };

        let controls = row![
            button("← Back").on_press(Message::WizardBack)
                .padding([12, 20]).style(quiet_button_style),
            container(text("")).width(Length::Fill),
            button(if self.wizard_step == 2 { "Finish setup  →" } else { "Continue  →" })
                .on_press(Message::WizardNext).padding([12, 22]).style(accent_button_style),
        ].spacing(12).align_y(iced::Alignment::Center);

        let panel = container(
            column![intro, step_row, body, controls, text(&self.status).size(12).color(MUTED)]
                .spacing(24)
        ).padding(36).width(Length::Fill).style(panel_style);
        container(panel)
            .padding(48).width(Length::Fill).height(Length::Fill)
            .style(workspace_style).into()
    }

    fn platform_picker(&self) -> Element<'_, Message> {
        row![
            nav_button("Switch 1", !self.switch2, Message::Platform(false)),
            nav_button("Switch 2", self.switch2, Message::Platform(true)),
        ].spacing(7).into()
    }

    fn path_editor<'a>(&'a self, label: &'static str, value: &'a str, source: bool) -> Element<'a, Message> {
        column![
            text(label).size(11).color(MUTED),
            row![
                text_input("Select a folder or file…", value)
                    .on_input(move |value| {
                        if source { Message::Source(value) } else { Message::Destination(value) }
                    })
                    .size(13)
                    .padding([11, 13]),
                button("Browse…")
                    .on_press(if source { Message::BrowseSource } else { Message::BrowseDestination })
                    .padding([11, 15])
                    .style(quiet_button_style),
            ].spacing(9),
        ].spacing(7).width(Length::Fill).into()
    }

    fn operation_view(&self) -> Element<'_, Message> {
        let packing = self.kind() == jobs::Kind::Pack;
        let platform = self.platform_picker();
        let scope = row![
            nav_button("Batch", self.scope == jobs::Scope::Batch, Message::Scope(jobs::Scope::Batch)),
            nav_button("Single", self.scope == jobs::Scope::Single, Message::Scope(jobs::Scope::Single)),
        ].spacing(7);
        let layouts = row![
            nav_button("Auto", self.settings.layout == "Auto", Message::Layout("Auto".into())),
            nav_button("Standard", self.settings.layout == "Standard", Message::Layout("Standard".into())),
            nav_button("Xenoblade 3", self.settings.layout == "Xenoblade 3", Message::Layout("Xenoblade 3".into())),
        ].spacing(7);

        let introduction = row![
            column![
                text(if packing { "Pack JSON → BDAT" } else { "Extract BDAT → JSON" })
                    .size(22).color(TEXT),
                text(if packing {
                    "Prepare BDAT files for your selected destination."
                } else {
                    "Convert BDAT files into editable JSON directories."
                }).size(12).color(MUTED),
            ].spacing(3),
            container(text("")).width(Length::Fill),
            container(text(if self.switch2 { "SWITCH 2 PROFILE" } else { "SWITCH 1 PROFILE" })
                .size(11).color(ACCENT))
                .padding([10, 13]).style(inset_style),
        ].spacing(12).align_y(iced::Alignment::Center);

        let options = row![
            column![text("PLATFORM").size(10).color(MUTED), platform].spacing(7),
            column![text("MODE").size(10).color(MUTED), scope].spacing(7),
            column![text("DIRECTORY LAYOUT").size(10).color(MUTED), layouts].spacing(7),
        ].spacing(24).align_y(iced::Alignment::Center);

        let paths = row![
            container(self.path_editor("SOURCE  /  INPUT", self.source(), true))
                .width(Length::FillPortion(1)),
            container(self.path_editor("DESTINATION  /  OUTPUT", self.destination(), false))
                .width(Length::FillPortion(1)),
        ].spacing(16).width(Length::Fill);

        let configuration = container(
            column![
                row![
                    text("CONVERSION OPTIONS").size(11).color(ACCENT),
                    container(text("")).width(Length::Fill),
                    text(if self.settings.layout == "Xenoblade 3" {
                        "XC3  •  evt layout applied on pack"
                    } else { "External bdat-toolset" }).size(11).color(MUTED),
                ].align_y(iced::Alignment::Center),
                options,
                paths,
            ].spacing(13)
        ).padding(17).width(Length::Fill).style(panel_style);

        let action = if self.running {
            button("Stop after current file")
                .style(warning_button_style)
                .on_press(Message::Stop)
                .padding([14, 20])
        } else {
            button(if packing { "▶   START PACKING" } else { "▶   START EXTRACTION" })
                .style(accent_button_style)
                .on_press(Message::Start)
                .padding([14, 20])
        };

        let fraction = if self.total == 0 { 0.0 } else {
            100.0 * (self.completed as f32) / (self.total as f32)
        };
        let open_output: Element<'_, Message> = if self.successful_destination.is_some() {
            button("Open output folder")
                .on_press(Message::OpenDestination)
                .padding([14, 20]).style(quiet_button_style).into()
        } else {
            container(text("")).into()
        };

        let summary = container(
            column![
                row![
                    action,
                    open_output,
                    container(text("")).width(Length::Fill),
                    metric("COMPLETED", format!("{} / {}", self.completed, self.total), ACCENT),
                    metric("SUCCESS", self.succeeded.to_string(), GOOD),
                    metric("ERRORS", self.failed.to_string(), if self.failed > 0 { DANGER } else { MUTED }),
                ].spacing(12).align_y(iced::Alignment::Center),
                row![
                    text(if self.running { "CONVERSION IN PROGRESS" } else { "BATCH PROGRESS" })
                        .size(10).color(MUTED),
                    container(text("")).width(Length::Fill),
                    text(format!("{fraction:.0}%")).size(12).color(ACCENT),
                ].align_y(iced::Alignment::Center),
                progress_bar(0.0..=100.0, fraction),
            ].spacing(12)
        ).padding([14, 18]).width(Length::Fill).style(panel_style);

        let console_panel = container(
            column![
                row![
                    text("●").size(13).color(ACCENT),
                    text("LIVE CONSOLE").size(12).color(TEXT),
                    container(text("")).width(Length::Fill),
                    button(if self.console_follow { "●  Following" } else { "↓  Latest" })
                        .on_press(Message::FollowLatest)
                        .padding([8, 12]).style(quiet_button_style),
                    button("Copy all").on_press(Message::CopyConsole)
                        .padding([8, 12]).style(quiet_button_style),
                    button("Clear").on_press(Message::ClearLogs)
                        .padding([8, 12]).style(quiet_button_style),
                ].spacing(9).align_y(iced::Alignment::Center),
                scrollable(
                    text_editor(&self.console_content)
                        .placeholder("Backend output will appear here…")
                        .on_action(Message::ConsoleAction)
                        .size(12).padding(12)
                        .style(log_editor_style)
                        .height(Length::Shrink)
                )
                .id("console-output")
                .direction(scrollable::Direction::Vertical(slim_scrollbar()))
                .on_scroll(|view| Message::ConsoleScrolled(
                    view.content_bounds().height <= view.bounds().height + 1.0
                        || view.relative_offset().y >= 0.98
                ))
                .height(Length::Fill),
            ].spacing(11)
        ).padding(14).width(Length::FillPortion(3)).height(Length::Fill).style(panel_style);

        let error_panel = container(
            column![
                row![
                    text("●").size(13).color(if self.errors.is_empty() { GOOD } else { DANGER }),
                    text(format!("ERRORS ({})", self.errors.len())).size(12).color(TEXT),
                    container(text("")).width(Length::Fill),
                    button("Copy all").on_press(Message::CopyErrors)
                        .padding([8, 12]).style(quiet_button_style),
                ].spacing(7).align_y(iced::Alignment::Center),
                scrollable(
                    text_editor(&self.error_content)
                        .placeholder("No errors recorded.")
                        .on_action(Message::ErrorAction)
                        .size(12).padding(12)
                        .style(log_editor_style)
                        .height(Length::Shrink)
                )
                .id("errors-output")
                .direction(scrollable::Direction::Vertical(slim_scrollbar()))
                .height(Length::Fill),
            ].spacing(11)
        ).padding(14).width(Length::FillPortion(1)).height(Length::Fill).style(panel_style);

        let logs = row![console_panel, error_panel]
            .spacing(12).height(Length::Fill).width(Length::Fill);

        column![introduction, configuration, summary, logs]
            .spacing(12)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn settings_view(&self) -> Element<'_, Message> {
        let backend_ready = backend::locate(&self.settings.toolset_path).is_some();
        let intro = column![
            text("Preferences").size(23).color(TEXT),
            text("Backend executable and default folders for each Switch profile.")
                .size(12).color(MUTED),
        ].spacing(4);

        let backend_panel = container(
            column![
                row![
                    text("BACKEND").size(12).color(ACCENT),
                    container(text("")).width(Length::Fill),
                    text(if backend_ready { "●  CONNECTED" } else { "●  NOT FOUND" })
                        .size(11).color(if backend_ready { GOOD } else { DANGER }),
                ].align_y(iced::Alignment::Center),
                text("bdat-toolset executable").size(13).color(TEXT),
                row![
                    text_input("Path to bdat-toolset", &self.settings.toolset_path)
                        .on_input(Message::Tool).size(13).padding(12),
                    button("Browse…").on_press(Message::BrowseTool)
                        .padding([12, 18]).style(quiet_button_style),
                ].spacing(10),
            ].spacing(12)
        ).padding(19).width(Length::Fill).style(panel_style);

        let platform_panel = container(
            column![
                text("PLATFORM DEFAULTS").size(12).color(ACCENT),
                row![
                    self.platform_picker(),
                    container(text("")).width(Length::Fill),
                    text("Settings save automatically").size(12).color(MUTED),
                ].align_y(iced::Alignment::Center),
                row![
                    container(settings_path("PACK SOURCE", &self.current_paths().pack_source, PathField::PackSource))
                        .width(Length::FillPortion(1)),
                    container(settings_path("PACK DESTINATION", &self.current_paths().pack_destination, PathField::PackDestination))
                        .width(Length::FillPortion(1)),
                ].spacing(18),
                row![
                    container(settings_path("EXTRACT SOURCE", &self.current_paths().extract_source, PathField::ExtractSource))
                        .width(Length::FillPortion(1)),
                    container(settings_path("EXTRACT DESTINATION", &self.current_paths().extract_destination, PathField::ExtractDestination))
                        .width(Length::FillPortion(1)),
                ].spacing(18),
            ].spacing(17)
        ).padding(19).width(Length::Fill).style(panel_style);

        column![intro, backend_panel, platform_panel]
            .spacing(15).height(Length::Fill).width(Length::Fill)
            .into()
    }
}

fn slim_scrollbar() -> scrollable::Scrollbar {
    scrollable::Scrollbar::new().width(8).scroller_width(5).margin(2).spacing(4)
}

// Incremental updates avoid rebuilding thousands of displayed log lines on every tick.
fn append_editor_lines(content: &mut text_editor::Content, lines: &[String], follow: bool) {
    if lines.is_empty() { return; }
    let previous_cursor = content.cursor();
    let existing = !content.is_empty();
    let mut update = lines.join("\n");
    if existing { update.insert(0, '\n'); }
    content.perform(text_editor::Action::Move(text_editor::Motion::DocumentEnd));
    content.perform(text_editor::Action::Edit(text_editor::Edit::Paste(Arc::new(update))));
    if !follow { content.move_to(previous_cursor); }
}

fn open_output_directory(output: &Path) -> Result<(), String> {
    let directory = if output.is_dir() {
        output
    } else {
        output.parent().ok_or("Output has no parent directory")?
    };
    if !directory.is_dir() { return Err("Output directory no longer exists".into()); }
    let mut command = if cfg!(target_os = "windows") {
        let mut command = Command::new("explorer.exe");
        command.arg(directory);
        command
    } else if cfg!(target_os = "macos") {
        let mut command = Command::new("open");
        command.arg(directory);
        command
    } else {
        let mut command = Command::new("xdg-open");
        command.arg(directory);
        command
    };
    command.spawn().map_err(|e| format!("Could not launch file manager: {e}"))?;
    Ok(())
}

const TEXT: Color = Color::from_rgb8(231, 240, 252);
const MUTED: Color = Color::from_rgb8(145, 163, 187);
const ACCENT: Color = Color::from_rgb8(91, 205, 229);
const GOOD: Color = Color::from_rgb8(104, 217, 171);
const DANGER: Color = Color::from_rgb8(240, 124, 139);

fn rgb(r: u8, g: u8, b: u8) -> Color { Color::from_rgb8(r, g, b) }

fn workspace_style(_: &Theme) -> container::Style {
    container::Style::default().background(rgb(13, 18, 29))
}

fn header_style(_: &Theme) -> container::Style {
    container::Style::default()
        .background(rgb(21, 31, 45))
        .border(Border { width: 1.0, radius: 16.0.into(), color: rgb(44, 60, 81) })
}

fn panel_style(_: &Theme) -> container::Style {
    container::Style::default()
        .background(rgb(24, 32, 46))
        .border(Border { width: 1.0, radius: 13.0.into(), color: rgb(44, 58, 77) })
}

fn inset_style(_: &Theme) -> container::Style {
    container::Style::default()
        .background(rgb(18, 26, 39))
        .border(Border { width: 1.0, radius: 9.0.into(), color: rgb(42, 57, 76) })
}

fn nav_style(theme: &Theme, status: button::Status, selected: bool) -> button::Style {
    let bg = if selected { rgb(35, 87, 115) } else { rgb(31, 41, 58) };
    let hovered = if selected { rgb(45, 106, 140) } else { rgb(44, 58, 79) };
    let color = if matches!(status, button::Status::Hovered | button::Status::Pressed) { hovered } else { bg };
    let mut style = button::secondary(theme, status);
    style.background = Some(Background::Color(color));
    style.text_color = if selected { rgb(230, 252, 255) } else { MUTED };
    style.border = Border {
        radius: 9.0.into(), width: 1.0,
        color: if selected { rgb(70, 147, 180) } else { rgb(47, 61, 80) },
    };
    style
}

fn selected_button_style(theme: &Theme, status: button::Status) -> button::Style {
    nav_style(theme, status, true)
}

fn quiet_button_style(theme: &Theme, status: button::Status) -> button::Style {
    nav_style(theme, status, false)
}

fn accent_button_style(theme: &Theme, status: button::Status) -> button::Style {
    let mut style = button::primary(theme, status);
    style.background = Some(Background::Color(if matches!(status, button::Status::Hovered) {
        rgb(120, 224, 239)
    } else { rgb(87, 195, 221) }));
    style.text_color = rgb(11, 28, 42);
    style.border = Border { radius: 9.0.into(), width: 0.0, color: ACCENT };
    style
}

fn warning_button_style(theme: &Theme, status: button::Status) -> button::Style {
    let mut style = button::danger(theme, status);
    style.border = Border { radius: 9.0.into(), width: 1.0, color: DANGER };
    style
}

fn nav_button<'a>(label: &'a str, selected: bool, message: Message) -> Element<'a, Message> {
    let style: fn(&Theme, button::Status) -> button::Style =
        if selected { selected_button_style } else { quiet_button_style };
    button(text(label).size(13))
        .padding([10, 16])
        .style(style)
        .on_press(message)
        .into()
}

fn step_label<'a>(label: &'a str, current: bool) -> Element<'a, Message> {
    let style: fn(&Theme) -> container::Style =
        if current { header_style } else { inset_style };
    container(text(label).size(13).color(if current { ACCENT } else { MUTED }))
        .padding([10, 15])
        .style(style)
        .into()
}

fn settings_path<'a>(label: &'static str, value: &'a str, field: PathField) -> Element<'a, Message> {
    column![
        text(label).size(11).color(MUTED),
        text_input("Folder", value).on_input(move |value| Message::EditPath(field, value))
            .size(13).padding(12),
    ].spacing(7).width(Length::Fill).into()
}

fn metric<'a>(label: &'static str, value: String, accent: Color) -> Element<'a, Message> {
    container(
        column![
            text(label).size(10).color(MUTED),
            text(value).size(18).color(accent),
        ].spacing(4)
    ).padding([7, 16]).style(inset_style).into()
}

fn log_editor_style(theme: &Theme, status: text_editor::Status) -> text_editor::Style {
    let mut style = text_editor::default(theme, status);
    style.background = Background::Color(rgb(14, 21, 32));
    style.border = Border { radius: 9.0.into(), width: 1.0, color: rgb(38, 53, 72) };
    style.placeholder = MUTED;
    style.value = TEXT;
    style.selection = rgb(36, 93, 119);
    style
}
