//! App settings, persisted as TOML at `~/.config/tabster.toml` (override with
//! `TABSTER_CONFIG`). Every field has a default so old files keep loading.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(default)]
pub struct Config {
    /// Layout zoom.
    pub scale: f32,
    pub show_rhythm: bool,
    /// Folders watched for new tab files to import automatically.
    pub watched_folders: Vec<String>,
    /// Keep the tab scrolled so the cursor stays in view.
    pub follow_cursor: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            scale: 1.0,
            show_rhythm: true,
            watched_folders: Vec::new(),
            follow_cursor: true,
        }
    }
}

pub fn default_path() -> PathBuf {
    if let Some(p) = std::env::var_os("TABSTER_CONFIG") {
        return PathBuf::from(p);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join(".config").join("tabster.toml")
}

/// Load the config; a missing or unreadable file gives the defaults.
pub fn load(path: &Path) -> Config {
    match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text).unwrap_or_else(|e| {
            log::warn!("config {} is invalid, using defaults: {e}", path.display());
            Config::default()
        }),
        Err(_) => Config::default(),
    }
}

/// Write atomically (temp file + rename).
pub fn save(path: &Path, config: &Config) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = toml::to_string_pretty(config).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.toml");
        assert_eq!(load(&path), Config::default());
        let c = Config {
            scale: 1.5,
            ..Default::default()
        };
        save(&path, &c).unwrap();
        assert_eq!(load(&path), c);
        std::fs::write(&path, "scale = 2.0\n").unwrap();
        assert_eq!(load(&path).scale, 2.0);
        assert!(load(&path).show_rhythm);
    }
}
