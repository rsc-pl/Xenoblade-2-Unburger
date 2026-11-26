use crate::{backend, config::{Game, Settings}, jobs, translation};
use iced::widget::{button, checkbox, column, container, progress_bar, row, scrollable, text, text_editor, text_input};
use iced::{Background, Border, Color, Element, Length, Subscription, Task, Theme};
use std::path::{Path, PathBuf};
use std::collections::{BTreeMap, HashSet};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

pub fn run() -> iced::Result {
    iced::application(App::new, App::update, App::view)
        .title("UNBURGER BDAT STUDIO")
        .theme(Theme::Dark)
        .subscription(App::subscription)
        .window_size((1340.0, 900.0))
        .centered()
        .run()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen { Pack, Extract, Translation, Settings }

#[derive(Debug, Clone, Copy)]
enum PathField { PackSource, PackDestination, ExtractSource, ExtractDestination }

#[derive(Debug, Clone)]
enum Message {
    Screen(Screen),
    Game(Game),
    Platform(bool),
    Scope(jobs::Scope),
    XcxCommonParent(bool, bool),
    Source(String),
    Destination(String),
    Tool(String),
    EditPath(PathField, String),
    BrowsePath(PathField),
    ImportLegacy(bool),
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
    TranslationRefresh,
    TranslationReference(String),
    TranslationBrowse,
    TranslationFile(PathBuf),
    TranslationRow(usize),
    TranslationEdit(text_editor::Action),
    TranslationSave,
    TranslationDiscard,
    TranslationFilter(String),
    TranslationFileFilter(String),
    TranslationFileScroll(scrollable::Viewport),
    TranslationRowScroll(scrollable::Viewport),
    TranslationWarnings(bool),
    TranslationRules,
    TranslationRuleAdd,
    TranslationRuleRemove(usize),
    TranslationRuleChange(usize, u8, String),
    TranslationRuleReset,
    TranslationAudit,
    TranslationAuditCancel,
    TranslationStatus(translation::BrowserFilter),
    TranslationFolder(PathBuf),
    TranslationTagPreview(bool),
    TranslationException,
    TranslationPaste,
    TranslationPasteEdit(text_editor::Action),
    TranslationPastePreview,
    TranslationPasteApply,
    TranslationVersion(String),
    TranslationVersionPreview,
    TranslationVersionOpen,
}

#[derive(Debug, Clone, Copy)]
struct ListViewport { offset: f32, height: f32 }
impl Default for ListViewport {
    fn default() -> Self { Self { offset: 0.0, height: 2400.0 } }
}
impl ListViewport {
    fn update(&mut self, view: scrollable::Viewport) { self.offset = view.absolute_offset().y; self.height = view.bounds().height; }
    fn window(self, count: usize, row_height: f32) -> std::ops::Range<usize> { translation::visible_window(count, self.offset, self.height, row_height) }
}

#[derive(Default)]
struct TranslationState {
    files: Vec<PathBuf>,
    scan: Option<mpsc::Receiver<Result<Vec<PathBuf>, String>>>,
    cancel: Option<Arc<AtomicBool>>,
    doc: Option<translation::Document>,
    selected: Option<usize>,
    editor: text_editor::Content,
    file_filter: String,
    row_filter: String,
    file_viewport: ListViewport,
    row_viewport: ListViewport,
    warnings: bool,
    rules_open: bool,
    audits: BTreeMap<PathBuf, translation::FileAudit>,
    audit_rx: Option<mpsc::Receiver<(PathBuf, translation::FileAudit)>>,
    audit_cancel: Option<Arc<AtomicBool>>,
    audit_total: usize,
    browser_filter: translation::BrowserFilter,
    expanded: HashSet<PathBuf>,
    tag_preview: bool,
    paste_open: bool,
    paste_content: text_editor::Content,
    paste_preview: Option<Vec<translation::PasteChange>>,
    version: String,
}

impl TranslationState {
    fn dirty(&self) -> bool { self.doc.as_ref().is_some_and(|d| d.dirty()) }
    fn clear(&mut self) {
        if let Some(cancel) = &self.cancel { cancel.store(true, Ordering::Relaxed); }
        self.cancel_audit();
        *self = Self::default();
    }
    fn cancel_audit(&mut self) {
        if let Some(cancel) = self.audit_cancel.take() { cancel.store(true, Ordering::Relaxed); }
        self.audit_rx = None;
    }
    fn invalidate_audit(&mut self) {
        self.cancel_audit();
        self.audits.clear();
        self.audit_total = 0;
        self.browser_filter = translation::BrowserFilter::All;
    }
    fn close_paste(&mut self) {
        self.paste_open = false;
        self.paste_preview = None;
        self.paste_content = text_editor::Content::new();
    }
    fn select(&mut self, index: usize) {
        if let Some(record) = self.doc.as_ref().and_then(|d| d.records.get(index)) {
            self.editor = text_editor::Content::with_text(&record.edited);
            self.selected = Some(index);
        }
    }
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
    translation: TranslationState,
}

impl App {
    fn new() -> Self {
        let mut settings = Settings::load();
        if settings.toolset_path.is_empty() {
            if let Some(tool) = backend::locate("") { settings.toolset_path = tool.to_string_lossy().into_owned(); }
        }
        let switch2 = settings.last_switch2();
        Self {
            settings,
            screen: Screen::Pack,
            switch2,
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
            translation: TranslationState::default(),
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        if self.running || self.translation.scan.is_some() || self.translation.audit_rx.is_some() {
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

    fn reset_operation(&mut self) {
        self.scope = jobs::Scope::Batch;
        self.completed = 0;
        self.succeeded = 0;
        self.failed = 0;
        self.total = 0;
        self.console.clear();
        self.errors.clear();
        self.console_content = text_editor::Content::new();
        self.error_content = text_editor::Content::new();
        self.console_follow = true;
        self.active_destination = None;
        self.successful_destination = None;
        self.status = "Ready".into();
    }

    fn edit_path(&mut self, field: PathField, value: String) {
        let paths = self.settings.current_mut(self.switch2);
        match field {
            PathField::PackSource => paths.pack_source = value,
            PathField::PackDestination => paths.pack_destination = value,
            PathField::ExtractSource => paths.extract_source = value,
            PathField::ExtractDestination => paths.extract_destination = value,
        }
        self.save();
    }

    fn update(&mut self, msg: Message) -> Task<Message> {
        if self.translation.dirty() && matches!(&msg, Message::Screen(_) | Message::Game(_) | Message::Platform(_) | Message::TranslationRefresh | Message::TranslationFile(_) | Message::TranslationReference(_) | Message::TranslationBrowse | Message::TranslationVersionOpen | Message::Start) {
            self.status = "Unsaved translation changes. Save or discard them first.".into();
            return Task::none();
        }
        match msg {
            Message::Screen(screen) if !self.running => {
                if self.screen != screen {
                    self.successful_destination = None;
                    self.screen = screen;
                    if screen == Screen::Translation { self.refresh_translation(); }
                    else { self.translation.clear(); }
                }
            }
            Message::Game(game) if !self.running && self.settings.active_game != game => {
                self.settings.active_game = game;
                self.switch2 = self.settings.last_switch2();
                self.reset_operation();
                if self.screen == Screen::Translation { self.refresh_translation(); } else { self.translation.clear(); }
                self.save();
            }
            Message::Platform(switch2) if !self.running && self.switch2 != switch2 => {
                self.switch2 = switch2;
                self.settings.set_platform(switch2);
                self.reset_operation();
                if self.screen == Screen::Translation { self.refresh_translation(); } else { self.translation.clear(); }
                self.save();
            }
            Message::Scope(scope) if !self.running => self.scope = scope,
            Message::XcxCommonParent(switch2, enabled) if !self.running && self.settings.active_game == Game::XenobladeX && self.switch2 == switch2 => {
                self.settings.set_xcx_common_parent(switch2, enabled);
                self.successful_destination = None;
                self.save();
            }
            Message::Source(value) if !self.running => { *self.source_mut() = value; self.save(); }
            Message::Destination(value) if !self.running => { *self.destination_mut() = value; self.save(); }
            Message::Tool(value) if !self.running => { self.settings.toolset_path = value; self.save(); }
            Message::EditPath(field, value) if !self.running => {
                self.edit_path(field, value);
            }
            Message::BrowsePath(field) if !self.running => {
                if let Some(path) = rfd::FileDialog::new().pick_folder() {
                    self.edit_path(field, path.to_string_lossy().into_owned());
                }
            }
            Message::ImportLegacy(switch2) if !self.running => {
                match self.settings.import_legacy(switch2) {
                    Ok(()) => {
                        self.switch2 = switch2;
                        self.status = "Previous paths imported".into();
                        self.save();
                    }
                    Err(problem) => self.status = problem.into(),
                }
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
                let mut audit_finished = false;
                if let Some(rx) = &self.translation.audit_rx {
                    for _ in 0..200 {
                        match rx.try_recv() {
                            Ok((path, audit)) => { self.translation.audits.insert(path, audit); }
                            Err(mpsc::TryRecvError::Empty) => break,
                            Err(mpsc::TryRecvError::Disconnected) => { audit_finished = true; break; }
                        }
                    }
                }
                if audit_finished {
                    self.translation.cancel_audit();
                    self.status = format!("Validation {}: {}/{} files", if self.translation.audits.len() == self.translation.audit_total { "complete" } else { "incomplete" }, self.translation.audits.len(), self.translation.audit_total);
                }
                if let Some(rx) = &self.translation.scan {
                    let result = match rx.try_recv() {
                        Ok(result) => Some(result),
                        Err(mpsc::TryRecvError::Disconnected) => Some(Err("Translation scan stopped".into())),
                        Err(mpsc::TryRecvError::Empty) => None,
                    };
                    if let Some(result) = result {
                        self.translation.scan = None;
                        self.translation.cancel = None;
                        match result {
                            Ok(files) => {
                                self.status = format!("{} JSON files indexed", files.len());
                                self.translation.files = files;
                            }
                            Err(e) => self.status = e,
                        }
                    }
                }
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
            Message::TranslationRefresh => self.refresh_translation(),
            Message::TranslationReference(value) => {
                self.translation.invalidate_audit();
                self.translation.close_paste();
                self.settings.translation_mut(self.switch2).japanese_reference = value;
                self.translation.doc = None;
                self.translation.selected = None;
                self.save();
            }
            Message::TranslationBrowse => {
                if let Some(path) = rfd::FileDialog::new().pick_folder() {
                    self.translation.invalidate_audit();
                    self.translation.close_paste();
                    self.settings.translation_mut(self.switch2).japanese_reference = path.to_string_lossy().into_owned();
                    self.translation.doc = None;
                    self.translation.selected = None;
                    self.save();
                }
            }
            Message::TranslationFile(relative) => {
                self.translation.close_paste();
                self.translation.version.clear();
                self.translation.rules_open = false;
                let source = Path::new(&self.current_paths().pack_source);
                let jp = &self.settings.translation(self.switch2).japanese_reference;
                match translation::Document::open(source, &relative, jp, self.settings.active_game) {
                    Ok(doc) => {
                        self.status = format!("{} editable text records · {}", doc.records.len(), doc.reference_status);
                        let prefs = self.settings.translation(self.switch2);
                        let audit = translation::audit_document(&doc, &self.current_paths().pack_source, &prefs.rules(self.settings.active_game), &prefs.exceptions);
                        self.translation.audits.insert(relative.clone(), audit);
                        self.translation.doc = Some(doc);
                        self.translation.selected = None;
                        self.translation.row_viewport.offset = 0.0;
                        self.translation.row_filter.clear();
                        self.translation.warnings = false;
                        self.translation.select(0);
                    }
                    Err(e) => { self.translation.doc = None; self.translation.selected = None; self.status = e; }
                }
                return reset_list_scroll("translation-records");
            }
            Message::TranslationRow(index) => self.translation.select(index),
            Message::TranslationEdit(action) => {
                let editing = matches!(&action, text_editor::Action::Edit(_));
                self.translation.editor.perform(action);
                if editing {
                    self.translation.paste_preview = None;
                    if let (Some(i), Some(doc)) = (self.translation.selected, self.translation.doc.as_mut()) {
                        if let Some(record) = doc.records.get_mut(i) { record.edited = self.translation.editor.text(); }
                    }
                }
            }
            Message::TranslationSave => {
                let mut saved = false;
                if let Some(doc) = &mut self.translation.doc {
                    self.status = match doc.save() { Ok(n) => { saved = true; format!("Saved {n} edited records") }, Err(e) => e };
                }
                if saved { self.refresh_selected_audit(); }
                self.translation.close_paste();
            }
            Message::TranslationDiscard => {
                self.translation.close_paste();
                if let Some(doc) = &mut self.translation.doc {
                    for record in &mut doc.records { record.edited.clone_from(&record.saved); }
                }
                if let Some(index) = self.translation.selected { self.translation.select(index); }
                self.status = "Unsaved edits discarded".into();
            }
            Message::TranslationFileFilter(value) => { self.translation.file_filter = value; self.translation.file_viewport.offset = 0.0; return reset_list_scroll("translation-files"); }
            Message::TranslationFilter(value) => { self.translation.row_filter = value; self.translation.row_viewport.offset = 0.0; return reset_list_scroll("translation-records"); }
            Message::TranslationFileScroll(view) => self.translation.file_viewport.update(view),
            Message::TranslationRowScroll(view) => self.translation.row_viewport.update(view),
            Message::TranslationWarnings(value) => { self.translation.warnings = value; self.translation.row_viewport.offset = 0.0; return reset_list_scroll("translation-records"); }
            Message::TranslationRules => {
                self.translation.rules_open = !self.translation.rules_open;
                self.translation.file_viewport = ListViewport::default();
                self.translation.row_viewport = ListViewport::default();
            }
            Message::TranslationRuleAdd => {
                self.translation.invalidate_audit();
                let game = self.settings.active_game;
                let prefs = self.settings.translation_mut(self.switch2);
                let mut rules = prefs.rules(game);
                rules.push(translation::Rule::default());
                prefs.rules = Some(rules);
                self.save();
            }
            Message::TranslationRuleRemove(index) => {
                self.translation.invalidate_audit();
                let game = self.settings.active_game;
                let prefs = self.settings.translation_mut(self.switch2);
                let mut rules = prefs.rules(game);
                if index < rules.len() { rules.remove(index); }
                prefs.rules = Some(rules);
                self.save();
            }
            Message::TranslationRuleChange(index, field, value) => {
                self.translation.invalidate_audit();
                let game = self.settings.active_game;
                let prefs = self.settings.translation_mut(self.switch2);
                let mut rules = prefs.rules(game);
                if let Some(rule) = rules.get_mut(index) {
                    if field == 0 { rule.patterns = value; }
                    else {
                        let limit = if value.is_empty() { None } else if let Ok(n) = value.parse::<usize>() { Some(n) } else { return Task::none(); };
                        if field == 1 { rule.max_chars = limit; } else { rule.max_lines = limit; }
                    }
                    prefs.rules = Some(rules);
                    self.save();
                }
            }
            Message::TranslationRuleReset => { self.translation.invalidate_audit(); self.settings.translation_mut(self.switch2).rules = None; self.save(); }
            Message::TranslationAudit => self.start_translation_audit(),
            Message::TranslationAuditCancel => { self.translation.cancel_audit(); self.status = "Validation stopped; partial results retained".into(); }
            Message::TranslationStatus(value) => { self.translation.browser_filter = value; self.translation.file_viewport.offset = 0.0; return reset_list_scroll("translation-files"); }
            Message::TranslationFolder(value) => { if !self.translation.expanded.remove(&value) { self.translation.expanded.insert(value); } }
            Message::TranslationTagPreview(value) => self.translation.tag_preview = value,
            Message::TranslationException => {
                let root = self.current_paths().pack_source.clone();
                let rules = self.settings.translation(self.switch2).rules(self.settings.active_game);
                if let Some(doc) = &self.translation.doc {
                    if let Some(record) = self.translation.selected.and_then(|i| doc.records.get(i)) {
                        let relative = doc.relative.clone();
                        let key = record.key.clone();
                        if let Some(rule) = translation::matching_rule(&rules, &relative) {
                            let prefs = self.settings.translation_mut(self.switch2);
                            let exists = translation::ignored(&prefs.exceptions, &root, &relative, record, Some(rule));
                            let warning = translation::validate(&record.edited, Some(rule));
                            prefs.exceptions.retain(|e| !e.same_record(&root, &relative, &key));
                            if !exists && (warning.too_long || warning.too_many) && record.saved == record.edited {
                                prefs.exceptions.push(translation::WarningException::new(&root, &relative, record, rule));
                            }
                            self.refresh_selected_audit();
                            self.save();
                        }
                    }
                }
            }
            Message::TranslationPaste => {
                if self.translation.paste_open { self.translation.close_paste(); }
                else if self.translation.doc.is_some() { self.translation.paste_open = true; }
                self.translation.row_viewport.offset = 0.0;
                return reset_list_scroll("translation-records");
            }
            Message::TranslationPasteEdit(action) => { self.translation.paste_content.perform(action); self.translation.paste_preview = None; }
            Message::TranslationPastePreview => {
                self.translation.paste_preview = None;
                if let Some(doc) = &self.translation.doc {
                    match translation::preview_paste(doc, &self.translation.paste_content.text()) {
                        Ok(changes) => { self.status = format!("Preview: {} changes; nothing written", changes.len()); self.translation.paste_preview = Some(changes); }
                        Err(e) => self.status = e,
                    }
                }
            }
            Message::TranslationPasteApply => {
                if let (Some(doc), Some(changes)) = (&mut self.translation.doc, self.translation.paste_preview.take()) {
                    self.status = match translation::stage_changes(doc, &changes) {
                        Ok(()) => format!("{} edits staged; use Save file to write", changes.len()), Err(e) => e,
                    };
                }
                self.translation.close_paste();
                if let Some(i) = self.translation.selected { self.translation.select(i); }
                self.translation.row_viewport.offset = 0.0;
                return reset_list_scroll("translation-records");
            }
            Message::TranslationVersion(value) => { self.translation.version = value; self.translation.paste_preview = None; }
            Message::TranslationVersionOpen => {
                let candidates: Vec<_> = self.translation.files.iter().filter(|p| translation::version_path(self.settings.active_game, p)).cloned().collect();
                if candidates.len() == 1 { return self.update(Message::TranslationFile(candidates[0].clone())); }
                self.status = format!("Expected one known version file, found {}. No changes made.", candidates.len());
            }
            Message::TranslationVersionPreview => {
                self.translation.paste_preview = None;
                if let Some(doc) = &self.translation.doc {
                    match translation::preview_version(doc, self.settings.active_game, &self.translation.version) {
                        Ok(changes) => { self.translation.paste_open = true; self.translation.row_viewport.offset = 0.0; self.translation.paste_preview = Some(changes); self.status = "Version preview; nothing written".into(); }
                        Err(e) => self.status = e,
                    }
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
            layout: jobs::Layout::for_game(self.settings.active_game, self.settings.xcx_common_parent(self.switch2)),
            source: PathBuf::from(self.source()),
            destination: PathBuf::from(self.destination()),
            toolset,
        };
        let plan = match jobs::plan(&request) {
            Ok(plan) => plan,
            Err(e) => { self.status = format!("Preflight failed: {e}"); return; }
        };
        self.total = plan.items.len();
        self.active_destination = Some(if request.kind == jobs::Kind::Pack
            && request.layout == jobs::Layout::XenobladeXCommonParent
            && plan.items.len() == 1
            && plan.items[0].output_name.eq_ignore_ascii_case("common.bdat") {
            plan.items[0].destination.clone()
        } else { request.destination.clone() });
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
        if let Some((switch2, paths)) = self.settings.pending_legacy() {
            return self.migration_view(switch2, paths);
        }
        if !self.settings.wizard_completed {
            return self.wizard();
        }

        let backend_ready = backend::locate(&self.settings.toolset_path).is_some();
        let header = container(
            row![
                container(text("◆").size(27).color(rgb(111, 215, 237)))
                    .padding([0, 6]),
                column![
                    text("UNBURGER BDAT STUDIO").size(26).color(rgb(240, 247, 255)),
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
            nav_button("03   Translation", self.screen == Screen::Translation, Message::Screen(Screen::Translation)),
            nav_button("Settings", self.screen == Screen::Settings, Message::Screen(Screen::Settings)),
            container(text("")).width(Length::Fill),
            text("JSON  ↔  BDAT").size(12).color(MUTED),
        ].spacing(9).align_y(iced::Alignment::Center);

        let content = if self.screen == Screen::Settings {
            self.settings_view()
        } else if self.screen == Screen::Translation {
            self.translation_view()
        } else {
            self.operation_view()
        };

        let footer = container(
            row![
                text("●").size(12).color(if self.failed > 0 { DANGER } else if self.running { ACCENT } else { GOOD }),
                text(&self.status).size(12).color(TEXT),
            ].spacing(9).align_y(iced::Alignment::Center)
        ).padding([10, 15]).width(Length::Fill).style(inset_style);

        let mut workspace = column![header];
        if self.screen != Screen::Settings {
            workspace = workspace.push(self.game_picker());
        }
        container(
            workspace.push(tabs).push(content).push(footer)
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
            text("◆   UNBURGER BDAT STUDIO").size(29).color(TEXT),
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
                text("Four game profiles. Two Switch platforms.").size(22).color(TEXT),
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
                text("Paths are saved separately for each game and platform. Empty paths can be filled in later.")
                    .size(13).color(MUTED),
                self.game_picker(),
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
        container(scrollable(panel))
            .padding(48).width(Length::Fill).height(Length::Fill)
            .style(workspace_style).into()
    }

    fn migration_view<'a>(&'a self, switch2: bool, paths: &'a crate::config::Paths) -> Element<'a, Message> {
        let platform = if switch2 { "Switch 2" } else { "Switch 1" };
        let panel = container(column![
            text("Assign previous paths").size(26).color(TEXT),
            text(format!("Select the game that owns your existing {platform} paths. This imports settings only; no files are moved."))
                .size(14).color(MUTED),
            self.game_picker(),
            text("PACK SOURCE").size(11).color(ACCENT),
            text(&paths.pack_source).size(13).color(TEXT),
            text("PACK DESTINATION").size(11).color(ACCENT),
            text(&paths.pack_destination).size(13).color(TEXT),
            text("EXTRACT SOURCE").size(11).color(ACCENT),
            text(&paths.extract_source).size(13).color(TEXT),
            text("EXTRACT DESTINATION").size(11).color(ACCENT),
            text(&paths.extract_destination).size(13).color(TEXT),
            button(text(format!("Import into {} / {platform}", self.settings.active_game.label())))
                .on_press(Message::ImportLegacy(switch2)).padding([12, 22]).style(accent_button_style),
            text(&self.status).size(12).color(MUTED),
        ].spacing(12)).padding(24).width(Length::Fill).style(panel_style);
        container(scrollable(panel)).padding(30).width(Length::Fill).height(Length::Fill)
            .style(workspace_style).into()
    }

    fn game_picker(&self) -> Element<'_, Message> {
        let mut games = row![text("GAME").size(11).color(MUTED)]
            .spacing(9).align_y(iced::Alignment::Center);
        for game in Game::ALL {
            games = games.push(locked_nav_button(game.label(), self.settings.active_game == game,
                Message::Game(game), self.running));
        }
        container(games).padding([8, 12]).width(Length::Fill).style(inset_style).into()
    }

    fn platform_picker(&self) -> Element<'_, Message> {
        row![
            locked_nav_button("Switch 1", !self.switch2, Message::Platform(false), self.running),
            locked_nav_button("Switch 2", self.switch2, Message::Platform(true), self.running),
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

    fn refresh_translation(&mut self) {
        self.translation.clear();
        let root = PathBuf::from(&self.current_paths().pack_source);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (tx, rx) = mpsc::channel();
        self.translation.scan = Some(rx);
        self.translation.cancel = Some(cancel);
        self.status = "Indexing translation files…".into();
        std::thread::spawn(move || { let _ = tx.send(translation::scan(&root, &worker_cancel)); });
    }

    fn start_translation_audit(&mut self) {
        if self.translation.scan.is_some() || self.translation.dirty() {
            self.status = "Wait for indexing or save/discard edits before validating the project".into();
            return;
        }
        let filter = self.translation.browser_filter;
        self.translation.invalidate_audit();
        self.translation.browser_filter = filter;
        let root = self.current_paths().pack_source.clone();
        let files = self.translation.files.clone();
        let prefs = self.settings.translation(self.switch2).clone();
        let game = self.settings.active_game;
        let rules = prefs.rules(game);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (tx, rx) = mpsc::sync_channel(256);
        self.translation.audit_total = files.len();
        self.translation.audit_rx = Some(rx);
        self.translation.audit_cancel = Some(cancel);
        self.status = "Validating saved translation files…".into();
        std::thread::spawn(move || {
            for path in files {
                if worker_cancel.load(Ordering::Relaxed) { break; }
                let result = translation::audit_file(&root, &path, game, &prefs, &rules);
                if tx.send((path, result)).is_err() { break; }
            }
        });
    }

    fn refresh_selected_audit(&mut self) {
        // Preserve unrelated results, and stop the worker so an older queued
        // result cannot overwrite the newly saved file's status.
        self.translation.cancel_audit();
        let prefs = self.settings.translation(self.switch2);
        if let Some(doc) = &self.translation.doc {
            let path = doc.relative.clone();
            let audit = translation::audit_file(&self.current_paths().pack_source, &path, self.settings.active_game, prefs, &prefs.rules(self.settings.active_game));
            self.translation.audits.insert(path, audit);
        }
    }

    fn translation_view(&self) -> Element<'_, Message> {
        const FILE_ROW: f32 = 30.0;
        const RECORD_ROW: f32 = 34.0;
        let state = &self.translation;
        let prefs = self.settings.translation(self.switch2);
        let rules = prefs.rules(self.settings.active_game);
        let root = &self.current_paths().pack_source;
        let audit_warnings: usize = state.audits.values().map(|a| a.warnings).sum();
        let audit_ignored: usize = state.audits.values().map(|a| a.ignored).sum();
        let audit_errors = state.audits.values().filter(|a| a.error.is_some()).count();
        let header = container(row![self.platform_picker(),
                container(text("")).width(Length::Fill),
                button("Refresh").on_press(Message::TranslationRefresh).style(quiet_button_style),
                button(if state.audit_rx.is_some() { "Stop validation" } else { "Validate project" })
                    .on_press_maybe(if state.audit_rx.is_some() { Some(Message::TranslationAuditCancel) } else if state.scan.is_none() && !state.files.is_empty() && !state.dirty() { Some(Message::TranslationAudit) } else { None }).style(quiet_button_style),
                button("Mod version").on_press_maybe((!state.dirty() && state.scan.is_none()).then_some(Message::TranslationVersionOpen)).style(quiet_button_style),
                button(if state.rules_open { "Back to editor" } else { "⚙ Translation settings" }).on_press(Message::TranslationRules).style(quiet_button_style),
            ].spacing(7).align_y(iced::Alignment::Center)).padding(6).width(Length::Fill).style(panel_style);

        if state.rules_open {
            let mut list = column![text("Translation settings · current game and platform").size(20).color(TEXT),
                text(format!("Pack Source: {root}")).size(12).color(MUTED),
                text("Japanese reference (optional)").size(12).color(ACCENT),
                row![text_input("Japanese reference directory", &prefs.japanese_reference).on_input(Message::TranslationReference).size(13).padding(8), button("Browse…").on_press(Message::TranslationBrowse).style(quiet_button_style)].spacing(8),
                text("Validation limits").size(15).color(ACCENT),
                text("First matching rule wins. Use | between patterns and * for a prefix; empty pattern matches every file. Empty limits disable that check.").size(12).color(MUTED),
                row![button("Add rule").on_press(Message::TranslationRuleAdd), button("Restore defaults").on_press(Message::TranslationRuleReset)].spacing(8),
            ].spacing(10);
            for (index, rule) in rules.iter().enumerate() {
                let chars = rule.max_chars.map(|n| n.to_string()).unwrap_or_default();
                let lines = rule.max_lines.map(|n| n.to_string()).unwrap_or_default();
                list = list.push(row![
                    text_input("Path pattern", &rule.patterns).on_input(move |v| Message::TranslationRuleChange(index, 0, v)).padding(8),
                    text_input("Max chars", &chars).on_input(move |v| Message::TranslationRuleChange(index, 1, v)).width(120).padding(8),
                    text_input("Max lines", &lines).on_input(move |v| Message::TranslationRuleChange(index, 2, v)).width(120).padding(8),
                    button("Remove").on_press(Message::TranslationRuleRemove(index)),
                ].spacing(8));
            }
            return column![header, container(scrollable(list).height(Length::Fill)).padding(14).style(panel_style).height(Length::Fill)].spacing(10).height(Length::Fill).into();
        }

        use translation::{BrowserFilter as F, BrowserStatus as S};
        let (entries, matched) = translation::browser_rows(&state.files, &state.audits, &state.expanded, &state.file_filter, state.browser_filter);
        let reveal = !state.file_filter.is_empty() || state.browser_filter != F::All;
        let window = state.file_viewport.window(entries.len(), FILE_ROW);
        let mut file_list = column![virtual_space(window.start, FILE_ROW)].spacing(0);
        for entry in &entries[window.clone()] {
            let selected = state.doc.as_ref().is_some_and(|d| d.relative == entry.path);
            let color = match entry.status { S::Error => DANGER, S::Warning => WARNING, S::Clear => GOOD, S::Pending => MUTED, S::Uncovered => MUTED };
            let marker = match entry.status { S::Pending => "○", S::Uncovered => "–", _ => "●" };
            let open = reveal || state.expanded.contains(&entry.path);
            let prefix = if entry.folder { if open { "▾" } else { "▸" } } else { "·" };
            let label = if entry.folder { format!("{} ({})", entry.label, entry.count) } else { entry.label.clone() };
            let control = button(row![container(text("")).width((entry.depth.min(7) * 11) as f32), text(prefix).size(12).color(MUTED), text(marker).size(12).color(color), text(label).size(12).color(if selected { TEXT } else { color }).wrapping(iced::widget::text::Wrapping::None).width(Length::Fill)].spacing(5).align_y(iced::Alignment::Center))
                .width(Length::Fill).height(FILE_ROW).padding([3, 4]).style(move |theme, status| nav_style(theme, status, selected));
            let control = if entry.folder { control.on_press_maybe((!reveal).then(|| Message::TranslationFolder(entry.path.clone()))) } else { control.on_press(Message::TranslationFile(entry.path.clone())) };
            file_list = file_list.push(iced::widget::tooltip(control, text(entry.path.to_string_lossy().into_owned()).size(12), iced::widget::tooltip::Position::FollowCursor));
        }
        file_list = file_list.push(virtual_space(entries.len() - window.end, FILE_ROW));
        let options = [F::All, F::Pending, F::Issues, F::Length, F::Lines, F::Clear, F::Uncovered];
        let files = container(column![
            text(format!("FILES · {matched}/{}", state.files.len())).size(12).color(ACCENT),
            text_input("Filter files…", &state.file_filter).on_input(Message::TranslationFileFilter).padding(8).size(12),
            iced::widget::pick_list(options, Some(state.browser_filter), Message::TranslationStatus).width(Length::Fill).text_size(12),
            text(format!("Checked {}/{} · {} warnings · {} read errors · {} ignored", state.audits.len(), state.files.len(), audit_warnings, audit_errors, audit_ignored)).size(11).color(MUTED),
            row![text("● Length/error").size(10).color(DANGER), text("● Lines").size(10).color(WARNING), text("● Clear").size(10).color(GOOD), text("○ Pending").size(10).color(MUTED)].spacing(6),
            scrollable(file_list).id("translation-files").on_scroll(Message::TranslationFileScroll).direction(scrollable::Direction::Vertical(slim_scrollbar())).height(Length::Fill),
        ].spacing(6).height(Length::Fill)).padding(9).width(Length::FillPortion(2)).style(panel_style);

        let editor: Element<'_, Message> = if let Some(doc) = &state.doc {
            if state.paste_open {
                let mut preview_list = column![text(doc.relative.to_string_lossy().into_owned()).size(15).color(ACCENT), text("Paste into the selected file: [123]: text or [123]: \"line 1\\nline 2\". String IDs use JSON quotes. Preview validates every ID before staging.").size(12).color(MUTED),
                    text_editor(&state.paste_content).on_action(Message::TranslationPasteEdit).height(Length::Fixed(130.0)).padding(8),
                    row![button("Preview paste").on_press(Message::TranslationPastePreview), button("Stage changes").on_press_maybe(state.paste_preview.as_ref().filter(|c| !c.is_empty()).map(|_| Message::TranslationPasteApply)), button("Close").on_press(Message::TranslationPaste)].spacing(8),
                ].spacing(8);
                if let Some(changes) = &state.paste_preview {
                    preview_list = preview_list.push(text(format!("{} pending changes · Explicit save required", changes.len())).size(13).color(ACCENT));
                    for change in changes {
                        preview_list = preview_list.push(container(column![text(format!("ID {}", change.key)).size(12).color(MUTED), text(format!("− {}", change.old)).size(13).color(DANGER), text(format!("+ {}", change.new)).size(13).color(GOOD)].spacing(3)).padding(6).style(inset_style));
                    }
                }
                return column![header, row![files, container(scrollable(preview_list).height(Length::Fill)).width(Length::FillPortion(5)).height(Length::Fill).padding(12).style(panel_style)].spacing(10).height(Length::Fill)].spacing(10).height(Length::Fill).into();
            }
            let rule = translation::matching_rule(&rules, &doc.relative);
            let has_reference = !prefs.japanese_reference.trim().is_empty();
            let q = state.row_filter.to_lowercase();
            let filtered: Vec<usize> = doc.records.iter().enumerate().filter_map(|(index, record)| {
                if !q.is_empty() && !record.key.to_lowercase().contains(&q) && !record.label.to_lowercase().contains(&q) && !record.edited.to_lowercase().contains(&q) && !record.original.as_ref().is_some_and(|s| s.to_lowercase().contains(&q)) { return None; }
                if state.warnings {
                    let v = translation::validate(&record.edited, rule);
                    if !(v.too_long || v.too_many) || translation::ignored(&prefs.exceptions, root, &doc.relative, record, rule) { return None; }
                }
                Some(index)
            }).collect();
            let window = state.row_viewport.window(filtered.len(), RECORD_ROW);
            let mut list = column![virtual_space(window.start, RECORD_ROW)].spacing(0);
            for &index in &filtered[window.clone()] {
                let record = &doc.records[index];
                let v = translation::validate(&record.edited, rule);
                let ignored = translation::ignored(&prefs.exceptions, root, &doc.relative, record, rule);
                let (marker, color) = if ignored { ("ignored", MUTED) } else if v.too_long { ("length", DANGER) } else if v.too_many { ("lines", WARNING) } else if record.edited != record.saved { ("edited", ACCENT) } else if rule.is_some_and(|r| r.max_chars.is_some() || r.max_lines.is_some()) { ("✓", GOOD) } else { ("–", MUTED) };
                let jp = record.original.as_deref().unwrap_or("—");
                let chosen = state.selected == Some(index);
                let mut cells = row![
                    text(marker).size(11).color(color).width(53),
                    text(record.key.clone()).size(12).width(50).wrapping(iced::widget::text::Wrapping::None),
                    text(record.label.clone()).size(11).color(MUTED).width(120).wrapping(iced::widget::text::Wrapping::None),
                ].spacing(6).align_y(iced::Alignment::Center);
                if has_reference { cells = cells.push(text(one_line(jp)).size(12).width(Length::FillPortion(1)).wrapping(iced::widget::text::Wrapping::None)); }
                cells = cells.push(text(one_line(&record.edited)).size(13).color(if chosen { ACCENT } else { TEXT }).width(Length::FillPortion(2)).wrapping(iced::widget::text::Wrapping::None));
                list = list.push(button(cells).padding([4, 6]).style(move |theme, status| nav_style(theme, status, chosen)).height(RECORD_ROW).width(Length::Fill).on_press(Message::TranslationRow(index)));
            }
            list = list.push(virtual_space(filtered.len() - window.end, RECORD_ROW));
            let mut headings = row![text("STATUS").size(10).color(MUTED).width(53), text("ID").size(10).color(MUTED).width(50), text("LABEL").size(10).color(MUTED).width(120)].spacing(6);
            if has_reference { headings = headings.push(text("JAPANESE").size(10).color(MUTED).width(Length::FillPortion(1))); }
            headings = headings.push(text("TRANSLATION").size(10).color(MUTED).width(Length::FillPortion(2)));
            let mut panel = column![
                row![text(doc.relative.to_string_lossy().into_owned()).size(13).color(ACCENT).width(Length::Fill), button("Mass Paste").on_press(Message::TranslationPaste).style(quiet_button_style)].spacing(8).align_y(iced::Alignment::Center),
                row![text_input("Filter ID, label, JP or translation…", &state.row_filter).on_input(Message::TranslationFilter).padding(8), checkbox(state.warnings).label("Warnings only").on_toggle(Message::TranslationWarnings)].spacing(9).align_y(iced::Alignment::Center),
                row![text(format!("{} / {} records", filtered.len(), doc.records.len())).size(11).color(MUTED), container(text("")).width(Length::Fill), text(&doc.reference_status).size(11).color(MUTED)].spacing(8),
                container(headings).padding([0, 6]).width(Length::Fill),
                scrollable(list).id("translation-records").on_scroll(Message::TranslationRowScroll).direction(scrollable::Direction::Vertical(slim_scrollbar())).height(Length::FillPortion(3)),
            ].spacing(7);
            if translation::version_path(self.settings.active_game, &doc.relative) {
                let current = translation::current_version(doc, self.settings.active_game);
                panel = panel.push(row![text(format!("Mod version: {}", current.as_ref().map(|s| s.as_str()).unwrap_or("unconfirmed"))).size(12).color(MUTED), text_input("New version", &state.version).on_input(Message::TranslationVersion).width(120), button("Preview version").on_press_maybe(current.is_ok().then_some(Message::TranslationVersionPreview))].spacing(8));
            }
            if let Some(record) = state.selected.and_then(|i| doc.records.get(i)) {
                let v = translation::validate(&record.edited, rule);
                let ignored = translation::ignored(&prefs.exceptions, root, &doc.relative, record, rule);
                let mut counts = row![text("LINE LENGTH").size(11).color(MUTED)].spacing(8);
                for (i, n) in v.counts.iter().enumerate().take(12) {
                    let limit = rule.and_then(|r| r.max_chars);
                    counts = counts.push(text(format!("L{}: {}{}", i + 1, n, limit.map(|m| format!("/{m}")).unwrap_or_default())).size(11).color(if limit.is_some_and(|m| *n > m) { DANGER } else { GOOD }));
                }
                let original = record.original.as_deref().unwrap_or("No matching Japanese record");
                let jp_panel = column![text("JAPANESE · read-only").size(11).color(MUTED), container(scrollable(container(highlighted_text(original)).width(Length::Fill)).direction(scrollable::Direction::Vertical(slim_scrollbar())).height(Length::Fill)).padding(9).width(Length::Fill).height(Length::Fill).style(inset_style)].spacing(4).width(Length::FillPortion(1)).height(Length::Fill);
                let mut en_panel = column![text("TRANSLATION").size(11).color(ACCENT), text_editor(&state.editor).on_action(Message::TranslationEdit).size(15).padding(10).height(Length::Fill)].spacing(4).width(Length::FillPortion(1)).height(Length::Fill);
                if state.tag_preview { en_panel = en_panel.push(container(scrollable(container(highlighted_text(&record.edited)).width(Length::Fill)).height(Length::Fill)).height(Length::Fixed(56.0)).width(Length::Fill).padding(5).style(inset_style)); }
                let mut text_panels = row![].spacing(10).height(Length::Fill);
                if has_reference { text_panels = text_panels.push(jp_panel); }
                text_panels = text_panels.push(en_panel);
                let details = column![
                    row![text(format!("ID {} · {}", record.key, record.label)).size(12).color(ACCENT).width(Length::Fill), checkbox(state.tag_preview).label("Tag preview").on_toggle(Message::TranslationTagPreview)].spacing(8).align_y(iced::Alignment::Center),
                    text_panels,
                    scrollable(counts).direction(scrollable::Direction::Horizontal(slim_scrollbar())).width(Length::Fill),
                    row![text(if ignored { "Warning ignored" } else if v.too_long { "Line exceeds character limit" } else if v.too_many { "Too many lines" } else if rule.is_none_or(|r| r.max_chars.is_none() && r.max_lines.is_none()) { "No validation limits" } else { "No length warning" }).size(12).color(if ignored { MUTED } else if v.too_long { DANGER } else if v.too_many { WARNING } else { MUTED }), container(text("")).width(Length::Fill), button(if ignored { "Restore warning" } else { "Ignore warning" }).on_press_maybe((ignored || ((v.too_many || v.too_long) && record.saved == record.edited)).then_some(Message::TranslationException)).style(quiet_button_style)].spacing(8).align_y(iced::Alignment::Center),
                ].spacing(5).height(Length::Fill);
                panel = panel.push(container(details).padding(8).height(Length::FillPortion(4)).width(Length::Fill).style(inset_style));
            }
            panel.push(row![button("Save file").on_press_maybe(doc.dirty().then_some(Message::TranslationSave)), button("Discard unsaved edits").on_press_maybe(doc.dirty().then_some(Message::TranslationDiscard)), text(if doc.dirty() { "Unsaved changes" } else { "Saved" }).size(12).color(if doc.dirty() { ACCENT } else { MUTED })].spacing(10)).height(Length::Fill).into()
        } else {
            column![text("Select a JSON table to edit").size(20).color(TEXT), text("Only rows with explicit text fields and unique IDs are editable. Reference files are read-only.").size(12).color(MUTED)].spacing(8).into()
        };
        column![header, row![files, container(editor).width(Length::FillPortion(5)).height(Length::Fill).padding(12).style(panel_style)].spacing(10).height(Length::Fill)].height(Length::Fill).spacing(10).into()
    }

    fn operation_view(&self) -> Element<'_, Message> {
        let packing = self.kind() == jobs::Kind::Pack;
        let platform = self.platform_picker();
        let scope = row![
            nav_button("Batch", self.scope == jobs::Scope::Batch, Message::Scope(jobs::Scope::Batch)),
            nav_button("Single", self.scope == jobs::Scope::Single, Message::Scope(jobs::Scope::Single)),
        ].spacing(7);
        let xc3 = jobs::Layout::for_game(self.settings.active_game, self.settings.xcx_common_parent(self.switch2)) == jobs::Layout::Xenoblade3;
        let layouts = container(text(if xc3 { "game/ + evt/" } else { "Standard" }).size(13).color(TEXT))
            .padding([10, 16]).style(inset_style);

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
            container(text(format!("{}  /  {}", self.settings.active_game.label(), if self.switch2 { "Switch 2" } else { "Switch 1" }))
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

        let mut conversion_options = column![
                row![
                    text("CONVERSION OPTIONS").size(11).color(ACCENT),
                    container(text("")).width(Length::Fill),
                    text(if xc3 {
                        "XC3  •  evt layout applied on pack"
                    } else { "External bdat-toolset" }).size(11).color(MUTED),
                ].align_y(iced::Alignment::Center),
                options,
                paths,
            ].spacing(13);
        if packing && self.settings.active_game == Game::XenobladeX {
            let switch2 = self.switch2;
            let toggle = checkbox(self.settings.xcx_common_parent(switch2))
                .label(if switch2 {
                    "Place common.bdat one folder above destination (Switch 2)"
                } else {
                    "Place common.bdat one folder above destination (Switch 1)"
                });
            conversion_options = conversion_options.push(if self.running {
                toggle
            } else { toggle.on_toggle(move |enabled| Message::XcxCommonParent(switch2, enabled)) });
        }
        let configuration = container(conversion_options).padding(17).width(Length::Fill).style(panel_style);

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
            text("Shared backend for all games and platforms.")
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

        column![intro, backend_panel,
            text("Input and output paths are saved automatically in Pack and Extract for each game and platform.")
                .size(12).color(MUTED)]
            .spacing(15).height(Length::Fill).width(Length::Fill)
            .into()
    }
}

fn slim_scrollbar() -> scrollable::Scrollbar {
    scrollable::Scrollbar::new().width(8).scroller_width(5).margin(2).spacing(4)
}

fn highlighted_text(value: &str) -> Element<'_, Message> {
    let spans: Vec<iced::widget::text::Span<'_, (), iced::Font>> = translation::highlighted_parts(value).into_iter()
        .map(|(part, is_tag)| iced::widget::span(part).color(if is_tag { ACCENT } else { TEXT })).collect();
    iced::widget::rich_text(spans).size(14).into()
}

fn reset_list_scroll(id: &'static str) -> Task<Message> {
    iced::widget::operation::scroll_to(id, scrollable::AbsoluteOffset { x: Some(0.0), y: Some(0.0) })
}

fn virtual_space(count: usize, height: f32) -> Element<'static, Message> {
    container(text("")).height(Length::Fixed(count as f32 * height)).width(Length::Fill).into()
}

fn one_line(value: &str) -> String {
    value.replace("\\n", " ↵ ").replace(['\n', '\r'], " ").chars().take(180).collect()
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
const WARNING: Color = Color::from_rgb8(255, 176, 88);

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
    locked_nav_button(label, selected, message, false)
}

fn locked_nav_button<'a>(label: &'a str, selected: bool, message: Message, locked: bool) -> Element<'a, Message> {
    let style: fn(&Theme, button::Status) -> button::Style =
        if selected { selected_button_style } else { quiet_button_style };
    let control = button(text(label).size(13))
        .padding([10, 16])
        .style(style);
    if locked { control.into() } else { control.on_press(message).into() }
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
        row![
            text_input("Folder", value).on_input(move |value| Message::EditPath(field, value))
                .size(13).padding(12),
            button("Browse…").on_press(Message::BrowsePath(field))
                .padding([12, 15]).style(quiet_button_style),
        ].spacing(9),
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
