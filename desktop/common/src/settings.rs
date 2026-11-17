//! Persisted UI preferences.
//!
//! The desktop window loads from a random loopback port on every launch, so
//! origin-scoped `localStorage` cannot be the source of truth. Settings live in
//! `settings.json` next to `targets.json`; the frontend keeps a cookie copy for
//! the first paint only.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

pub const SETTINGS_FILENAME: &str = "settings.json";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeChoice {
    Auto,
    /// Shipped default: a light UI reads better for a file browser, and the
    /// dark palette stays one click away from the 视图 menu.
    #[default]
    Light,
    Dark,
}

impl ThemeChoice {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}

/// How the listing is laid out. Explorer-style choices, kept next to the other
/// preferences so the layout survives a restart even though the window loads
/// from a fresh random port on every launch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewMode {
    #[default]
    List,
    Grid,
    Details,
}

impl ViewMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Grid => "grid",
            Self::Details => "details",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppSettings {
    #[serde(default)]
    pub theme: ThemeChoice,
    #[serde(default)]
    pub show_hidden: bool,
    #[serde(default)]
    pub view_mode: ViewMode,
    /// Storage analysis is an opt-in extension: the toolbar entry only appears
    /// once the user turns it on from the 视图 menu.
    #[serde(default)]
    pub analyze_enabled: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::default(),
            show_hidden: false,
            view_mode: ViewMode::default(),
            analyze_enabled: false,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct SettingsPatch {
    #[serde(default)]
    pub theme: Option<ThemeChoice>,
    #[serde(default)]
    pub show_hidden: Option<bool>,
    #[serde(default)]
    pub view_mode: Option<ViewMode>,
    #[serde(default)]
    pub analyze_enabled: Option<bool>,
}

impl AppSettings {
    pub fn apply(&mut self, patch: SettingsPatch) {
        if let Some(theme) = patch.theme {
            self.theme = theme;
        }
        if let Some(show_hidden) = patch.show_hidden {
            self.show_hidden = show_hidden;
        }
        if let Some(view_mode) = patch.view_mode {
            self.view_mode = view_mode;
        }
        if let Some(analyze_enabled) = patch.analyze_enabled {
            self.analyze_enabled = analyze_enabled;
        }
    }
}

pub fn settings_path(dir: &Path) -> PathBuf {
    dir.join(SETTINGS_FILENAME)
}

pub fn load(dir: &Path) -> AppSettings {
    let path = settings_path(dir);
    let Ok(raw) = fs::read(&path) else {
        return AppSettings::default();
    };
    match serde_json::from_slice(&raw) {
        Ok(settings) => settings,
        Err(err) => {
            let backup = path.with_extension(format!(
                "json.corrupt-{}",
                crate::known_hosts::now_seconds()
            ));
            let _ = fs::rename(&path, &backup);
            rockbadger::error::log(&format!(
                "settings.json 解析失败，已备份为 {}: {err}",
                backup.display()
            ));
            AppSettings::default()
        }
    }
}

pub fn save(dir: &Path, settings: &AppSettings) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    let path = settings_path(dir);
    let raw = serde_json::to_vec_pretty(settings)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, raw)?;
    fs::rename(&tmp, &path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_use_light_theme_and_hide_dotfiles() {
        let settings = AppSettings::default();
        assert_eq!(settings.theme, ThemeChoice::Light);
        assert!(!settings.show_hidden);
        assert_eq!(settings.view_mode, ViewMode::List);
        // The analysis extension stays off until the user asks for it.
        assert!(!settings.analyze_enabled);
    }

    #[test]
    fn layout_and_extension_survive_a_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = AppSettings::default();
        settings.apply(SettingsPatch {
            theme: None,
            show_hidden: None,
            view_mode: Some(ViewMode::Grid),
            analyze_enabled: Some(true),
        });
        save(dir.path(), &settings).unwrap();
        let loaded = load(dir.path());
        assert_eq!(loaded.view_mode, ViewMode::Grid);
        assert!(loaded.analyze_enabled);
    }

    #[test]
    fn roundtrips_and_merges_patches() {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = AppSettings::default();
        settings.apply(SettingsPatch {
            theme: Some(ThemeChoice::Light),
            show_hidden: None,
            view_mode: None,
            analyze_enabled: None,
        });
        save(dir.path(), &settings).unwrap();
        assert_eq!(load(dir.path()).theme, ThemeChoice::Light);

        settings.apply(SettingsPatch {
            theme: None,
            show_hidden: Some(true),
            view_mode: None,
            analyze_enabled: None,
        });
        save(dir.path(), &settings).unwrap();
        let loaded = load(dir.path());
        assert_eq!(loaded.theme, ThemeChoice::Light);
        assert!(loaded.show_hidden);
    }

    #[test]
    fn corrupt_file_is_backed_up_and_defaults_are_used() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(settings_path(dir.path()), b"{not json").unwrap();
        let settings = load(dir.path());
        assert_eq!(settings.theme, ThemeChoice::Light);
        let backups = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().contains(".corrupt-"))
            .count();
        assert_eq!(backups, 1);
    }
}
