//! Bulk import: expand dropped files and folders into tab files, import them in parallel with
//! throttled progress, and remember what watched-folder scans have already seen.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant, UNIX_EPOCH};

use parking_lot::Mutex;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::LibraryObserver;
use crate::library::watch::is_tab_file;
use crate::library::{ImportOutcome, Library};

/// Minimum time between two progress reports (about 30 a second).
pub(crate) const PROGRESS_INTERVAL: Duration = Duration::from_millis(33);

/// Expand `paths` into the files to import, in order, without duplicates.
///
/// Files named explicitly are kept whatever their extension (an unsupported one gets an error
/// outcome, which tells the user why). Folders are walked recursively in sorted order, keeping
/// only visible tab files; hidden entries, `skip` (the library's data directory) and folders
/// already visited (symlink loops) are skipped.
pub(crate) fn expand(paths: &[PathBuf], skip: &Path) -> Vec<PathBuf> {
    let mut walker = Walker {
        skip: skip.canonicalize().unwrap_or_else(|_| skip.to_path_buf()),
        seen_dirs: HashSet::new(),
        seen_files: HashSet::new(),
        files: Vec::new(),
    };
    for path in paths {
        walker.add(path, true);
    }
    walker.files
}

struct Walker {
    skip: PathBuf,
    seen_dirs: HashSet<PathBuf>,
    seen_files: HashSet<PathBuf>,
    files: Vec<PathBuf>,
}

impl Walker {
    fn add(&mut self, path: &Path, explicit: bool) {
        match fs::metadata(path) {
            Ok(meta) if meta.is_dir() => self.walk(path),
            Ok(_) if explicit || is_tab_file(path) => self.push(path),
            Ok(_) => {}
            // Missing or unreadable: report it if the user named it, ignore broken links.
            Err(_) if explicit => self.push(path),
            Err(_) => {}
        }
    }

    fn push(&mut self, path: &Path) {
        let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if self.seen_files.insert(key) {
            self.files.push(path.to_path_buf());
        }
    }

    fn walk(&mut self, dir: &Path) {
        let Ok(canonical) = dir.canonicalize() else {
            return;
        };
        if canonical.starts_with(&self.skip) || !self.seen_dirs.insert(canonical) {
            return;
        }
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) => {
                log::warn!("couldn't read {}: {e}", dir.display());
                return;
            }
        };
        let mut children: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
        children.sort();
        for child in children {
            let hidden = child
                .file_name()
                .and_then(|n| n.to_str())
                .is_none_or(|n| n.starts_with('.'));
            if !hidden {
                self.add(&child, false);
            }
        }
    }
}

/// Import `files` in parallel. Outcomes are in the order of `files`. Progress goes to
/// `observer` at most once per [`PROGRESS_INTERVAL`], always ending with `(total, total)`.
pub(crate) fn import_all(
    library: &Library,
    files: &[PathBuf],
    observer: Option<&dyn LibraryObserver>,
) -> Vec<ImportOutcome> {
    let progress = Progress::new(observer, files.len(), PROGRESS_INTERVAL);
    let outcomes = files
        .par_iter()
        .map(|path| {
            let outcome = library.import_file(path);
            progress.tick();
            outcome
        })
        .collect();
    progress.finish();
    outcomes
}

/// Throttled progress reporting from many threads. Reports are made under a lock so they
/// arrive in order, and a thread that finds the lock taken just skips its report.
pub(crate) struct Progress<'a> {
    observer: Option<&'a dyn LibraryObserver>,
    total: u32,
    interval: Duration,
    done: AtomicU32,
    /// When the last report was made and the count it reported.
    last: Mutex<(Option<Instant>, u32)>,
}

impl<'a> Progress<'a> {
    pub(crate) fn new(
        observer: Option<&'a dyn LibraryObserver>,
        total: usize,
        interval: Duration,
    ) -> Self {
        Self {
            observer,
            total: u32::try_from(total).unwrap_or(u32::MAX),
            interval,
            done: AtomicU32::new(0),
            last: Mutex::new((None, 0)),
        }
    }

    /// One more item is done.
    pub(crate) fn tick(&self) {
        self.done.fetch_add(1, Ordering::Relaxed);
        let Some(observer) = self.observer else {
            return;
        };
        let Some(mut last) = self.last.try_lock() else {
            return;
        };
        let now = Instant::now();
        if last
            .0
            .is_some_and(|at| now.duration_since(at) < self.interval)
        {
            return;
        }
        // The final count is left to `finish`, so it's reported exactly once.
        let done = self.done.load(Ordering::Relaxed);
        if done > last.1 && done < self.total {
            observer.import_progress(done, self.total);
            *last = (Some(now), done);
        }
    }

    pub(crate) fn finish(&self) {
        if let Some(observer) = self.observer {
            let _last = self.last.lock();
            observer.import_progress(self.total, self.total);
        }
    }
}

/// The size and modification time of every file a watched-folder scan has imported, so the
/// next launch's scan can skip unchanged files without reading them. Also means a tab the user
/// deleted from the library stays deleted until its file changes (or a full rescan).
#[derive(Debug, Default)]
pub(crate) struct ScanIndex {
    path: PathBuf,
    files: HashMap<String, Stamp>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct Stamp {
    size: u64,
    mtime_ns: u128,
}

impl Stamp {
    fn of(path: &Path) -> Option<Self> {
        let meta = fs::metadata(path).ok()?;
        let mtime = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
        Some(Self {
            size: meta.len(),
            mtime_ns: mtime.as_nanos(),
        })
    }
}

impl ScanIndex {
    /// Load from `path`; a missing or unreadable file gives an empty index.
    pub(crate) fn load(path: PathBuf) -> Self {
        let files = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self { path, files }
    }

    /// The files that are new or changed since they were recorded.
    pub(crate) fn changed(&self, files: Vec<PathBuf>) -> Vec<PathBuf> {
        files
            .into_iter()
            .filter(|f| {
                let key = f.to_string_lossy();
                self.files.get(key.as_ref()).copied() != Stamp::of(f)
            })
            .collect()
    }

    /// Record `files` as imported and save the index.
    pub(crate) fn record<'p>(&mut self, files: impl IntoIterator<Item = &'p Path>) {
        let mut any = false;
        for file in files {
            if let Some(stamp) = Stamp::of(file) {
                self.files
                    .insert(file.to_string_lossy().into_owned(), stamp);
                any = true;
            }
        }
        if any && let Err(e) = self.save() {
            log::warn!("couldn't save {}: {e}", self.path.display());
        }
    }

    fn save(&self) -> std::io::Result<()> {
        let bytes = serde_json::to_vec(&self.files).map_err(std::io::Error::other)?;
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, bytes)?;
        fs::rename(tmp, &self.path)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::thread;

    use super::*;

    fn touch(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"x").unwrap();
    }

    fn names(root: &Path, files: &[PathBuf]) -> Vec<String> {
        files
            .iter()
            .map(|f| f.strip_prefix(root).unwrap().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn expands_folders() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for name in [
            "tabs/b.gp5",
            "tabs/a.GP",
            "tabs/notes.txt",
            "tabs/.hidden.gp",
            "tabs/.git/x.gp",
            "tabs/nested/deeper/c.gpx",
            "tabs/nested/d.gp4",
            "tabs/nested/e.Gp3",
            "tabs/nested/f.gp.download",
            "tabs/data/stored.gp",
            "loose.txt",
        ] {
            touch(&root.join(name));
        }
        // A symlink loop and a second route to the same folder.
        std::os::unix::fs::symlink(root.join("tabs"), root.join("tabs/nested/loop")).unwrap();
        std::os::unix::fs::symlink(root.join("tabs/nested"), root.join("tabs/alias")).unwrap();

        let files = expand(
            &[
                root.join("tabs"),
                root.join("tabs/b.gp5"),
                root.join("loose.txt"),
                root.join("missing.gp"),
            ],
            &root.join("tabs/data"),
        );
        assert_eq!(
            names(root, &files),
            vec![
                "tabs/a.GP",
                "tabs/alias/d.gp4",
                "tabs/alias/deeper/c.gpx",
                "tabs/alias/e.Gp3",
                "tabs/b.gp5",
                "loose.txt",
                "missing.gp",
            ]
        );
    }

    #[derive(Default)]
    struct Counter {
        calls: Mutex<Vec<(u32, u32)>>,
    }

    impl LibraryObserver for Counter {
        fn import_progress(&self, done: u32, total: u32) {
            self.calls.lock().push((done, total));
        }
        fn library_changed(&self) {}
    }

    #[test]
    fn progress_is_throttled_and_ordered() {
        let counter = Counter::default();
        let total = 4000;
        let progress = Progress::new(Some(&counter), total, Duration::from_millis(20));
        let started = Instant::now();
        thread::scope(|s| {
            for _ in 0..8 {
                s.spawn(|| {
                    for _ in 0..total / 8 {
                        progress.tick();
                        thread::sleep(Duration::from_micros(200));
                    }
                });
            }
        });
        progress.finish();
        let elapsed = started.elapsed();

        let calls = counter.calls.lock();
        assert_eq!(calls.last(), Some(&(4000, 4000)));
        assert_eq!(calls.iter().filter(|c| c.0 == 4000).count(), 1);
        assert!(calls.windows(2).all(|w| w[0].0 < w[1].0), "{calls:?}");
        assert!(calls.len() > 1);
        let max = (elapsed.as_millis() / 20 + 2) as usize;
        assert!(calls.len() <= max, "{} calls in {elapsed:?}", calls.len());
    }

    #[test]
    fn progress_without_observer_or_items() {
        Progress::new(None, 3, PROGRESS_INTERVAL).tick();
        let counter = Arc::new(Counter::default());
        Progress::new(Some(counter.as_ref()), 0, PROGRESS_INTERVAL).finish();
        assert_eq!(*counter.calls.lock(), vec![(0, 0)]);
    }

    #[test]
    fn scan_index_skips_unchanged_files() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.gp");
        let b = dir.path().join("b.gp");
        touch(&a);
        touch(&b);
        let index_path = dir.path().join("index.json");
        let mut index = ScanIndex::load(index_path.clone());
        assert_eq!(index.changed(vec![a.clone(), b.clone()]).len(), 2);
        index.record([a.as_path()]);

        let index = ScanIndex::load(index_path);
        assert_eq!(index.changed(vec![a.clone(), b.clone()]), vec![b.clone()]);
        fs::write(&a, b"longer").unwrap();
        assert_eq!(index.changed(vec![a.clone(), b.clone()]), vec![a, b]);
    }
}
