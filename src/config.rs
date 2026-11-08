//! App settings, persisted as TOML at `~/.config/tabster.toml` (override with
//! `TABSTER_CONFIG`). Every field has a default so old files keep loading.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};
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

/// Writes the config on a background thread once changes settle (the newest config wins), so a
/// slider dragged across its range is one write, not hundreds. Dropping it writes anything
/// still pending.
pub struct ConfigSaver {
    shared: Arc<SaverShared>,
    thread: Option<JoinHandle<()>>,
}

struct SaverShared {
    path: PathBuf,
    debounce: Duration,
    state: Mutex<SaverState>,
    wake: Condvar,
    /// Held for the length of a write, so the thread and `flush` never write at once.
    writing: Mutex<()>,
}

#[derive(Default)]
struct SaverState {
    /// The newest config not yet written, and when it arrived.
    pending: Option<(Config, Instant)>,
    shutdown: bool,
}

impl ConfigSaver {
    pub fn new(path: PathBuf, debounce: Duration) -> Self {
        let shared = Arc::new(SaverShared {
            path,
            debounce,
            state: Mutex::new(SaverState::default()),
            wake: Condvar::new(),
            writing: Mutex::new(()),
        });
        let thread_shared = Arc::clone(&shared);
        let thread = thread::Builder::new()
            .name("tabster-config-saver".into())
            .spawn(move || run_saver(&thread_shared))
            .inspect_err(|e| log::error!("couldn't start the config saver: {e}"))
            .ok();
        Self { shared, thread }
    }

    pub fn path(&self) -> &Path {
        &self.shared.path
    }

    /// Queue `config` to be written once changes settle.
    pub fn save(&self, config: Config) {
        let mut state = self.shared.state.lock();
        state.pending = Some((config, Instant::now()));
        self.shared.wake.notify_one();
    }

    /// Write any pending config now.
    pub fn flush(&self) -> std::io::Result<()> {
        self.shared.write_pending()
    }
}

impl Drop for ConfigSaver {
    fn drop(&mut self) {
        if let Err(e) = self.flush() {
            log::error!("couldn't save the config: {e}");
        }
        self.shared.state.lock().shutdown = true;
        self.shared.wake.notify_one();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl SaverShared {
    fn write_pending(&self) -> std::io::Result<()> {
        let _writing = self.writing.lock();
        let Some((config, _)) = self.state.lock().pending.take() else {
            return Ok(());
        };
        save(&self.path, &config)
    }
}

fn run_saver(shared: &SaverShared) {
    loop {
        {
            let mut state = shared.state.lock();
            loop {
                if state.shutdown {
                    return;
                }
                match &state.pending {
                    None => shared.wake.wait(&mut state),
                    Some((_, changed)) => {
                        let quiet = changed.elapsed();
                        if quiet >= shared.debounce {
                            break;
                        }
                        let wait = shared.debounce - quiet;
                        shared.wake.wait_for(&mut state, wait);
                    }
                }
            }
        }
        if let Err(e) = shared.write_pending() {
            log::error!("couldn't save the config to {}: {e}", shared.path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scale(scale: f32) -> Config {
        Config {
            scale,
            ..Default::default()
        }
    }

    #[test]
    fn saver_writes_once_changes_settle() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tabster.toml");
        let saver = ConfigSaver::new(path.clone(), Duration::from_millis(50));
        saver.save(scale(1.5));
        saver.save(scale(2.0));
        assert!(!path.exists());
        let deadline = Instant::now() + Duration::from_secs(5);
        while !path.exists() {
            assert!(Instant::now() < deadline, "the config was never written");
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(load(&path), scale(2.0));
    }

    #[test]
    fn saver_flushes_on_request_and_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tabster.toml");
        let saver = ConfigSaver::new(path.clone(), Duration::from_secs(3600));
        saver.save(scale(1.5));
        saver.flush().unwrap();
        assert_eq!(load(&path), scale(1.5));
        saver.save(scale(2.0));
        drop(saver);
        assert_eq!(load(&path), scale(2.0));
    }

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
