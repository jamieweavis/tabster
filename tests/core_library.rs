//! `Core` library plumbing: bulk import, config, watched folders, the player server.

use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tabster_core::config::{self, Config};
use tabster_core::library::TabFilter;
use tabster_core::{Core, LibraryObserver};

const TITLE_OFFSET: usize = 0x24;
const TITLE_LEN: usize = 14;

/// `testdata/gp5/basics.gp5` with its 14-character title replaced, so every copy has distinct
/// content (the library dedupes by hash).
fn gp5_variant(n: usize) -> Vec<u8> {
    let mut bytes =
        fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/gp5/basics.gp5")).unwrap();
    assert_eq!(
        &bytes[TITLE_OFFSET..TITLE_OFFSET + TITLE_LEN],
        b"Tabster Basics"
    );
    let title = format!("Variant {n:06}");
    bytes[TITLE_OFFSET..TITLE_OFFSET + TITLE_LEN].copy_from_slice(title.as_bytes());
    bytes
}

fn write_variants(dir: &Path, range: std::ops::Range<usize>) -> Vec<PathBuf> {
    fs::create_dir_all(dir).unwrap();
    range
        .map(|n| {
            let path = dir.join(format!("tab{n:05}.gp5"));
            fs::write(&path, gp5_variant(n)).unwrap();
            path
        })
        .collect()
}

fn core_in(dir: &Path) -> Arc<Core> {
    Core::with_paths(&dir.join("data"), &dir.join("tabster.toml")).unwrap()
}

fn tab_count(core: &Core) -> usize {
    core.list_tabs(TabFilter::default()).unwrap().len()
}

fn wait_for_tabs(core: &Core, count: usize) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while tab_count(core) != count {
        assert!(
            Instant::now() < deadline,
            "expected {count} tabs, have {}",
            tab_count(core)
        );
        thread::sleep(Duration::from_millis(20));
    }
}

#[derive(Default)]
struct Recorder {
    progress: Mutex<Vec<(u32, u32)>>,
    changed: AtomicU32,
}

impl LibraryObserver for Recorder {
    fn import_progress(&self, done: u32, total: u32) {
        self.progress.lock().push((done, total));
    }
    fn library_changed(&self) {
        self.changed.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn imports_folders_in_parallel() {
    let dir = tempfile::tempdir().unwrap();
    let tabs = dir.path().join("tabs");
    write_variants(&tabs.join("a"), 0..20);
    write_variants(&tabs.join(".hidden"), 20..25);
    fs::write(tabs.join("notes.txt"), "not a tab").unwrap();
    fs::write(tabs.join("broken.gp5"), "not a tab either").unwrap();
    fs::copy(tabs.join("a/tab00000.gp5"), tabs.join("copy.GP5")).unwrap();

    let core = core_in(dir.path());
    let recorder = Arc::new(Recorder::default());
    core.set_library_observer(Some(recorder.clone()));
    let outcomes = core.import_paths(vec![
        tabs.to_string_lossy().into_owned(),
        tabs.join("a/tab00003.gp5").to_string_lossy().into_owned(),
    ]);

    // a/ (20), then broken.gp5, then copy.GP5; the repeated file and hidden folder are skipped.
    assert_eq!(outcomes.len(), 22);
    assert!(outcomes[0].path.ends_with("a/tab00000.gp5"));
    assert!(outcomes[19].path.ends_with("a/tab00019.gp5"));
    assert!(outcomes[..20].iter().all(|o| o.error.is_none()));
    assert!(outcomes[20].path.ends_with("broken.gp5") && outcomes[20].error.is_some());
    // copy.GP5 has the same content as tab00000.gp5 and they import in parallel, so either can
    // be the original: exactly one is the duplicate, both resolve to the same tab, neither fails.
    assert!(outcomes[21].error.is_none());
    assert!(outcomes[0].duplicate != outcomes[21].duplicate);
    assert_eq!(outcomes[21].id, outcomes[0].id);
    assert_eq!(tab_count(&core), 20);

    assert_eq!(recorder.progress.lock().last(), Some(&(22, 22)));
    assert_eq!(recorder.changed.load(Ordering::SeqCst), 1);
}

#[test]
fn config_round_trips_through_core() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("tabster.toml");
    let core = core_in(dir.path());
    assert_eq!(core.config(), Config::default());

    let config = Config {
        scale: 1.75,
        show_rhythm: false,
        watched_folders: vec![dir.path().join("nowhere").to_string_lossy().into_owned()],
        follow_cursor: false,
    };
    core.set_config(config.clone());
    assert_eq!(core.config(), config);
    // Written once changes settle.
    let deadline = Instant::now() + Duration::from_secs(5);
    while config::load(&config_path) != config {
        assert!(Instant::now() < deadline, "config never saved");
        thread::sleep(Duration::from_millis(20));
    }

    // The newest config is flushed on drop.
    let newer = Config {
        scale: 0.5,
        ..config
    };
    core.set_config(newer.clone());
    drop(core);
    assert_eq!(config::load(&config_path), newer);
    assert_eq!(core_in(dir.path()).config(), newer);
}

#[test]
fn watched_folders_are_scanned_and_watched() {
    let dir = tempfile::tempdir().unwrap();
    let watched = dir.path().join("watched");
    write_variants(&watched, 0..3);
    let config = Config {
        watched_folders: vec![
            watched.to_string_lossy().into_owned(),
            dir.path().join("missing").to_string_lossy().into_owned(),
        ],
        ..Default::default()
    };
    config::save(&dir.path().join("tabster.toml"), &config).unwrap();

    // Startup scan.
    let core = core_in(dir.path());
    wait_for_tabs(&core, 3);

    // A deleted tab stays deleted on the next launch (its file is unchanged)…
    let first = core.list_tabs(TabFilter::default()).unwrap()[0].id;
    core.delete_tab(first).unwrap();
    drop(core);
    let core = core_in(dir.path());
    thread::sleep(Duration::from_millis(300));
    assert_eq!(tab_count(&core), 2);

    // …until an explicit rescan.
    core.rescan_watched_folders();
    wait_for_tabs(&core, 3);

    // A folder added in settings is scanned straight away.
    let later = dir.path().join("later");
    write_variants(&later, 10..12);
    let mut config = core.config();
    config
        .watched_folders
        .push(later.to_string_lossy().into_owned());
    core.set_config(config);
    wait_for_tabs(&core, 5);
}

#[test]
fn player_url_serves_the_page() {
    let dir = tempfile::tempdir().unwrap();
    let core = core_in(dir.path());
    core.set_player_page("<html>yt</html>".into());
    let url = core.player_url();
    assert_eq!(core.player_url(), url, "one server per core");
    let port: u16 = url
        .strip_prefix("http://localhost:")
        .and_then(|rest| rest.strip_suffix("/player.html"))
        .unwrap()
        .parse()
        .unwrap();
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .write_all(
            b"GET /player.html?v=dQw4w9WgXcQ HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
        )
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.ends_with("<html>yt</html>"));
}

/// The PLAN.md target: 1,000 small tabs in under 10 s.
/// `cargo test --release --test core_library bulk_import_benchmark -- --ignored --nocapture`
#[test]
#[ignore = "benchmark"]
fn bulk_import_benchmark() {
    let dir = tempfile::tempdir().unwrap();
    let tabs = dir.path().join("tabs");
    write_variants(&tabs, 0..1000);
    let core = core_in(dir.path());
    let recorder = Arc::new(Recorder::default());
    core.set_library_observer(Some(recorder.clone()));

    let started = Instant::now();
    let outcomes = core.import_paths(vec![tabs.to_string_lossy().into_owned()]);
    let elapsed = started.elapsed();
    assert_eq!(outcomes.len(), 1000);
    assert!(outcomes.iter().all(|o| o.error.is_none() && !o.duplicate));
    assert_eq!(tab_count(&core), 1000);

    let started = Instant::now();
    let again = core.import_paths(vec![tabs.to_string_lossy().into_owned()]);
    let dedupe = started.elapsed();
    assert!(again.iter().all(|o| o.duplicate));

    println!(
        "imported 1000 tabs in {elapsed:?} ({} progress calls); re-import (all duplicates) {dedupe:?}",
        recorder.progress.lock().len()
    );
    assert!(elapsed < Duration::from_secs(10));
}
