//! JSON translation editing and validation, independent of the BDAT converter.
use crate::config::Game;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::{HashMap, HashSet}, fs, io::Write, path::{Path, PathBuf}, sync::{OnceLock, atomic::{AtomicBool, Ordering}}};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub japanese_reference: String,
    pub rules: Option<Vec<Rule>>,
    pub exceptions: Vec<WarningException>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WarningException {
    pub root: String,
    pub relative: String,
    pub key: String,
    pub text: String,
    pub max_chars: Option<usize>,
    pub max_lines: Option<usize>,
}

fn relative_key(path: &Path) -> String { path.to_string_lossy().replace('\\', "/") }

impl WarningException {
    pub fn new(root: &str, path: &Path, record: &Record, rule: &Rule) -> Self {
        Self { root: root.into(), relative: relative_key(path), key: record.key.clone(), text: record.edited.clone(), max_chars: rule.max_chars, max_lines: rule.max_lines }
    }
    pub fn same_record(&self, root: &str, path: &Path, key: &str) -> bool {
        self.root == root && self.relative == relative_key(path) && self.key == key
    }
}

pub fn ignored(exceptions: &[WarningException], root: &str, path: &Path, record: &Record, rule: Option<&Rule>) -> bool {
    rule.is_some_and(|r| exceptions.iter().any(|e| e.same_record(root, path, &record.key) && e.text == record.edited && e.max_chars == r.max_chars && e.max_lines == r.max_lines))
}

#[derive(Clone, Debug, Default)]
pub struct FileAudit {
    pub records: usize,
    pub warnings: usize,
    pub long_lines: usize,
    pub many_lines: usize,
    pub ignored: usize,
    pub covered: bool,
    pub reference: bool,
    pub error: Option<String>,
}

impl FileAudit {
    pub fn issue(&self) -> bool { self.warnings != 0 || self.error.is_some() }
}

pub fn audit_document(doc: &Document, root: &str, rules: &[Rule], exceptions: &[WarningException]) -> FileAudit {
    let rule = matching_rule(rules, &doc.relative);
    let covered = rule.is_some_and(|r| r.max_chars.is_some() || r.max_lines.is_some());
    let mut result = FileAudit { records: doc.records.len(), covered, ..FileAudit::default() };
    if covered {
        for record in &doc.records {
            let v = validate(&record.edited, rule);
            if v.too_long || v.too_many {
                if ignored(exceptions, root, &doc.relative, record, rule) { result.ignored += 1; }
                else {
                    result.warnings += 1;
                    if v.too_long { result.long_lines += 1; }
                    if v.too_many { result.many_lines += 1; }
                }
            }
        }
    }
    result
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum BrowserStatus { Clear, Uncovered, Pending, Warning, Error }

impl BrowserStatus {
    pub fn of(audit: Option<&FileAudit>) -> Self {
        match audit {
            None => Self::Pending,
            Some(a) if a.error.is_some() || a.long_lines > 0 => Self::Error,
            Some(a) if a.many_lines > 0 || a.warnings > 0 => Self::Warning,
            Some(a) if a.covered => Self::Clear,
            Some(_) => Self::Uncovered,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BrowserFilter { #[default] All, Pending, Issues, Length, Lines, Clear, Uncovered }

impl std::fmt::Display for BrowserFilter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self { Self::All => "All statuses", Self::Pending => "Pending", Self::Issues => "All problems", Self::Length => "Length / read errors", Self::Lines => "Too many lines", Self::Clear => "Checked · clear", Self::Uncovered => "No rule / reference" })
    }
}

impl BrowserFilter {
    pub fn matches(self, audit: Option<&FileAudit>) -> bool {
        match self {
            Self::All => true,
            Self::Pending => audit.is_none(),
            Self::Issues => audit.is_some_and(FileAudit::issue),
            Self::Length => audit.is_some_and(|a| a.long_lines > 0 || a.error.is_some()),
            Self::Lines => audit.is_some_and(|a| a.many_lines > 0),
            Self::Clear => BrowserStatus::of(audit) == BrowserStatus::Clear,
            Self::Uncovered => BrowserStatus::of(audit) == BrowserStatus::Uncovered,
        }
    }
}

#[derive(Debug, Clone)]
pub struct BrowserRow {
    pub path: PathBuf,
    pub label: String,
    pub depth: usize,
    pub folder: bool,
    pub status: BrowserStatus,
    pub count: usize,
}

/// Collapse every one-file subtree into its relative file path. Multi-file
/// folders remain expandable. Filtering is independent of folder expansion.
pub fn browser_rows(files: &[PathBuf], audits: &std::collections::BTreeMap<PathBuf, FileAudit>, expanded: &HashSet<PathBuf>, query: &str, filter: BrowserFilter) -> (Vec<BrowserRow>, usize) {
    use std::collections::BTreeMap;
    fn add(rows: &mut Vec<BrowserRow>, files: &[&PathBuf], parent: &Path, depth: usize, audits: &BTreeMap<PathBuf, FileAudit>, expanded: &HashSet<PathBuf>, reveal: bool) {
        let mut dirs: BTreeMap<PathBuf, Vec<&PathBuf>> = BTreeMap::new();
        let mut direct = Vec::new();
        for &file in files {
            let relative = file.strip_prefix(parent).unwrap_or(file);
            let mut parts = relative.components();
            if let Some(first) = parts.next() {
                if parts.next().is_some() { dirs.entry(parent.join(first.as_os_str())).or_default().push(file); }
                else { direct.push(file); }
            }
        }
        let leaf = |file: &PathBuf| BrowserRow { path: file.clone(), label: relative_key(file.strip_prefix(parent).unwrap_or(file)), depth, folder: false, status: BrowserStatus::of(audits.get(file)), count: 1 };
        for (dir, members) in dirs {
            if members.len() == 1 { direct.push(members[0]); continue; }
            let status = members.iter().map(|p| BrowserStatus::of(audits.get(*p))).max().unwrap_or(BrowserStatus::Uncovered);
            rows.push(BrowserRow { label: relative_key(dir.strip_prefix(parent).unwrap_or(&dir)), path: dir.clone(), depth, folder: true, status, count: members.len() });
            if reveal || expanded.contains(&dir) { add(rows, &members, &dir, depth + 1, audits, expanded, reveal); }
        }
        direct.sort();
        rows.extend(direct.into_iter().map(leaf));
    }
    let query = query.to_lowercase();
    let matching: Vec<_> = files.iter().filter(|p| relative_key(p).to_lowercase().contains(&query) && filter.matches(audits.get(*p))).collect();
    let mut rows = Vec::new();
    add(&mut rows, &matching, Path::new(""), 0, audits, expanded, !query.is_empty() || filter != BrowserFilter::All);
    (rows, matching.len())
}

/// Visible window plus overscan for fixed-height continuous scroll rows.
pub fn visible_window(total: usize, offset: f32, height: f32, row: f32) -> std::ops::Range<usize> {
    let height = height.max(1.0);
    let top = offset.max(0.0).min((total as f32 * row - height).max(0.0));
    let start = ((top / row).floor() as usize).saturating_sub(12);
    let end = (((top + height) / row).ceil() as usize).saturating_add(12).min(total);
    start.min(end)..end
}

pub fn audit_file(root: &str, path: &Path, game: Game, prefs: &Preferences, rules: &[Rule]) -> FileAudit {
    if !prefs.japanese_reference.trim().is_empty() {
        if let (Ok(reference), Ok(candidate)) = (Path::new(&prefs.japanese_reference).canonicalize(), checked_file(Path::new(root), path)) {
            if candidate.starts_with(reference) { return FileAudit { reference: true, ..FileAudit::default() }; }
        }
    }
    match Document::open(Path::new(root), path, "", game) {
        Ok(doc) => audit_document(&doc, root, rules, &prefs.exceptions),
        Err(e) => FileAudit { error: Some(e), ..FileAudit::default() },
    }
}

#[derive(Debug, Clone)]
pub struct PasteChange { pub index: usize, pub key: String, pub old: String, pub new: String }

/// Parse the clipboard format [id]: text. JSON-quoted IDs distinguish string IDs
/// from numeric IDs, and JSON-quoted text can represent actual line breaks.
pub fn preview_paste(doc: &Document, input: &str) -> Result<Vec<PasteChange>, String> {
    let mut changes = Vec::new();
    let mut seen = HashSet::new();
    let lookup: HashMap<&str, usize> = doc.records.iter().enumerate().map(|(i, r)| (r.key.as_str(), i)).collect();
    for (line_no, line) in input.lines().enumerate() {
        if line.trim().is_empty() { continue; }
        let (key, value) = line.strip_prefix('[').and_then(|l| l.split_once("]:"))
            .ok_or_else(|| format!("Line {}: expected [id]: text", line_no + 1))?;
        let id_value: Value = serde_json::from_str(key.trim()).map_err(|_| format!("Line {}: ID must be an integer or JSON-quoted string", line_no + 1))?;
        if !(id_value.is_string() || id_value.is_i64() || id_value.is_u64()) { return Err(format!("Invalid ID on line {}", line_no + 1)); }
        let key = id_value.to_string();
        if !seen.insert(key.clone()) { return Err(format!("Duplicate ID {key}")); }
        let index = *lookup.get(key.as_str()).ok_or_else(|| format!("ID {key} not found in selected file"))?;
        let value = value.strip_prefix(' ').unwrap_or(value);
        let new = if value.starts_with('"') { serde_json::from_str::<String>(value).map_err(|e| format!("Line {}: {e}", line_no + 1))? } else { value.to_owned() };
        let old = doc.records[index].edited.clone();
        if old != new { changes.push(PasteChange { index, key, old, new }); }
    }
    Ok(changes)
}

pub fn stage_changes(doc: &mut Document, changes: &[PasteChange]) -> Result<(), String> {
    let mut seen = HashSet::new();
    for c in changes {
        if !seen.insert(c.index) || !doc.records.get(c.index).is_some_and(|r| r.key == c.key && r.edited == c.old) { return Err("Preview is stale or contains duplicate targets".into()); }
    }
    for c in changes { doc.records[c.index].edited.clone_from(&c.new); }
    Ok(())
}

pub fn version_path(game: Game, path: &Path) -> bool {
    let key = relative_key(path);
    match game {
        Game::Xenoblade1 => matches!(key.as_str(), "bdat_common_ms/bdat_common_ms/MNU_title_ms.json" | "bdat_common_ms/MNU_title_ms.json"),
        Game::Xenoblade2 => matches!(key.as_str(), "common_ms/common_ms/menu_ms.json" | "common_ms/menu_ms.json"),
        Game::Xenoblade3 => matches!(key.as_str(), "game/menu/menu/6D15742F.json" | "game/menu/6D15742F.json"),
        Game::XenobladeX => matches!(key.as_str(), "common_ms/common_ms/menu_program_ms.json" | "common_ms/menu_program_ms.json"),
    }
}

fn version_rows(game: Game) -> &'static [(&'static str, Option<&'static str>)] {
    match game {
        Game::Xenoblade1 => &[("8", None)],
        Game::Xenoblade2 => &[("132", None)],
        Game::Xenoblade3 => &[("11", None)],
        Game::XenobladeX => &[("4399", Some("TitleScreen_Copyright")), ("5237", Some("menu_program_TitleLogo03"))],
    }
}

fn version_marker() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)\bunburger(?:\s+(?:patch|mod))?\s+v?(\d+(?:\.\d+)+)\b").unwrap())
}

pub fn current_version(doc: &Document, game: Game) -> Result<String, String> {
    if !version_path(game, &doc.relative) { return Err("Not a recognized version table".into()); }
    let mut version = None;
    for (key, label) in version_rows(game) {
        let record = doc.records.iter().find(|r| r.key == *key && label.is_none_or(|s| r.label == s)).ok_or_else(|| format!("Version record {key} missing or label differs"))?;
        let mut matches = version_marker().captures_iter(&record.edited);
        let value = matches.next().and_then(|c| c.get(1)).ok_or_else(|| format!("No existing UNBURGER version marker in ID {key}"))?.as_str().to_owned();
        if matches.next().is_some() { return Err(format!("Multiple version markers in ID {key}")); }
        if version.as_ref().is_some_and(|v| v != &value) { return Err("Version markers disagree; review manually".into()); }
        version = Some(value);
    }
    version.ok_or_else(|| "No configured version records".into())
}

pub fn preview_version(doc: &Document, game: Game, version: &str) -> Result<Vec<PasteChange>, String> {
    if !Regex::new(r"^\d+(?:\.\d+){1,3}$").unwrap().is_match(version) { return Err("Enter a numeric version, e.g. 0.5.0".into()); }
    current_version(doc, game)?;
    let mut changes = Vec::new();
    for (key, _) in version_rows(game) {
        let (index, record) = doc.records.iter().enumerate().find(|(_, r)| r.key == *key).ok_or("Missing version record")?;
        let marker = version_marker().captures(&record.edited).and_then(|c| c.get(1)).ok_or("Missing version marker")?;
        let mut new = record.edited.clone();
        new.replace_range(marker.range(), version);
        if new != record.edited { changes.push(PasteChange { index, key: record.key.clone(), old: record.edited.clone(), new }); }
    }
    Ok(changes)
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Rule {
    pub patterns: String,
    pub max_chars: Option<usize>,
    pub max_lines: Option<usize>,
}

pub fn defaults(game: Game) -> Vec<Rule> {
    let rule = |patterns: &str, max_chars, max_lines| Rule { patterns: patterns.into(), max_chars, max_lines };
    match game {
        Game::Xenoblade2 => vec![rule("bf*", Some(54), None), rule("campfev*|fev*|kizuna*|qst*|tlk*", Some(39), None)],
        Game::Xenoblade3 => vec![rule("msg_ev*|msg_cq*", Some(70), Some(2)), rule("msg_ask*|msg_fev*|msg_nq*|msg_sq*|msg_tlk*|msg_tq*", Some(45), Some(3))],
        _ => Vec::new(),
    }
}

impl Preferences {
    pub fn rules(&self, game: Game) -> Vec<Rule> { self.rules.clone().unwrap_or_else(|| defaults(game)) }
}

pub fn text_field(game: Game) -> &'static str {
    if game == Game::Xenoblade3 { "<DBAF43F0>" } else { "name" }
}

pub fn scan(root: &Path, cancelled: &AtomicBool) -> Result<Vec<PathBuf>, String> {
    if !root.is_absolute() || !root.is_dir() { return Err("Pack Source must be an existing absolute directory".into()); }
    let mut pending = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(dir) = pending.pop() {
        if cancelled.load(Ordering::Relaxed) { return Err("Scan cancelled".into()); }
        for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
            if cancelled.load(Ordering::Relaxed) { return Err("Scan cancelled".into()); }
            let entry = entry.map_err(|e| e.to_string())?;
            let ty = entry.file_type().map_err(|e| e.to_string())?;
            let path = entry.path();
            if ty.is_dir() && !entry.file_name().to_string_lossy().starts_with('.') { pending.push(path); }
            else if ty.is_file() && path.extension().is_some_and(|s| s.eq_ignore_ascii_case("json")) {
                files.push(path.strip_prefix(root).map_err(|e| e.to_string())?.to_path_buf());
            }
        }
    }
    files.sort();
    Ok(files)
}

fn checked_file(root: &Path, relative: &Path) -> Result<PathBuf, String> {
    if !root.is_absolute() || relative.components().any(|p| !matches!(p, std::path::Component::Normal(_))) {
        return Err("Invalid source path".into());
    }
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let path = root.join(relative);
    let canonical = path.canonicalize().map_err(|e| e.to_string())?;
    if !canonical.starts_with(&root) || canonical != path || !canonical.is_file() {
        return Err("File is outside the source directory or uses a symbolic link".into());
    }
    Ok(canonical)
}

fn rows(data: &Value) -> Result<&Vec<Value>, String> {
    data.get("rows").and_then(Value::as_array).ok_or_else(|| "No BDAT rows array".into())
}

fn id(row: &Value) -> Option<String> {
    let v = row.get("$id")?;
    if v.is_string() || v.is_i64() || v.is_u64() { Some(v.to_string()) } else { None }
}

fn check_ids(data: &Value) -> Result<(), String> {
    let mut seen = HashSet::new();
    for row in rows(data)? {
        if let Some(key) = id(row) {
            if !seen.insert(key) { return Err("Duplicate $id; editing disabled for this file".into()); }
        }
    }
    Ok(())
}

#[derive(Debug)]
pub struct Record {
    pub key: String,
    pub label: String,
    pub original: Option<String>,
    pub saved: String,
    pub edited: String,
}

pub struct Document {
    path: PathBuf,
    bytes: Vec<u8>,
    data: Value,
    field: &'static str,
    pub relative: PathBuf,
    pub records: Vec<Record>,
    pub reference_status: String,
}

impl Document {
    pub fn open(root: &Path, relative: &Path, japanese: &str, game: Game) -> Result<Self, String> {
        let path = checked_file(root, relative)?;
        if !japanese.trim().is_empty() {
            if let Ok(reference_root) = Path::new(japanese).canonicalize() {
                if path.starts_with(reference_root) { return Err("The selected file belongs to the Japanese reference directory".into()); }
            }
        }
        let bytes = fs::read(&path).map_err(|e| e.to_string())?;
        let data: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        check_ids(&data)?;
        let field = text_field(game);
        let mut original = HashMap::new();
        let reference_status = if japanese.trim().is_empty() { "Japanese reference not configured".into() } else {
            let mut load = || -> Result<(), String> {
                let jp_path = checked_file(Path::new(japanese), relative)?;
                if jp_path == path { return Err("Reference is the translation file".into()); }
                let jp: Value = serde_json::from_slice(&fs::read(jp_path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
                check_ids(&jp)?;
                for row in rows(&jp)? {
                    if let (Some(k), Some(text)) = (id(row), row.get(field).and_then(Value::as_str)) { original.insert(k, text.to_owned()); }
                }
                Ok(())
            };
            match load() { Ok(()) => "Japanese reference matched by path and $id".into(), Err(e) => format!("Reference unavailable: {e}") }
        };
        let mut records = Vec::new();
        for row in rows(&data)? {
            if let (Some(key), Some(text)) = (id(row), row.get(field).and_then(Value::as_str)) {
                records.push(Record { label: row.get("label").and_then(Value::as_str).unwrap_or("").into(), original: original.remove(&key), key, saved: text.into(), edited: text.into() });
            }
        }
        Ok(Self { path, bytes, data, field, relative: relative.into(), records, reference_status })
    }

    pub fn dirty(&self) -> bool { self.records.iter().any(|r| r.saved != r.edited) }

    pub fn save(&mut self) -> Result<usize, String> {
        let changes: HashMap<_, _> = self.records.iter().filter(|r| r.saved != r.edited).map(|r| (r.key.clone(), r.edited.clone())).collect();
        if changes.is_empty() { return Ok(0); }
        if self.path.canonicalize().map_err(|e| e.to_string())? != self.path || fs::read(&self.path).map_err(|e| e.to_string())? != self.bytes {
            return Err("File changed outside Studio. Save refused; reload before editing again.".into());
        }
        let mut updated = self.data.clone();
        let mut count = 0;
        for row in updated["rows"].as_array_mut().ok_or("Missing rows")? {
            if let Some(new) = id(row).and_then(|k| changes.get(&k)) {
                let value = row.get_mut(self.field).filter(|v| v.is_string()).ok_or("Text field changed")?;
                *value = Value::String(new.clone());
                count += 1;
            }
        }
        if count != changes.len() { return Err("Not all edited IDs matched; save refused".into()); }
        let mut bytes = serde_json::to_vec_pretty(&updated).map_err(|e| e.to_string())?;
        if self.bytes.ends_with(b"\n") { bytes.push(b'\n'); }
        let mut tmp = tempfile::NamedTempFile::new_in(self.path.parent().ok_or("No file parent")?).map_err(|e| e.to_string())?;
        tmp.as_file().set_permissions(fs::metadata(&self.path).map_err(|e| e.to_string())?.permissions()).map_err(|e| e.to_string())?;
        tmp.write_all(&bytes).map_err(|e| e.to_string())?;
        tmp.as_file().sync_all().map_err(|e| e.to_string())?;
        if fs::read(&self.path).map_err(|e| e.to_string())? != self.bytes { return Err("File changed during save".into()); }
        tmp.persist(&self.path).map_err(|e| e.error.to_string())?;
        self.data = updated;
        self.bytes = bytes;
        for record in &mut self.records { record.saved.clone_from(&record.edited); }
        Ok(count)
    }
}

pub fn matching_rule<'a>(rules: &'a [Rule], path: &Path) -> Option<&'a Rule> {
    rules.iter().find(|rule| rule.patterns.trim().is_empty() || rule.patterns.split('|').any(|pattern| {
        let p = pattern.trim().to_lowercase();
        if p.is_empty() { return false; }
        path.components().any(|c| {
            let name = c.as_os_str().to_string_lossy().to_lowercase();
            let stem = name.strip_suffix(".json").unwrap_or(&name);
            if let Some(prefix) = p.strip_suffix('*') { stem.starts_with(prefix) } else { name == p || stem == p }
        })
    }))
}

#[derive(Debug)]
pub struct Validation { pub counts: Vec<usize>, pub too_long: bool, pub too_many: bool }

fn tags() -> &'static Regex {
    static TAGS: OnceLock<Regex> = OnceLock::new();
    TAGS.get_or_init(|| Regex::new(r"\[[^\]\r\n]*\]").unwrap())
}

pub fn highlighted_parts(text: &str) -> Vec<(&str, bool)> {
    let mut result = Vec::new();
    let mut last = 0;
    for m in tags().find_iter(text) {
        if last < m.start() { result.push((&text[last..m.start()], false)); }
        result.push((m.as_str(), true));
        last = m.end();
    }
    if last < text.len() { result.push((&text[last..], false)); }
    result
}

pub fn validate(text: &str, rule: Option<&Rule>) -> Validation {
    let normalized = text.replace("\\n", "\n").replace("\r\n", "\n");
    let counts: Vec<_> = normalized.split('\n').map(|line| tags().replace_all(line, "").chars().filter(|&c| c != '\u{200b}').count()).collect();
    Validation { too_long: rule.and_then(|r| r.max_chars).is_some_and(|n| counts.iter().any(|&c| c > n)), too_many: rule.and_then(|r| r.max_lines).is_some_and(|n| counts.len() > n), counts }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validate_default_rules_and_tags() {
        let rules = defaults(Game::Xenoblade2);
        let rule = matching_rule(&rules, Path::new("qst/qst_01.json"));
        assert_eq!(validate("[ML:foo ]abc\\ndef", rule).counts, vec![3,3]);
        assert!(validate(&"a".repeat(40), rule).too_long);
        assert!(!validate(&"a".repeat(40), None).too_long);
    }
    #[test]
    fn save_only_explicit_string_and_reject_external_changes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let file = root.join("test.json");
        let initial = br#"{"rows":[{"$id":1,"name":"abc","style":82},{"$id":2,"name":"def"},{"$id":3,"style":7}],"extra":true}"#;
        fs::write(&file, initial).unwrap();
        let mut doc = Document::open(&root, Path::new("test.json"), "", Game::Xenoblade2).unwrap();
        assert_eq!(doc.records.len(), 2);
        assert_eq!(doc.save().unwrap(), 0);
        assert_eq!(fs::read(&file).unwrap(), initial);
        doc.records[0].edited = "new\nline".into();
        assert_eq!(doc.save().unwrap(), 1);
        let result: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        assert_eq!(result["rows"][0]["name"], "new\nline");
        assert_eq!(result["rows"][0]["style"], 82);
        assert_eq!(result["rows"][1]["name"], "def");
        doc.records[0].edited = "other".into();
        fs::write(&file, initial).unwrap();
        assert!(doc.save().is_err());
        assert_eq!(fs::read(&file).unwrap(), initial);
    }

    #[test]
    fn reference_matches_typed_ids_not_indices() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let jp = root.join("jp");
        fs::create_dir(&jp).unwrap();
        fs::write(root.join("table.json"), r#"{"rows":[{"$id":1,"name":"a"},{"$id":2,"name":"b"}]}"#).unwrap();
        fs::write(jp.join("table.json"), r#"{"rows":[{"$id":2,"name":"二"},{"$id":1,"name":"一"}]}"#).unwrap();
        let doc = Document::open(&root, Path::new("table.json"), jp.to_str().unwrap(), Game::XenobladeX).unwrap();
        assert_eq!(doc.records[0].original.as_deref(), Some("一"));
        assert_eq!(doc.records[1].original.as_deref(), Some("二"));
        fs::write(root.join("table.json"), r#"{"rows":[{"$id":1,"name":"a"},{"$id":1,"name":"b"}]}"#).unwrap();
        assert!(Document::open(&root, Path::new("table.json"), "", Game::XenobladeX).is_err());
    }

    #[test]
    fn audit_exceptions_are_scoped_and_expire() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let root_str = root.to_str().unwrap();
        let path = Path::new("qst_01.json");
        fs::write(root.join(path), r#"{"rows":[{"$id":1,"name":"abcdefgh"},{"$id":2,"name":"abc"}]}"#).unwrap();
        let mut prefs = Preferences::default();
        let rules = vec![Rule { patterns: "qst*".into(), max_chars: Some(5), max_lines: None }];
        let mut doc = Document::open(&root, path, "", Game::Xenoblade2).unwrap();
        assert_eq!(audit_document(&doc, root_str, &rules, &[]).warnings, 1);
        assert!(!audit_document(&doc, root_str, &[], &[]).covered);
        prefs.exceptions.push(WarningException::new(root_str, path, &doc.records[0], &rules[0]));
        let encoded = toml::to_string(&prefs).unwrap();
        let prefs: Preferences = toml::from_str(&encoded).unwrap();
        let result = audit_file(root_str, path, Game::Xenoblade2, &prefs, &rules);
        assert_eq!((result.records, result.warnings, result.ignored), (2, 0, 1));
        assert_eq!(audit_document(&doc, "other", &rules, &prefs.exceptions).warnings, 1);
        let changed = vec![Rule { max_chars: Some(4), ..rules[0].clone() }];
        assert_eq!(audit_document(&doc, root_str, &changed, &prefs.exceptions).warnings, 1);
        doc.records[0].edited.push('!');
        assert_eq!(audit_document(&doc, root_str, &rules, &prefs.exceptions).warnings, 1);
        let jp = root.join("jp");
        fs::create_dir(&jp).unwrap();
        fs::write(jp.join(path), "not JSON").unwrap();
        let prefs = Preferences { japanese_reference: jp.to_string_lossy().into_owned(), ..prefs };
        assert!(audit_file(root_str, &Path::new("jp").join(path), Game::Xenoblade2, &prefs, &rules).reference);
    }

    #[test]
    fn browser_flattens_singletons_keeps_real_folders_and_filters() {
        use std::collections::BTreeMap;
        let files: Vec<PathBuf> = ["bf/bf/only.json", "other/a.json", "other/b.json", "root.json"].into_iter().map(PathBuf::from).collect();
        let mut audits = BTreeMap::new();
        audits.insert(files[0].clone(), FileAudit { covered: true, ..FileAudit::default() });
        audits.insert(files[1].clone(), FileAudit { covered: true, warnings: 1, long_lines: 1, ..FileAudit::default() });
        audits.insert(files[2].clone(), FileAudit { covered: true, warnings: 1, many_lines: 1, ..FileAudit::default() });
        let mut expanded = HashSet::new();
        let (rows, count) = browser_rows(&files, &audits, &expanded, "", BrowserFilter::All);
        assert_eq!(count, 4);
        assert_eq!(rows.len(), 3);
        assert!(rows.iter().any(|r| !r.folder && r.label == "bf/bf/only.json" && r.status == BrowserStatus::Clear));
        assert!(rows.iter().any(|r| r.folder && r.path == Path::new("other") && r.status == BrowserStatus::Error));
        expanded.insert(PathBuf::from("other"));
        let (rows, _) = browser_rows(&files, &audits, &expanded, "", BrowserFilter::All);
        assert_eq!(rows.len(), 5);
        let (pending, count) = browser_rows(&files, &audits, &expanded, "", BrowserFilter::Pending);
        assert_eq!(count, 1);
        assert_eq!(pending[0].label, "root.json");
        let (orange, count) = browser_rows(&files, &audits, &expanded, "", BrowserFilter::Lines);
        assert_eq!(count, 1);
        assert_eq!(orange[0].label, "other/b.json");
        assert_eq!(orange[0].status, BrowserStatus::Warning);
        let (search, count) = browser_rows(&files, &audits, &expanded, "ONLY", BrowserFilter::All);
        assert_eq!(count, 1);
        assert_eq!(search[0].path, files[0]);
    }

    #[test]
    fn virtual_window_covers_large_lists_and_clamps_after_filter() {
        assert_eq!(visible_window(0, 1000.0, 600.0, 30.0), 0..0);
        assert_eq!(visible_window(4, 500000.0, 600.0, 30.0), 0..4);
        let top = visible_window(4000, 0.0, 600.0, 30.0);
        assert_eq!(top, 0..32);
        let mid = visible_window(4000, 60000.0, 600.0, 30.0);
        assert_eq!(mid, 1988..2032);
        let end = visible_window(4000, 120000.0, 600.0, 30.0);
        assert_eq!(end, 3968..4000);
    }

    #[test]
    fn paste_preview_is_atomic_and_checks_typed_ids() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let path = Path::new("table.json");
        fs::write(root.join(path), r#"{"rows":[{"$id":1,"name":"a"},{"$id":"1","name":"b"}]}"#).unwrap();
        let mut doc = Document::open(&root, path, "", Game::Xenoblade2).unwrap();
        for bad in ["[1]: x\n[1]: y", "[1]: x\n[9]: y", "[1]: x\nmalformed", "[1]: \"unterminated"] {
            assert!(preview_paste(&doc, bad).is_err());
            assert!(!doc.dirty());
        }
        let changes = preview_paste(&doc, "[1]: plain\n[\"1\"]: \"two\\nlines\"").unwrap();
        assert_eq!(changes.len(), 2);
        assert!(!doc.dirty());
        doc.records[1].edited = "intervening edit".into();
        assert!(stage_changes(&mut doc, &changes).is_err());
        assert_eq!(doc.records[0].edited, "a");
        doc.records[1].edited = "b".into();
        stage_changes(&mut doc, &changes).unwrap();
        assert_eq!(doc.records[0].edited, "plain");
        assert_eq!(doc.records[1].edited, "two\nlines");
        assert_eq!(doc.save().unwrap(), 2);
    }

    #[test]
    fn version_requires_all_known_records_and_preserves_other_text() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let path = Path::new("common_ms/menu_program_ms.json");
        fs::create_dir(root.join("common_ms")).unwrap();
        fs::write(root.join(path), r#"{"rows":[{"$id":4399,"label":"TitleScreen_Copyright","name":"  Unburger Patch 0.5.0 [ML:x ]"},{"$id":5237,"label":"menu_program_TitleLogo03","name":"UNBURGER Mod 0.5.0"},{"$id":9,"name":"Unburger Patch 0.5.0"}]}"#).unwrap();
        let mut doc = Document::open(&root, path, "", Game::XenobladeX).unwrap();
        assert_eq!(current_version(&doc, Game::XenobladeX).unwrap(), "0.5.0");
        assert!(preview_version(&doc, Game::Xenoblade1, "0.6.0").is_err());
        assert!(preview_version(&doc, Game::XenobladeX, "oops").is_err());
        let changes = preview_version(&doc, Game::XenobladeX, "0.6.0").unwrap();
        assert_eq!(changes.len(), 2);
        assert!(!doc.dirty());
        stage_changes(&mut doc, &changes).unwrap();
        assert_eq!(doc.records[0].edited, "  Unburger Patch 0.6.0 [ML:x ]");
        assert_eq!(doc.records[2].edited, "Unburger Patch 0.5.0");
        doc.records[1].edited = "UNBURGER Mod 0.5.0".into();
        assert!(preview_version(&doc, Game::XenobladeX, "0.7.0").is_err());
        doc.records.pop();
        doc.records.pop();
        assert!(current_version(&doc, Game::XenobladeX).is_err());
    }
}
