//! Watched folders: report new or changed tab files (via FSEvents on macOS), debounced so a
//! file still being written or downloaded is reported once, after it settles.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use super::LibraryError;

/// How long a file must go without events before it's reported.
const DEBOUNCE: Duration = Duration::from_secs(1);

/// Extensions of the tab files worth reporting (compared case-insensitively).
const TAB_EXTENSIONS: &[&str] = &["gp", "gpx", "gp5", "gp4", "gp3"];

enum Message {
    Changed(PathBuf),
    Stop,
}

/// Watches folders recursively and calls back with batches of tab files that appeared or
/// changed. The callback runs on a dedicated thread; dropping the watcher stops it.
pub struct FolderWatcher {
    watcher: RecommendedWatcher,
    messages: Sender<Message>,
}

impl FolderWatcher {
    /// Watch `folders`. Files under `ignore` (the library's data directory) are never
    /// reported. `on_files` gets each batch of settled files, sorted.
    pub fn new<F>(folders: &[PathBuf], ignore: &Path, on_files: F) -> Result<Self, LibraryError>
    where
        F: Fn(Vec<PathBuf>) + Send + 'static,
    {
        Self::with_debounce(folders, ignore, DEBOUNCE, on_files)
    }

    fn with_debounce<F>(
        folders: &[PathBuf],
        ignore: &Path,
        debounce: Duration,
        on_files: F,
    ) -> Result<Self, LibraryError>
    where
        F: Fn(Vec<PathBuf>) + Send + 'static,
    {
        let (messages, receiver) = mpsc::channel();
        let sender = messages.clone();
        let watcher = notify::recommended_watcher(move |event: notify::Result<Event>| {
            let event = match event {
                Ok(event) => event,
                Err(e) => {
                    log::warn!("folder watcher: {e}");
                    return;
                }
            };
            if !matches!(
                event.kind,
                EventKind::Create(_) | EventKind::Modify(_) | EventKind::Any
            ) {
                return;
            }
            for path in event.paths {
                if is_tab_file(&path) {
                    let _ = sender.send(Message::Changed(path));
                }
            }
        })
        .map_err(watch_error)?;

        // FSEvents reports resolved paths (e.g. /private/var/… for /var/…): ignore both forms.
        let mut ignored = vec![ignore.to_path_buf()];
        if let Ok(canonical) = ignore.canonicalize() {
            ignored.push(canonical);
        }
        thread::Builder::new()
            .name("tabster-folder-watch".into())
            .spawn(move || debounce_loop(&receiver, &ignored, debounce, &on_files))?;

        let mut this = Self { watcher, messages };
        for folder in folders {
            this.watch(folder)?;
        }
        Ok(this)
    }

    /// Start watching another folder (recursively).
    pub fn watch(&mut self, folder: &Path) -> Result<(), LibraryError> {
        self.watcher
            .watch(folder, RecursiveMode::Recursive)
            .map_err(watch_error)
    }

    pub fn unwatch(&mut self, folder: &Path) -> Result<(), LibraryError> {
        self.watcher.unwatch(folder).map_err(watch_error)
    }
}

impl Drop for FolderWatcher {
    fn drop(&mut self) {
        let _ = self.messages.send(Message::Stop);
    }
}

/// Collect changed paths until each has been quiet for `debounce`, then report the ones that
/// still exist as files.
fn debounce_loop(
    receiver: &mpsc::Receiver<Message>,
    ignored: &[PathBuf],
    debounce: Duration,
    on_files: &dyn Fn(Vec<PathBuf>),
) {
    let tick = debounce / 4;
    let mut pending: HashMap<PathBuf, Instant> = HashMap::new();
    loop {
        let message = if pending.is_empty() {
            receiver.recv().map_err(|_| RecvTimeoutError::Disconnected)
        } else {
            receiver.recv_timeout(tick)
        };
        match message {
            Ok(Message::Changed(path)) => {
                if !ignored.iter().any(|dir| path.starts_with(dir)) {
                    pending.insert(path, Instant::now());
                }
            }
            Ok(Message::Stop) | Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {}
        }

        let now = Instant::now();
        let settled: Vec<PathBuf> = pending
            .iter()
            .filter(|(_, last)| now.duration_since(**last) >= debounce)
            .map(|(path, _)| path.clone())
            .collect();
        if settled.is_empty() {
            continue;
        }
        for path in &settled {
            pending.remove(path);
        }
        let mut files: Vec<PathBuf> = settled.into_iter().filter(|p| p.is_file()).collect();
        if !files.is_empty() {
            files.sort();
            on_files(files);
        }
    }
}

/// A visible file with a Guitar Pro extension (not `._foo.gp` resource forks or other dot
/// files, which editors and downloads leave behind).
fn is_tab_file(path: &Path) -> bool {
    let visible = path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| !n.starts_with('.'));
    let tab = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| TAB_EXTENSIONS.iter().any(|t| e.eq_ignore_ascii_case(t)));
    visible && tab
}

fn watch_error(error: notify::Error) -> LibraryError {
    LibraryError::Io(std::io::Error::other(format!("folder watcher: {error}")))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;

    use parking_lot::Mutex;

    use super::*;

    #[test]
    fn watcher_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<FolderWatcher>();
    }

    #[test]
    fn recognises_tab_files() {
        assert!(is_tab_file(Path::new("/a/song.gp")));
        assert!(is_tab_file(Path::new("/a/song.GP5")));
        assert!(is_tab_file(Path::new("/a/song.gpx")));
        assert!(is_tab_file(Path::new("/a/b/song.gp3")));
        assert!(!is_tab_file(Path::new("/a/song.txt")));
        assert!(!is_tab_file(Path::new("/a/song.gp.download")));
        assert!(!is_tab_file(Path::new("/a/._song.gp")));
        assert!(!is_tab_file(Path::new("/a/gp")));
    }

    #[test]
    fn debounces_and_filters() {
        let (sender, receiver) = mpsc::channel();
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        fs::create_dir_all(&data).unwrap();
        let a = dir.path().join("a.gp");
        let gone = dir.path().join("gone.gp");
        let stored = data.join("x.gp");
        fs::write(&a, b"a").unwrap();
        fs::write(&stored, b"x").unwrap();

        let batches = Arc::new(Mutex::new(Vec::new()));
        let seen = batches.clone();
        let ignored = vec![data];
        let worker = thread::spawn(move || {
            debounce_loop(
                &receiver,
                &ignored,
                Duration::from_millis(100),
                &move |files| seen.lock().push(files),
            );
        });
        for path in [&a, &a, &gone, &stored, &a] {
            sender.send(Message::Changed(path.clone())).unwrap();
        }
        thread::sleep(Duration::from_millis(400));
        sender.send(Message::Stop).unwrap();
        worker.join().unwrap();
        assert_eq!(*batches.lock(), vec![vec![a]]);
    }

    /// Real FSEvents round trip. Timing-dependent, so not run by default:
    /// `cargo test watches_a_folder -- --ignored`.
    #[test]
    #[ignore = "needs real FSEvents timing"]
    fn watches_a_folder() {
        let dir = tempfile::tempdir().unwrap();
        let watched = dir.path().join("watched");
        let data = watched.join("data");
        fs::create_dir_all(watched.join("nested")).unwrap();
        fs::create_dir_all(&data).unwrap();

        let (sender, receiver) = mpsc::channel();
        let sender = parking_lot::Mutex::new(sender);
        let _watcher = FolderWatcher::with_debounce(
            std::slice::from_ref(&watched),
            &data,
            Duration::from_millis(300),
            move |files| {
                let _ = sender.lock().send(files);
            },
        )
        .unwrap();
        // FSEvents only reports changes made after the stream has started.
        thread::sleep(Duration::from_millis(500));
        fs::write(watched.join("nested").join("song.gp5"), b"tab").unwrap();
        fs::write(watched.join("notes.txt"), b"text").unwrap();
        fs::write(data.join("stored.gp"), b"tab").unwrap();

        let mut reported = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            match receiver.recv_timeout(left.min(Duration::from_secs(2))) {
                Ok(files) => reported.extend(files),
                Err(_) if !reported.is_empty() => break,
                Err(_) => {}
            }
        }
        let names: Vec<String> = reported
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["song.gp5"]);
    }
}
