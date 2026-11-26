use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::PathBuf;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Paths {
    pub pack_source: String,
    pub pack_destination: String,
    pub extract_source: String,
    pub extract_destination: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Game {
    Xenoblade1,
    #[default]
    Xenoblade2,
    Xenoblade3,
    XenobladeX,
}

impl Game {
    pub const ALL: [Self; 4] = [Self::Xenoblade1, Self::Xenoblade2, Self::Xenoblade3, Self::XenobladeX];

    pub fn label(self) -> &'static str {
        match self {
            Self::Xenoblade1 => "Xenoblade 1",
            Self::Xenoblade2 => "Xenoblade 2",
            Self::Xenoblade3 => "Xenoblade 3",
            Self::XenobladeX => "Xenoblade X",
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub last_switch2: bool,
    pub switch1: Paths,
    pub switch2: Paths,
    pub translation_sw1: crate::translation::Preferences,
    pub translation_sw2: crate::translation::Preferences,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Profiles {
    pub xenoblade1: Profile,
    pub xenoblade2: Profile,
    pub xenoblade3: Profile,
    pub xenoblade_x: Profile,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub wizard_completed: bool,
    pub toolset_path: String,
    pub active_game: Game,
    #[serde(rename = "xcx_common_parent", skip_serializing_if = "Option::is_none")]
    legacy_xcx_common_parent: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    xcx_common_parent_sw1: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    xcx_common_parent_sw2: Option<bool>,
    pub profiles: Profiles,
    // Older settings had unqualified platform paths. Preserve them until the
    // user explicitly assigns each platform to a game.
    #[serde(alias = "switch1", skip_serializing_if = "Option::is_none")]
    legacy_switch1: Option<Paths>,
    #[serde(alias = "switch2", skip_serializing_if = "Option::is_none")]
    legacy_switch2: Option<Paths>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            wizard_completed: false,
            toolset_path: String::new(),
            active_game: Game::default(),
            legacy_xcx_common_parent: None,
            xcx_common_parent_sw1: None,
            xcx_common_parent_sw2: None,
            profiles: Profiles::default(),
            legacy_switch1: None,
            legacy_switch2: None,
        }
    }
}

impl Settings {
    fn profile(&self) -> &Profile {
        match self.active_game {
            Game::Xenoblade1 => &self.profiles.xenoblade1,
            Game::Xenoblade2 => &self.profiles.xenoblade2,
            Game::Xenoblade3 => &self.profiles.xenoblade3,
            Game::XenobladeX => &self.profiles.xenoblade_x,
        }
    }

    fn profile_mut(&mut self) -> &mut Profile {
        match self.active_game {
            Game::Xenoblade1 => &mut self.profiles.xenoblade1,
            Game::Xenoblade2 => &mut self.profiles.xenoblade2,
            Game::Xenoblade3 => &mut self.profiles.xenoblade3,
            Game::XenobladeX => &mut self.profiles.xenoblade_x,
        }
    }

    pub fn last_switch2(&self) -> bool { self.profile().last_switch2 }

    pub fn set_platform(&mut self, switch2: bool) { self.profile_mut().last_switch2 = switch2; }

    pub fn xcx_common_parent(&self, switch2: bool) -> bool {
        let platform = if switch2 { self.xcx_common_parent_sw2 } else { self.xcx_common_parent_sw1 };
        platform.or(self.legacy_xcx_common_parent).unwrap_or(true)
    }

    fn migrate_xcx_common_parent(&mut self) {
        self.xcx_common_parent_sw1 = Some(self.xcx_common_parent(false));
        self.xcx_common_parent_sw2 = Some(self.xcx_common_parent(true));
        self.legacy_xcx_common_parent = None;
    }

    pub fn set_xcx_common_parent(&mut self, switch2: bool, enabled: bool) {
        self.migrate_xcx_common_parent();
        if switch2 {
            self.xcx_common_parent_sw2 = Some(enabled);
        } else {
            self.xcx_common_parent_sw1 = Some(enabled);
        }
    }

    pub fn current(&self, switch2: bool) -> &Paths {
        let profile = self.profile();
        if switch2 { &profile.switch2 } else { &profile.switch1 }
    }

    pub fn current_mut(&mut self, switch2: bool) -> &mut Paths {
        let profile = self.profile_mut();
        if switch2 { &mut profile.switch2 } else { &mut profile.switch1 }
    }

    pub fn translation(&self, switch2: bool) -> &crate::translation::Preferences {
        let p = self.profile();
        if switch2 { &p.translation_sw2 } else { &p.translation_sw1 }
    }

    pub fn translation_mut(&mut self, switch2: bool) -> &mut crate::translation::Preferences {
        let p = self.profile_mut();
        if switch2 { &mut p.translation_sw2 } else { &mut p.translation_sw1 }
    }

    pub fn pending_legacy(&self) -> Option<(bool, &Paths)> {
        for (platform, paths) in [(false, &self.legacy_switch1), (true, &self.legacy_switch2)] {
            if let Some(paths) = paths.as_ref().filter(|p| *p != &Paths::default()) {
                return Some((platform, paths));
            }
        }
        None
    }

    pub fn import_legacy(&mut self, switch2: bool) -> Result<(), &'static str> {
        if self.current(switch2) != &Paths::default() {
            return Err("This game/platform already has paths. Choose an empty profile.");
        }
        let paths = if switch2 { self.legacy_switch2.take() } else { self.legacy_switch1.take() };
        let paths = paths.ok_or("No previous paths for this platform")?;
        *self.current_mut(switch2) = paths;
        self.set_platform(switch2);
        Ok(())
    }

    pub fn load() -> Self {
        let mut settings: Self = config_file().ok()
            .and_then(|path| fs::read_to_string(path).ok())
            .and_then(|data| toml::from_str(&data).ok())
            .unwrap_or_default();
        settings.migrate_xcx_common_parent();
        settings
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(tag: &str) -> Paths {
        Paths {
            pack_source: format!("{tag}/pack/input"),
            pack_destination: format!("{tag}/pack/output"),
            extract_source: format!("{tag}/extract/input"),
            extract_destination: format!("{tag}/extract/output"),
        }
    }

    #[test]
    fn eight_profiles_round_trip_without_cross_talk() {
        let mut settings = Settings::default();
        for (index, game) in Game::ALL.into_iter().enumerate() {
            settings.active_game = game;
            for platform in [false, true] {
                *settings.current_mut(platform) = sample(&format!("{index}/{platform}"));
            }
            settings.set_platform(index % 2 == 0);
        }
        let encoded = toml::to_string_pretty(&settings).unwrap();
        let mut decoded: Settings = toml::from_str(&encoded).unwrap();
        assert_eq!(decoded.active_game, Game::XenobladeX);
        assert!(decoded.pending_legacy().is_none());
        for (index, game) in Game::ALL.into_iter().enumerate() {
            decoded.active_game = game;
            for platform in [false, true] {
                assert_eq!(decoded.current(platform), &sample(&format!("{index}/{platform}")));
            }
            assert_eq!(decoded.last_switch2(), index % 2 == 0);
        }
    }

    #[test]
    fn old_paths_are_preserved_until_explicit_assignment() {
        let old = r#"
wizard_completed = true
toolset_path = "backend"
layout = "Auto"

[switch1]
pack_source = "old-sw1-input"
pack_destination = "old-sw1-output"
extract_source = "old-sw1-extract"
extract_destination = "old-sw1-json"

[switch2]
pack_source = "old-sw2-input"
pack_destination = "old-sw2-output"
"#;
        let mut settings: Settings = toml::from_str(old).unwrap();
        assert!(settings.wizard_completed);
        assert_eq!(settings.toolset_path, "backend");
        assert_eq!(settings.pending_legacy().unwrap().0, false);
        assert_eq!(settings.current(false), &Paths::default());
        settings.active_game = Game::Xenoblade3;
        settings.import_legacy(false).unwrap();
        assert_eq!(settings.current(false).pack_source, "old-sw1-input");
        assert_eq!(settings.current(false).extract_destination, "old-sw1-json");
        let serialized = toml::to_string_pretty(&settings).unwrap();
        let mut settings: Settings = toml::from_str(&serialized).unwrap();
        assert_eq!(settings.pending_legacy().unwrap().0, true);
        settings.active_game = Game::XenobladeX;
        settings.import_legacy(true).unwrap();
        assert_eq!(settings.current(true).pack_destination, "old-sw2-output");
        assert_eq!(settings.current(false), &Paths::default());
        assert!(settings.pending_legacy().is_none());
        assert!(settings.last_switch2());
    }

    #[test]
    fn migration_never_overwrites_existing_paths() {
        let mut settings: Settings = toml::from_str("[switch1]\npack_source = 'previous'").unwrap();
        *settings.current_mut(false) = sample("current");
        assert!(settings.import_legacy(false).is_err());
        assert_eq!(settings.current(false), &sample("current"));
        assert_eq!(settings.pending_legacy().unwrap().1.pack_source, "previous");
    }

    #[test]
    fn xcx_common_parent_defaults_on_and_persists_independently() {
        let mut settings: Settings = toml::from_str("wizard_completed = true").unwrap();
        assert!(settings.xcx_common_parent(false));
        assert!(settings.xcx_common_parent(true));
        settings.active_game = Game::XenobladeX;
        settings.set_xcx_common_parent(false, false);
        settings.set_platform(true);
        assert!(!settings.xcx_common_parent(false));
        assert!(settings.xcx_common_parent(true));
        let serialized = toml::to_string_pretty(&settings).unwrap();
        let mut decoded: Settings = toml::from_str(&serialized).unwrap();
        assert!(!decoded.xcx_common_parent(false));
        assert!(decoded.xcx_common_parent(true));
        assert!(decoded.last_switch2());
        decoded.set_xcx_common_parent(true, false);
        decoded.set_xcx_common_parent(false, true);
        assert!(decoded.xcx_common_parent(false));
        assert!(!decoded.xcx_common_parent(true));
    }

    #[test]
    fn xcx_common_parent_migrates_previous_shared_value_for_both_platforms() {
        for old_value in [true, false] {
            let mut settings: Settings = toml::from_str(&format!("xcx_common_parent = {old_value}")).unwrap();
            settings.migrate_xcx_common_parent();
            assert_eq!(settings.xcx_common_parent(false), old_value);
            assert_eq!(settings.xcx_common_parent(true), old_value);
            assert!(settings.legacy_xcx_common_parent.is_none());
            settings.set_xcx_common_parent(true, !old_value);
            let serialized = toml::to_string_pretty(&settings).unwrap();
            assert!(!serialized.lines().any(|line| line.starts_with("xcx_common_parent =")));
            let mut decoded: Settings = toml::from_str(&serialized).unwrap();
            decoded.migrate_xcx_common_parent();
            assert_eq!(decoded.xcx_common_parent(false), old_value);
            assert_eq!(decoded.xcx_common_parent(true), !old_value);
        }
    }

    #[test]
    fn xcx_common_parent_migration_keeps_explicit_platform_value() {
        let mut settings: Settings = toml::from_str("xcx_common_parent = false\nxcx_common_parent_sw2 = true").unwrap();
        settings.migrate_xcx_common_parent();
        assert!(!settings.xcx_common_parent(false));
        assert!(settings.xcx_common_parent(true));
    }
}
