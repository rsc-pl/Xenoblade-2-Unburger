use std::env;
use std::path::{Path, PathBuf};

fn executable_names() -> &'static [&'static str] {
    if cfg!(windows) {
        &["bdat-toolset.exe", "bdat-toolset"]
    } else {
        &["bdat-toolset-linux", "bdat-toolset"]
    }
}

pub fn locate(configured: &str) -> Option<PathBuf> {
    if !configured.trim().is_empty() {
        let p = PathBuf::from(configured.trim());
        return p.is_file().then_some(p);
    }
    let mut directories = Vec::new();
    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            for ancestor in dir.ancestors().take(4) {
                directories.push(ancestor.to_path_buf());
            }
        }
    }
    if let Some(path) = env::var_os("PATH") {
        directories.extend(env::split_paths(&path));
    }
    for dir in directories {
        for name in executable_names() {
            let candidate = dir.join(name);
            if candidate.is_file() { return Some(candidate); }
        }
    }
    None
}

pub fn check_path(path: &Path) -> Result<(), String> {
    if !path.is_file() {
        return Err(format!("bdat-toolset is not a file: {}", path.display()));
    }
    // A full runtime check happens on first conversion, which captures its exit code.
    Ok(())
}
