use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::PathBuf;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Paths {
    pub pack_source: String,
    pub pack_destination: String,
    pub extract_source: String,
    pub extract_destination: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub wizard_completed: bool,
    pub toolset_path: String,
    pub switch1: Paths,
    pub switch2: Paths,
    pub layout: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            wizard_completed: false,
            toolset_path: String::new(),
            switch1: Paths::default(),
            switch2: Paths::default(),
            layout: "Auto".into(),
        }
    }
}

impl Settings {
    pub fn current(&self, switch2: bool) -> &Paths {
        if switch2 { &self.switch2 } else { &self.switch1 }
    }

    pub fn current_mut(&mut self, switch2: bool) -> &mut Paths {
        if switch2 { &mut self.switch2 } else { &mut self.switch1 }
    }

    pub fn load() -> Self {
        let Ok(path) = config_file() else { return Self::default(); };
        match fs::read_to_string(path) {
            Ok(data) => toml::from_str(&data).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) -> io::Result<()> {
        let path = config_file()?;
        let folder = path.parent().ok_or_else(|| io::Error::other("No config parent"))?;
        fs::create_dir_all(folder)?;
        let data = toml::to_string_pretty(self).map_err(io::Error::other)?;
        let mut tmp = tempfile::NamedTempFile::new_in(folder)?;
        use std::io::Write;
        tmp.write_all(data.as_bytes())?;
        tmp.as_file_mut().sync_all()?;
        // Windows cannot always rename over an existing file. Persist a newly written
        // configuration file only after the temp file is fully written.
        if cfg!(windows) && path.exists() {
            let backup = path.with_extension("toml.bak");
            if backup.exists() { fs::remove_file(&backup)?; }
            fs::rename(&path, &backup)?;
            match tmp.persist(&path) {
                Ok(_) => { fs::remove_file(backup)?; Ok(()) }
                Err(e) => {
                    match fs::rename(&backup, &path) {
                        Ok(()) => Err(e.error),
                        Err(restore) => Err(io::Error::other(format!(
                            "Configuration replacement failed ({}); rollback failed ({restore}); previous data remains in backup",
                            e.error
                        ))),
                    }
                }
            }
        } else {
            tmp.persist(&path).map(|_| ()).map_err(|e| e.error)
        }
    }
}

pub fn config_file() -> io::Result<PathBuf> {
    dirs::config_dir()
        .map(|p| p.join("xenoblade-bdat-studio").join("settings.toml"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "User configuration directory unavailable"))
}

pub fn log_directory() -> io::Result<PathBuf> {
    dirs::data_local_dir()
        .map(|p| p.join("xenoblade-bdat-studio").join("logs"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "User local data directory unavailable"))
}
