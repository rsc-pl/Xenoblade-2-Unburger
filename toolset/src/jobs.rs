//! File planning and conversion. No UI or game-content knowledge is hidden in the frontend.
use crate::{backend, config};
use chrono::Local;
use std::env;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind { Pack, Extract }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope { Batch, Single }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout { Auto, Standard, Xenoblade3 }

impl Layout {
    pub fn from_label(label: &str) -> Self {
        match label {
            "Standard" => Self::Standard,
            "Xenoblade 3" => Self::Xenoblade3,
            _ => Self::Auto,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Request {
    pub kind: Kind,
    pub scope: Scope,
    pub layout: Layout,
    pub source: PathBuf,
    pub destination: PathBuf,
    pub toolset: PathBuf,
}

#[derive(Clone, Debug)]
pub struct Item {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub output_name: String,
}

#[derive(Clone, Debug)]
pub struct Plan {
    pub kind: Kind,
    pub items: Vec<Item>,
    pub notes: Vec<String>,
}

pub fn plan(req: &Request) -> Result<Plan, String> {
    backend::check_path(&req.toolset)?;
    // Only absolute paths are accepted: a relative destination would depend on
    // whichever directory launched the GUI and could write into the repository.
    for (label, path) in [("Input", &req.source), ("Output", &req.destination)] {
        if !path.is_absolute() {
            return Err(format!("{label} must be an absolute path. Use Browse to select a location."));
        }
        if path.components().any(|part| matches!(part, Component::ParentDir | Component::CurDir)) {
            return Err(format!("{label} path contains . or .. components; select a normalized path."));
        }
    }
    if !req.source.exists() {
        return Err(format!("Source does not exist: {}", req.source.display()));
    }
    if req.destination.as_os_str().is_empty() {
        return Err("Choose a destination".into());
    }
    let mut items = Vec::new();
    let mut notes = Vec::new();
    match (req.kind, req.scope) {
        (Kind::Pack, Scope::Single) => {
            if !req.source.is_dir() { return Err("Single packing needs a JSON directory".into()); }
            let filename = req.destination.file_name()
                .ok_or("Choose a target .bdat filename")?.to_string_lossy().into_owned();
            if !filename.to_ascii_lowercase().ends_with(".bdat") {
                return Err("Output file must end in .bdat".into());
            }
            items.push(Item { source: req.source.clone(), destination: req.destination.clone(), output_name: filename });
        }
        (Kind::Extract, Scope::Single) => {
            if !req.source.is_file() { return Err("Single extraction needs a .bdat file".into()); }
            let stem = stem_of(&req.source)?;
            items.push(Item { source: req.source.clone(), destination: req.destination.join(&stem), output_name: stem });
        }
        (Kind::Pack, Scope::Batch) => {
            if !req.source.is_dir() { return Err("Batch packing needs a source directory".into()); }
            let xc3 = use_xc3(req.layout, &req.source);
            if xc3 {
                for group in ["game", "evt"] {
                    let group_dir = req.source.join(group);
                    if !group_dir.is_dir() { continue; }
                    for folder in child_directories(&group_dir)? {
                        let stem = stem_of(&folder)?;
                        let name = format!("{stem}.bdat");
                        let dest = if group == "evt" {
                            match evt_destination(&name) {
                                Some(mapped) => req.destination.join(group).join(mapped).join(&name),
                                None => {
                                    notes.push(format!("Unmatched XC3 evt name: {name} (stays in evt root)"));
                                    req.destination.join(group).join(&name)
                                }
                            }
                        } else { req.destination.join(group).join(&name) };
                        items.push(Item { source: folder, destination: dest, output_name: name });
                    }
                }
                if items.is_empty() {
                    return Err("No game/evt JSON directories found in XC3 input".into());
                }
            } else {
                for folder in child_directories(&req.source)? {
                    let stem = stem_of(&folder)?;
                    let filename = format!("{stem}.bdat");
                    items.push(Item { source: folder, destination: req.destination.join(&filename), output_name: filename });
                }
            }
        }
        (Kind::Extract, Scope::Batch) => {
            if !req.source.is_dir() { return Err("Batch extraction needs a BDAT directory".into()); }
            let xc3 = use_xc3(req.layout, &req.source);
            if xc3 {
                // Deliberately flatten working folders for each group, unlike game runtime paths.
                for group in ["game", "evt"] {
                    let root = req.source.join(group);
                    if root.is_dir() {
                        for bdat in recursive_bdats(&root)? {
                            let stem = stem_of(&bdat)?;
                            items.push(Item {
                                source: bdat,
                                destination: req.destination.join(group).join(&stem),
                                output_name: stem,
                            });
                        }
                    }
                }
            } else {
                for bdat in direct_bdats(&req.source)? {
                    let stem = stem_of(&bdat)?;
                    items.push(Item { source: bdat, destination: req.destination.join(&stem), output_name: stem });
                }
            }
        }
    }
    if items.is_empty() { return Err("No suitable files or directories found".into()); }
    items.sort_by(|a, b| a.destination.cmp(&b.destination));
    // Colliding stems would silently overwrite the same output if not rejected.
    for pair in items.windows(2) {
        if pair[0].destination == pair[1].destination {
            return Err(format!("Output collision: {} ({} and {})", pair[0].destination.display(), pair[0].source.display(), pair[1].source.display()));
        }
    }
    Ok(Plan { kind: req.kind, items, notes })
}

fn use_xc3(layout: Layout, base: &Path) -> bool {
    match layout {
        Layout::Xenoblade3 => true,
        Layout::Standard => false,
        Layout::Auto => base.join("game").is_dir() || base.join("evt").is_dir(),
    }
}

fn stem_of(path: &Path) -> Result<String, String> {
    path.file_stem().map(|s| s.to_string_lossy().into_owned()).filter(|s| !s.is_empty())
        .ok_or_else(|| format!("Invalid filename: {}", path.display()))
}

fn child_directories(base: &Path) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    for entry in fs::read_dir(base).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_type().map_err(|e| e.to_string())?.is_dir() { out.push(entry.path()); }
    }
    out.sort(); Ok(out)
}

fn direct_bdats(base: &Path) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    for entry in fs::read_dir(base).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_type().map_err(|e| e.to_string())?.is_file() && is_bdat(&entry.path()) { out.push(entry.path()); }
    }
    out.sort(); Ok(out)
}

fn recursive_bdats(base: &Path) -> Result<Vec<PathBuf>, String> {
    let mut pending = vec![base.to_path_buf()];
    let mut out = Vec::new();
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let ty = entry.file_type().map_err(|e| e.to_string())?;
            if ty.is_dir() { pending.push(entry.path()); }
            else if ty.is_file() && is_bdat(&entry.path()) { out.push(entry.path()); }
        }
    }
    out.sort(); Ok(out)
}

fn is_bdat(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.to_string_lossy().eq_ignore_ascii_case("bdat"))
}

/// Exact effective behavior of Python's `reorganize_evt_folder`.
/// The duplicate exception `msg_ev10010100.bdat` resolves to ev01 in Python.
pub fn evt_destination(filename: &str) -> Option<&'static str> {
    let exception = match filename {
        "msg_ev10030100.bdat" => Some("ev30"),
        "msg_mv021801.bdat" | "msg_mv062602.bdat" => Some("cq"),
        "msg_ev10010100.bdat" | "msg_mv01100110.bdat" | "msg_mv01290110.bdat" | "msg_mv01310140.bdat" => Some("ev01"),
        "msg_ev10020100.bdat" | "msg_ev10020200.bdat" | "msg_mv03285130.bdat" => Some("ev03"),
        "msg_mv04220110.bdat" => Some("ev04"),
        "msg_ev_featuredsong.bdat" | "msg_ev10020300.bdat" => Some("ev05"),
        "msg_ev10020400.bdat" => Some("ev06"),
        "msg_ev_endthemesong.bdat" => Some("ev07"),
        _ => None,
    };
    if exception.is_some() { return exception; }
    const FOLDERS: &[&str] = &["ask", "cq", "ev01", "ev02", "ev03", "ev04", "ev05", "ev06", "ev07", "ev08", "ev09", "ev10", "ev11", "ev12", "ev13", "ev14", "ev15", "ev30", "ev40", "fev", "nq", "sq", "tlk", "tq"];
    FOLDERS.iter().copied().find(|folder| filename.starts_with(&format!("msg_{folder}")))
}

// Preserve diagnostic messages while removing known workstation paths
// before they reach persistent logs or the console transcript.
#[derive(Clone)]
struct LogRedactor {
    paths: Vec<String>,
}

impl LogRedactor {
    fn new(req: &Request, item: &Item, staging: Option<&Path>) -> Self {
        let mut paths = Vec::new();
        let mut add = |value: &Path| {
            let text = value.to_string_lossy().into_owned();
            if text.len() > 3 {
                paths.push(text.clone());
                paths.push(text.replace('\\', "/"));
            }
        };
        for value in [&req.toolset, &req.source, &req.destination, &item.source, &item.destination] {
            add(value);
        }
        if let Some(parent) = item.destination.parent() { add(parent); }
        if let Some(path) = staging { add(path); }
        for name in ["HOME", "USERPROFILE"] {
            if let Some(home) = env::var_os(name) { add(Path::new(&home)); }
        }
        paths.sort_by_key(|value| std::cmp::Reverse(value.len()));
        paths.dedup();
        Self { paths }
    }

    fn clean(&self, line: &str) -> String {
        self.paths.iter().fold(line.to_owned(), |text, path| text.replace(path, "[path]"))
    }
}

#[derive(Debug, Clone)]
pub enum Event {
    Line(String),
    Progress { completed: usize, succeeded: usize, failed: usize },
    Failure(String),
    Fatal(String),
    Finished { completed: usize, succeeded: usize, failed: usize, log_file: String, error_file: String },
}

pub fn launch(req: Request, plan: Plan, sender: mpsc::Sender<Event>, cancel: Arc<AtomicBool>) {
    thread::spawn(move || run(req, plan, sender, cancel));
}

fn run(req: Request, plan: Plan, sender: mpsc::Sender<Event>, cancel: Arc<AtomicBool>) {
    let path = match config::log_directory() {
        Ok(p) => p,
        Err(e) => { let _ = sender.send(Event::Fatal(format!("Cannot determine log directory: {e}"))); return; }
    };
    if let Err(e) = fs::create_dir_all(&path) { let _ = sender.send(Event::Fatal(format!("Cannot create log directory: {e}"))); return; }
    let id = format!("{}-{}", Local::now().format("%Y%m%d-%H%M%S-%3f"), std::process::id());
    let log_path = path.join(format!("{id}.log"));
    let err_path = path.join(format!("{id}.errors.log"));
    let log = match File::create(&log_path) { Ok(f) => Arc::new(Mutex::new(f)), Err(e) => { let _ = sender.send(Event::Fatal(format!("Cannot create full log: {e}"))); return; } };
    let mut errors = match File::create(&err_path) { Ok(f) => f, Err(e) => { let _ = sender.send(Event::Fatal(format!("Cannot create error log: {e}"))); return; } };
    for note in &plan.notes { send_log(&sender, &log, format!("WARNING: {note}")); }
    let mut succeeded = 0;
    let mut failed = 0;
    let mut completed = 0;
    for item in &plan.items {
        if cancel.load(Ordering::Relaxed) {
            send_log(&sender, &log, "Cancelled before next file".into());
            break;
        }
        send_log(&sender, &log, format!("START: {}", item.output_name));
        match run_item(&req, &plan, item, &sender, &log) {
            Ok(()) => { succeeded += 1; send_log(&sender, &log, format!("OK: {}", item.output_name)); }
            Err(problem) => {
                failed += 1;
                let clean_problem = LogRedactor::new(&req, item, None).clean(&problem);
                let detail = format!("FAIL: {}\n{}\n", item.output_name, clean_problem);
                if let Err(e) = writeln!(errors, "{detail}").and_then(|_| errors.flush()) {
                    let _ = sender.send(Event::Failure(format!("Cannot write error report: {e}")));
                }
                send_log(&sender, &log, detail.clone());
                let _ = sender.send(Event::Failure(detail));
            }
        }
        completed += 1;
        let _ = sender.send(Event::Progress { completed, succeeded, failed });
    }
    let _ = sender.send(Event::Finished {
        completed, succeeded, failed,
        log_file: log_path.display().to_string(), error_file: err_path.display().to_string(),
    });
}

fn run_item(req: &Request, plan: &Plan, item: &Item, sender: &mpsc::Sender<Event>, log: &Arc<Mutex<File>>) -> Result<(), String> {
    let parent = item.destination.parent().ok_or("Output has no parent")?;
    fs::create_dir_all(parent).map_err(|e| format!("Create output parent: {e}"))?;
    let tmp = tempfile::Builder::new().prefix(".bdat-studio-").tempdir_in(parent)
        .map_err(|e| format!("Create staging dir: {e}"))?;
    let staging = tmp.path().join("result");
    let executable = req.toolset.canonicalize()
        .map_err(|e| format!("Resolve backend executable: {e}"))?;
    let input = item.source.canonicalize()
        .map_err(|e| format!("Resolve input path: {e}"))?;
    let mut cmd = Command::new(executable);
    fs::create_dir_all(&staging).map_err(|e| format!("Create staging directory: {e}"))?;
    // The backend's -o argument is always a directory. The output filename
    // is encoded in the .bschema data, not provided as an -o filename.
    if plan.kind == Kind::Pack {
        cmd.arg("pack").arg(&input).arg("-o").arg(&staging).arg("-f").arg("json");
    } else {
        cmd.arg("extract").arg(&input).arg("-o").arg(&staging).arg("-f").arg("json").arg("--pretty");
    }
    let redactor = LogRedactor::new(req, item, Some(&staging));
    let command_name = if plan.kind == Kind::Pack { "pack" } else { "extract" };
    let extra = if plan.kind == Kind::Extract { " --pretty" } else { "" };
    send_log(sender, log, format!("CMD: bdat-toolset {command_name} [input] -o [staging] -f json{extra}"));
    // The backend must never inherit the GUI launcher working directory.
    // Any relative path generated by the child will resolve inside the
    // temporary staging directory under the selected output location.
    cmd.current_dir(&staging);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("Cannot start bdat-toolset: {e}"))?;
    let stdout = child.stdout.take().ok_or("Could not capture stdout")?;
    let stderr = child.stderr.take().ok_or("Could not capture stderr")?;
    let sender_a = sender.clone();
    let logger_a = Arc::clone(log);
    let clean_a = redactor.clone();
    let stdout_handle = thread::spawn(move || read_stream(stdout, "OUT", &sender_a, &logger_a, &clean_a));
    let sender_b = sender.clone();
    let logger_b = Arc::clone(log);
    let stderr_handle = thread::spawn(move || read_stream(stderr, "ERR", &sender_b, &logger_b, &redactor));
    let exit = child.wait().map_err(|e| format!("Wait for bdat-toolset: {e}"))?;
    let out_lines = stdout_handle.join().unwrap_or_else(|_| vec!["stdout thread panicked".into()]);
    let err_lines = stderr_handle.join().unwrap_or_else(|_| vec!["stderr thread panicked".into()]);
    send_log(sender, log, format!("EXIT: {} -> {}", item.output_name, exit.code().map_or_else(|| "signal".into(), |code| code.to_string())));
    if !exit.success() {
        return Err(format!("Exit code: {:?}\nSTDOUT:\n{}\nSTDERR:\n{}", exit.code(), out_lines.join("\n"), err_lines.join("\n")));
    }
    let expected = if plan.kind == Kind::Pack {
        let generated = find_generated_bdats(&staging)?;
        if generated.len() != 1 {
            let names = generated.iter()
                .filter_map(|p| p.file_name().map(|f| f.to_string_lossy().into_owned()))
                .collect::<Vec<_>>().join(", ");
            return Err(format!(
                "Tool returned 0, but produced {} BDAT files (expected exactly 1). Files: [{}]. Check source schemas and the backend console.",
                generated.len(), names
            ));
        }
        let output = generated.into_iter().next().ok_or("Missing generated BDAT")?;
        let actual_name = output.file_name().ok_or("Generated BDAT has no filename")?.to_string_lossy();
        if actual_name != item.output_name {
            return Err(format!(
                "Tool returned 0, but schema generated '{actual_name}' while destination expects '{}'. The previous output was preserved.",
                item.output_name
            ));
        }
        let info = fs::metadata(&output).map_err(|e| format!("Read generated BDAT metadata: {e}"))?;
        if !info.is_file() || info.len() == 0 { return Err("Tool returned 0 but produced an empty BDAT".into()); }
        output
    } else {
        let mut entries = fs::read_dir(&staging).map_err(|e| format!("Tool returned 0 but output directory missing: {e}"))?;
        if entries.next().is_none() { return Err("Tool returned 0 but extracted directory is empty".into()); }
        staging.clone()
    };
    commit_result(&expected, &item.destination)
}

fn find_generated_bdats(directory: &Path) -> Result<Vec<PathBuf>, String> {
    let mut found = Vec::new();
    let mut pending = vec![directory.to_path_buf()];
    while let Some(parent) = pending.pop() {
        for entry in fs::read_dir(&parent).map_err(|e| format!("Read staged output: {e}"))? {
            let entry = entry.map_err(|e| format!("Inspect staged output: {e}"))?;
            let kind = entry.file_type().map_err(|e| format!("Inspect staged file type: {e}"))?;
            if kind.is_symlink() { return Err("Staged output contains an unexpected symlink".into()); }
            if kind.is_dir() { pending.push(entry.path()); }
            else if kind.is_file() && is_bdat(&entry.path()) { found.push(entry.path()); }
        }
    }
    found.sort();
    Ok(found)
}

fn read_stream<R: io::Read>(stream: R, tag: &str, sender: &mpsc::Sender<Event>, log: &Arc<Mutex<File>>, redactor: &LogRedactor) -> Vec<String> {
    let mut lines = Vec::new();
    for line in BufReader::new(stream).lines() {
        match line {
            Ok(s) => {
                let safe = redactor.clean(&s);
                send_log(sender, log, format!("[{tag}] {safe}"));
                lines.push(safe);
            }
            Err(e) => { let message = format!("Read {tag} failed: {e}"); send_log(sender, log, message.clone()); lines.push(message); break; }
        }
    }
    lines
}

fn send_log(sender: &mpsc::Sender<Event>, log: &Arc<Mutex<File>>, message: String) {
    match log.lock() {
        Ok(mut f) => {
            if let Err(e) = writeln!(f, "{message}").and_then(|_| f.flush()) {
                let _ = sender.send(Event::Failure(format!("Cannot write conversion log: {e}")));
            }
        }
        Err(_) => {
            let _ = sender.send(Event::Failure("Conversion log is unavailable".into()));
        }
    }
    // A closed UI receiver cannot be repaired from the conversion thread.
    let _ = sender.send(Event::Line(message));
}

/// Stage on the same destination filesystem. Never delete a previous successful file
/// before a newly generated replacement passes validation.
fn commit_result(from: &Path, to: &Path) -> Result<(), String> {
    if fs::symlink_metadata(to).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err("Destination is a symbolic link; refusing to replace it".into());
    }
    if !to.exists() {
        return fs::rename(from, to).map_err(|e| format!("Install result: {e}"));
    }
    let parent = to.parent().ok_or("Destination without parent")?;
    let backup_holder = tempfile::Builder::new().prefix(".bdat-backup-").tempdir_in(parent)
        .map_err(|e| format!("Create rollback directory: {e}"))?;
    let backup = backup_holder.path().join("previous");
    fs::rename(to, &backup).map_err(|e| format!("Move old result to rollback area: {e}"))?;
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(e) => {
            let restoration = fs::rename(&backup, to);
            match restoration {
                Ok(()) => Err(format!("Install replacement failed ({e}); original restored")),
                Err(restore) => {
                    // Critical: avoid deletion of the previous result when rollback fails.
                    let _ = backup_holder.keep();
                    Err(format!("Install replacement failed ({e}); ROLLBACK FAILED ({restore}), old result retained in the staging backup directory"))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn xc3_exceptions_match_python_effective_values() {
        assert_eq!(evt_destination("msg_ev10010100.bdat"), Some("ev01"));
        assert_eq!(evt_destination("msg_ev10030100.bdat"), Some("ev30"));
        assert_eq!(evt_destination("msg_mv021801.bdat"), Some("cq"));
        assert_eq!(evt_destination("msg_ev10020100.bdat"), Some("ev03"));
        assert_eq!(evt_destination("msg_ev_endthemesong.bdat"), Some("ev07"));
        assert_eq!(evt_destination("other.bdat"), None);
    }
    #[test]
    fn xc3_prefixes_follow_python_order() {
        assert_eq!(evt_destination("msg_sq_a.bdat"), Some("sq"));
        assert_eq!(evt_destination("msg_ev13_x.bdat"), Some("ev13"));
        assert_eq!(evt_destination("msg_fev_test.bdat"), Some("fev"));
        assert_eq!(evt_destination("msg_nq_foo.bdat"), Some("nq"));
    }
    #[test]
    fn log_redactor_omits_known_private_paths() {
        let req = Request {
            kind: Kind::Pack, scope: Scope::Batch, layout: Layout::Standard,
            source: PathBuf::from("private-source/folder"),
            destination: PathBuf::from("private-output/results"),
            toolset: PathBuf::from("private-backend/bdat-toolset"),
        };
        let item = Item {
            source: PathBuf::from("private-source/folder/table"),
            destination: PathBuf::from("private-output/results/table.bdat"),
            output_name: "table.bdat".into(),
        };
        let line = "tool: private-backend/bdat-toolset; input private-source/folder/table; output private-output/results/table.bdat";
        let clean = LogRedactor::new(&req, &item, None).clean(line);
        assert!(!clean.contains("private-source"));
        assert!(!clean.contains("private-output"));
        assert!(!clean.contains("private-backend"));
        assert!(clean.contains("[path]"));
    }

    #[test]
    fn finds_staged_bdats_without_assuming_output_directory_shape() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("nested")).unwrap();
        fs::write(tmp.path().join("nested/table.bdat"), b"BDAT").unwrap();
        let output = find_generated_bdats(tmp.path()).unwrap();
        assert_eq!(output.len(), 1);
        assert_eq!(output[0].file_name().unwrap(), "table.bdat");
    }

    #[cfg(unix)]
    fn make_mock_backend(script: &str, root: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = root.join("bdat-toolset");
        fs::write(&path, script).unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o700);
        fs::set_permissions(&path, perms).unwrap();
        path
    }

    #[cfg(unix)]
    fn run_mock_pack(script: &str) -> (Result<(), String>, Vec<u8>) {
        let tmp = tempfile::tempdir().unwrap();
        let backend = make_mock_backend(script, tmp.path());
        let input = tmp.path().join("table");
        let output = tmp.path().join("out/table.bdat");
        fs::create_dir_all(&input).unwrap();
        fs::create_dir_all(output.parent().unwrap()).unwrap();
        fs::write(&output, b"previous").unwrap();
        let req = Request {
            kind: Kind::Pack, scope: Scope::Single, layout: Layout::Standard,
            source: input.clone(), destination: output.clone(), toolset: backend,
        };
        let item = Item { source: input, destination: output.clone(), output_name: "table.bdat".into() };
        let plan = Plan { kind: Kind::Pack, items: vec![item.clone()], notes: Vec::new() };
        let (sender, _receiver) = mpsc::channel();
        let log = Arc::new(Mutex::new(File::create(tmp.path().join("test.log")).unwrap()));
        let result = run_item(&req, &plan, &item, &sender, &log);
        (result, fs::read(&output).unwrap())
    }

    #[cfg(unix)]
    #[test]
    fn single_pack_passes_an_output_directory_and_preserves_valid_result() {
        let script = "#!/bin/sh\n[ \"$1\" = pack ] || exit 3\n[ \"$3\" = -o ] || exit 4\n[ -d \"$4\" ] || exit 5\nprintf BDAT > \"$4/table.bdat\"\nexit 0\n";
        let (result, output) = run_mock_pack(script);
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(output, b"BDAT");
    }

    #[cfg(unix)]
    #[test]
    fn success_exit_without_generated_bdat_keeps_previous_file() {
        let (result, output) = run_mock_pack("#!/bin/sh\nexit 0\n");
        assert!(result.unwrap_err().contains("produced 0 BDAT"));
        assert_eq!(output, b"previous");
    }

    #[cfg(unix)]
    #[test]
    fn schema_filename_mismatch_keeps_previous_file() {
        let script = "#!/bin/sh\nprintf BDAT > \"$4/unexpected.bdat\"\nexit 0\n";
        let (result, output) = run_mock_pack(script);
        assert!(result.unwrap_err().contains("schema generated"));
        assert_eq!(output, b"previous");
    }

    #[test]
    fn refuses_relative_destination_that_could_write_into_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("input");
        fs::create_dir_all(&src).unwrap();
        let req = Request {
            kind: Kind::Pack, scope: Scope::Batch, layout: Layout::Standard,
            source: src, destination: PathBuf::from("home/example/output"),
            toolset: std::env::current_exe().unwrap(),
        };
        assert!(plan(&req).unwrap_err().contains("absolute path"));
    }

    #[cfg(unix)]
    #[test]
    fn subprocess_current_dir_is_staged_in_requested_destination() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = make_mock_backend(
            "#!/bin/sh\n[ \"$1\" = pack ] || exit 3\n[ -d \"$4\" ] || exit 4\n[ \"$PWD\" = \"$4\" ] || exit 5\nprintf BDAT > \"$4/table.bdat\"\n",
            tmp.path(),
        );
        let source = tmp.path().join("input/table");
        let dest = tmp.path().join("output/table.bdat");
        fs::create_dir_all(&source).unwrap();
        let req = Request {
            kind: Kind::Pack, scope: Scope::Single, layout: Layout::Standard,
            source: source.clone(), destination: dest.clone(), toolset: backend,
        };
        let item = Item { source, destination: dest.clone(), output_name: "table.bdat".into() };
        let plan = Plan { kind: Kind::Pack, items: vec![item.clone()], notes: vec![] };
        let (sender, _receiver) = mpsc::channel();
        let log = Arc::new(Mutex::new(File::create(tmp.path().join("test.log")).unwrap()));
        assert!(run_item(&req, &plan, &item, &sender, &log).is_ok());
        assert_eq!(fs::read(&dest).unwrap(), b"BDAT");
    }

    #[test]
    fn detects_colliding_xc3_names() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("input");
        fs::create_dir_all(root.join("evt/ev01")).unwrap();
        fs::create_dir_all(root.join("evt/ev30")).unwrap();
        fs::write(root.join("evt/ev01/msg_ev10010100.bdat"), b"hi").unwrap();
        fs::write(root.join("evt/ev30/msg_ev10010100.bdat"), b"hi").unwrap();
        let req = Request {
            kind: Kind::Extract, scope: Scope::Batch, layout: Layout::Xenoblade3,
            source: root, destination: tmp.path().join("out"), toolset: std::env::current_exe().unwrap(),
        };
        assert!(plan(&req).unwrap_err().contains("collision"));
    }
}
