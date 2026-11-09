//! Tabster core: everything the macOS shell needs, exposed over UniFFI as the [`Core`] object
//! (library management) and [`TabSession`] objects (one open tab: layout, timing, sync).

uniffi::setup_scaffolding!();

pub mod config;
pub mod formats;
mod import;
pub mod layout;
pub mod library;
pub mod logging;
pub mod model;
pub mod playback;
pub mod player_server;
pub mod sync;
pub mod timeline;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::{Mutex, RwLock};

use crate::config::{Config, ConfigSaver};
use crate::import::ScanIndex;
use crate::layout::{Cursor, DisplayList, LayoutOptions};
use crate::library::{
    FolderWatcher, ImportOutcome, Library, LibraryError, PracticeState, PracticeStats, SavedLoop,
    Setlist, TabFilter, TabId, TabSummary, Video, VideoId,
};
use crate::model::Score;
use crate::playback::{SynthError, TabSynth};
use crate::player_server::PlayerServer;
use crate::sync::{SyncMap, SyncPoint, SyncSpec};
use crate::timeline::Timeline;

#[derive(Debug, thiserror::Error, uniffi::Error)]
#[uniffi(flat_error)]
pub enum CoreError {
    #[error("{0}")]
    Library(String),
    #[error("not a YouTube link or video id")]
    InvalidVideo,
    #[error("not found")]
    NotFound,
}

impl From<LibraryError> for CoreError {
    fn from(e: LibraryError) -> Self {
        match e {
            LibraryError::NotFound => CoreError::NotFound,
            e => CoreError::Library(e.to_string()),
        }
    }
}

/// Library change notifications. Called on arbitrary threads.
#[uniffi::export(with_foreign)]
pub trait LibraryObserver: Send + Sync {
    fn import_progress(&self, done: u32, total: u32);
    fn library_changed(&self);
}

/// How long config changes must settle before they're written.
const CONFIG_SAVE_DEBOUNCE: Duration = Duration::from_millis(500);

#[derive(uniffi::Object)]
pub struct Core {
    shared: Arc<Shared>,
    config: RwLock<Config>,
    saver: ConfigSaver,
    watched: Mutex<Watched>,
    player_page: player_server::Page,
    player: Mutex<Option<PlayerServer>>,
}

/// The parts background work (folder watching, scans) needs too.
struct Shared {
    library: Library,
    observer: RwLock<Option<Arc<dyn LibraryObserver>>>,
    scan_index: Mutex<ScanIndex>,
}

struct Watched {
    /// `None` if the watcher couldn't start (then folders are only scanned).
    watcher: Option<FolderWatcher>,
    /// The folders being watched: the configured ones that existed when last applied.
    folders: Vec<PathBuf>,
}

#[uniffi::export]
impl Core {
    /// `data_dir`: the app's data directory (`~/Library/Application Support/Tabster`). The
    /// config comes from `~/.config/tabster.toml` (or `TABSTER_CONFIG`).
    #[uniffi::constructor]
    pub fn new(data_dir: String) -> Result<Arc<Self>, CoreError> {
        logging::init();
        Self::with_paths(Path::new(&data_dir), &config::default_path())
    }

    pub fn version(&self) -> String {
        env!("CARGO_PKG_VERSION").to_string()
    }

    pub fn set_library_observer(&self, observer: Option<Arc<dyn LibraryObserver>>) {
        *self.shared.observer.write() = observer;
    }

    /// Import files (and the tab files inside any folders, recursively; hidden folders are
    /// skipped). Blocking: call from a background task. Files are imported in parallel;
    /// progress is reported through the observer at most ~30 times a second, then once with
    /// `(total, total)`, followed by one `library_changed`. Outcomes are in the order of the
    /// expanded file list (each folder's files sorted, duplicates dropped).
    pub fn import_paths(&self, paths: Vec<String>) -> Vec<ImportOutcome> {
        let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
        let files = import::expand(&paths, self.shared.library.data_dir());
        self.shared.import_files(&files)
    }

    /// The current settings.
    pub fn config(&self) -> Config {
        self.config.read().clone()
    }

    /// Replace the settings. Applied immediately; written to disk once changes settle
    /// (~500 ms). Newly watched folders are watched and scanned in the background.
    pub fn set_config(&self, config: Config) {
        let folders_changed = {
            let mut current = self.config.write();
            let changed = current.watched_folders != config.watched_folders;
            *current = config.clone();
            changed
        };
        self.saver.save(config);
        if folders_changed {
            let added = self.apply_watched_folders();
            self.scan_in_background(added, true);
        }
    }

    /// Re-read every watched folder (picking up configured folders that didn't exist before)
    /// and import whatever isn't in the library, including files of tabs deleted from it.
    /// Returns at once; the work happens in the background and reports through the observer.
    pub fn rescan_watched_folders(&self) {
        self.apply_watched_folders();
        let folders = self.watched.lock().folders.clone();
        self.scan_in_background(folders, false);
    }

    /// Provide the YouTube player page served at [`Core::player_url`].
    pub fn set_player_page(&self, html: String) {
        *self.player_page.write() = Arc::from(html.into_bytes());
    }

    /// `http://localhost:<port>/player.html`, served by a local HTTP server (listening on
    /// 127.0.0.1 only) started on the first call. The URL names `localhost` because YouTube
    /// rejects IP-literal origins with error 150. Query strings are accepted
    /// (`/player.html?v=<id>`). It serves the page given to `set_player_page` (404 until then).
    /// Empty if the server couldn't start.
    pub fn player_url(&self) -> String {
        let mut player = self.player.lock();
        if player.is_none() {
            match PlayerServer::start(Arc::clone(&self.player_page)) {
                Ok(server) => *player = Some(server),
                Err(e) => {
                    log::error!("couldn't start the player server: {e}");
                    return String::new();
                }
            }
        }
        player.as_ref().map(PlayerServer::url).unwrap_or_default()
    }

    pub fn list_tabs(&self, filter: TabFilter) -> Result<Vec<TabSummary>, CoreError> {
        Ok(self.shared.library.list(&filter)?)
    }

    pub fn tab(&self, id: TabId) -> Result<TabSummary, CoreError> {
        Ok(self.shared.library.get(id)?)
    }

    pub fn tab_file_path(&self, id: TabId) -> Result<String, CoreError> {
        Ok(self
            .shared
            .library
            .file_path(id)?
            .to_string_lossy()
            .into_owned())
    }

    pub fn delete_tab(&self, id: TabId) -> Result<(), CoreError> {
        self.shared.library.delete(id)?;
        self.changed();
        Ok(())
    }

    pub fn set_favourite(&self, id: TabId, favourite: bool) -> Result<(), CoreError> {
        self.shared.library.set_favourite(id, favourite)?;
        self.changed();
        Ok(())
    }

    pub fn set_tags(&self, id: TabId, tags: Vec<String>) -> Result<(), CoreError> {
        self.shared.library.set_tags(id, &tags)?;
        self.changed();
        Ok(())
    }

    pub fn tags(&self) -> Result<Vec<String>, CoreError> {
        Ok(self.shared.library.tags()?)
    }

    pub fn setlists(&self) -> Result<Vec<Setlist>, CoreError> {
        Ok(self.shared.library.setlists()?)
    }

    pub fn create_setlist(&self, name: String) -> Result<i64, CoreError> {
        let id = self.shared.library.create_setlist(&name)?;
        self.changed();
        Ok(id)
    }

    pub fn rename_setlist(&self, id: i64, name: String) -> Result<(), CoreError> {
        self.shared.library.rename_setlist(id, &name)?;
        self.changed();
        Ok(())
    }

    pub fn delete_setlist(&self, id: i64) -> Result<(), CoreError> {
        self.shared.library.delete_setlist(id)?;
        self.changed();
        Ok(())
    }

    pub fn set_setlist_tabs(&self, id: i64, tabs: Vec<TabId>) -> Result<(), CoreError> {
        self.shared.library.set_setlist_tabs(id, &tabs)?;
        self.changed();
        Ok(())
    }

    pub fn videos(&self, tab: TabId) -> Result<Vec<Video>, CoreError> {
        Ok(self.shared.library.videos(tab)?)
    }

    /// Attach a YouTube video given a URL or bare id.
    pub fn add_video(
        &self,
        tab: TabId,
        url_or_id: String,
        label: String,
    ) -> Result<VideoId, CoreError> {
        let youtube_id = library::parse_youtube_id(&url_or_id).ok_or(CoreError::InvalidVideo)?;
        let id = self.shared.library.add_video(tab, &youtube_id, &label)?;
        self.changed();
        Ok(id)
    }

    pub fn remove_video(&self, id: VideoId) -> Result<(), CoreError> {
        self.shared.library.remove_video(id)?;
        self.changed();
        Ok(())
    }

    pub fn set_default_video(&self, id: VideoId) -> Result<(), CoreError> {
        Ok(self.shared.library.set_default_video(id)?)
    }

    pub fn set_video_sync(&self, id: VideoId, sync: SyncSpec) -> Result<(), CoreError> {
        Ok(self.shared.library.set_video_sync(id, &sync)?)
    }

    pub fn practice_state(&self, tab: TabId) -> Result<PracticeState, CoreError> {
        Ok(self.shared.library.practice_state(tab)?)
    }

    pub fn set_practice_state(&self, tab: TabId, state: PracticeState) -> Result<(), CoreError> {
        Ok(self.shared.library.set_practice_state(tab, &state)?)
    }

    /// The tab's saved loop sections, by start bar.
    pub fn saved_loops(&self, tab: TabId) -> Result<Vec<SavedLoop>, CoreError> {
        Ok(self.shared.library.saved_loops(tab)?)
    }

    /// Save a named loop over master bars `start_bar..=end_bar`; returns its id.
    pub fn save_loop(
        &self,
        tab: TabId,
        name: String,
        start_bar: u32,
        end_bar: u32,
    ) -> Result<i64, CoreError> {
        Ok(self
            .shared
            .library
            .save_loop(tab, &name, start_bar, end_bar)?)
    }

    pub fn delete_loop(&self, id: i64) -> Result<(), CoreError> {
        Ok(self.shared.library.delete_loop(id)?)
    }

    pub fn rename_loop(&self, id: i64, name: String) -> Result<(), CoreError> {
        Ok(self.shared.library.rename_loop(id, &name)?)
    }

    /// Add `seconds` of playing time to the tab's practice history (today, local time).
    pub fn record_practice(&self, tab: TabId, seconds: f64) -> Result<(), CoreError> {
        Ok(self.shared.library.record_practice(tab, seconds)?)
    }

    pub fn practice_stats(&self, tab: TabId) -> Result<PracticeStats, CoreError> {
        Ok(self.shared.library.practice_stats(tab)?)
    }

    /// Total practice seconds per tab (tabs never practised are absent).
    pub fn practice_totals(&self) -> Result<HashMap<TabId, f64>, CoreError> {
        Ok(self.shared.library.practice_totals()?)
    }

    /// Open a tab for viewing and playback.
    pub fn open_tab(&self, id: TabId) -> Result<Arc<TabSession>, CoreError> {
        let score = self.shared.library.load_score(id)?;
        // Opening updates "last opened", which the Recently Opened list shows.
        self.changed();
        Ok(Arc::new(TabSession::new(id, score)))
    }
}

impl Core {
    /// Open the library in `data_dir` with the config at `config_path`, start watching the
    /// configured folders, and scan them for new files in the background.
    pub fn with_paths(data_dir: &Path, config_path: &Path) -> Result<Arc<Self>, CoreError> {
        let library = Library::open(data_dir)?;
        let scan_index = ScanIndex::load(library.data_dir().join("watch-index.json"));
        let shared = Arc::new(Shared {
            library,
            observer: RwLock::new(None),
            scan_index: Mutex::new(scan_index),
        });
        let watcher = {
            let shared = Arc::clone(&shared);
            let data_dir = shared.library.data_dir().to_path_buf();
            FolderWatcher::new(&[], &data_dir, move |files| {
                log::info!("{} new file(s) in watched folders", files.len());
                shared.import_files(&files);
            })
        };
        let watcher = watcher
            .inspect_err(|e| log::error!("couldn't start watching folders: {e}"))
            .ok();
        let core = Arc::new(Self {
            shared,
            config: RwLock::new(config::load(config_path)),
            saver: ConfigSaver::new(config_path.to_path_buf(), CONFIG_SAVE_DEBOUNCE),
            watched: Mutex::new(Watched {
                watcher,
                folders: Vec::new(),
            }),
            player_page: Arc::new(RwLock::new(Arc::from(&b""[..]))),
            player: Mutex::new(None),
        });
        let folders = core.apply_watched_folders();
        core.scan_in_background(folders, true);
        Ok(core)
    }

    fn changed(&self) {
        self.shared.changed();
    }

    /// Make the watcher match the configured folders; returns the folders newly watched.
    /// Folders that don't exist are skipped (with a warning) until the next call.
    fn apply_watched_folders(&self) -> Vec<PathBuf> {
        let mut wanted: Vec<PathBuf> = Vec::new();
        for folder in &self.config.read().watched_folders {
            let folder = expand_home(folder);
            if !wanted.contains(&folder) {
                wanted.push(folder);
            }
        }
        let mut watched = self.watched.lock();
        let Watched { watcher, folders } = &mut *watched;
        for old in folders.iter().filter(|f| !wanted.contains(f)) {
            if let Some(watcher) = watcher.as_mut()
                && let Err(e) = watcher.unwatch(old)
            {
                log::warn!("couldn't stop watching {}: {e}", old.display());
            }
        }
        let mut now_watched = Vec::new();
        let mut added = Vec::new();
        for folder in wanted {
            if folders.contains(&folder) {
                now_watched.push(folder);
                continue;
            }
            if !folder.is_dir() {
                log::warn!(
                    "watched folder {} doesn't exist; skipping",
                    folder.display()
                );
                continue;
            }
            if let Some(watcher) = watcher.as_mut()
                && let Err(e) = watcher.watch(&folder)
            {
                log::warn!("couldn't watch {}: {e}", folder.display());
            }
            now_watched.push(folder.clone());
            added.push(folder);
        }
        *folders = now_watched;
        added
    }

    /// Import new tab files from `folders` on a background thread. With `skip_seen`, files a
    /// previous scan imported that haven't changed since aren't read again.
    fn scan_in_background(&self, folders: Vec<PathBuf>, skip_seen: bool) {
        if folders.is_empty() {
            return;
        }
        let shared = Arc::clone(&self.shared);
        let spawned = std::thread::Builder::new()
            .name("tabster-folder-scan".into())
            .spawn(move || shared.scan(&folders, skip_seen));
        if let Err(e) = spawned {
            log::error!("couldn't start a folder scan: {e}");
        }
    }
}

impl Shared {
    fn changed(&self) {
        if let Some(o) = self.observer.read().clone() {
            o.library_changed();
        }
    }

    /// Import `files` in parallel with progress, then notify one library change.
    fn import_files(&self, files: &[PathBuf]) -> Vec<ImportOutcome> {
        let observer = self.observer.read().clone();
        let outcomes = import::import_all(&self.library, files, observer.as_deref());
        self.scan_index.lock().record(
            files
                .iter()
                .zip(&outcomes)
                .filter(|(_, o)| o.error.is_none())
                .map(|(f, _)| f.as_path()),
        );
        if let Some(o) = observer {
            o.library_changed();
        }
        outcomes
    }

    /// Import the tab files in `folders`; with `skip_seen`, only new or changed ones.
    fn scan(&self, folders: &[PathBuf], skip_seen: bool) -> Vec<ImportOutcome> {
        let mut files = import::expand(folders, self.library.data_dir());
        if skip_seen {
            files = self.scan_index.lock().changed(files);
        }
        if files.is_empty() {
            return Vec::new();
        }
        log::info!("importing {} file(s) from watched folders", files.len());
        self.import_files(&files)
    }
}

/// `~/x` → `$HOME/x`.
fn expand_home(path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(path),
    }
}

// ---------------------------------------------------------------------------------------------
// TabSession
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct TrackInfo {
    pub index: u32,
    /// What the track picker shows, e.g. "Bass · E A D G" or "Guitar 2 · D A D G B E · capo 2".
    pub label: String,
    pub strings: u32,
    /// Note names, string 1 (highest) first, e.g. ["E", "B", "G", "D", "A", "E"].
    pub tuning: Vec<String>,
    pub capo: u32,
    pub is_percussion: bool,
}

/// "Guitar 2 · E A D G B E · capo 2": the instrument (numbered when the score has several of the
/// same kind), the tuning lowest string first, and the capo. Tracks of no recognised kind keep
/// their own name.
fn track_label(tracks: &[model::Track], index: usize) -> String {
    use model::Instrument;
    let track = &tracks[index];
    let kind = track.instrument();
    let mut label = if kind == Instrument::Other {
        match track.name.trim() {
            "" => format!("Track {}", index + 1),
            name => name.to_owned(),
        }
    } else {
        let same = |t: &model::Track| t.instrument() == kind;
        let total = tracks.iter().filter(|t| same(t)).count();
        if total > 1 {
            let nth = tracks[..index].iter().filter(|t| same(t)).count() + 1;
            format!("{} {nth}", kind.label())
        } else {
            kind.label().to_owned()
        }
    };
    if matches!(
        kind,
        Instrument::Guitar | Instrument::Bass | Instrument::Other
    ) && !track.is_percussion
        && !track.tuning.is_empty()
    {
        label.push_str(" · ");
        let names: Vec<&str> = track
            .tuning
            .iter()
            .rev()
            .map(|&p| model::note_name(p))
            .collect();
        label.push_str(&names.join(" "));
    }
    if track.capo > 0 && !track.is_percussion {
        label.push_str(&format!(" · capo {}", track.capo));
    }
    label
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SectionInfo {
    pub master_bar: u32,
    pub label: String,
    pub score_secs: f64,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct TabInfo {
    pub id: TabId,
    pub title: String,
    pub artist: String,
    pub tracks: Vec<TrackInfo>,
    pub bar_count: u32,
    pub played_bar_count: u32,
    pub duration_secs: f64,
    pub initial_tempo: f64,
    pub sections: Vec<SectionInfo>,
}

/// One bar as played (repeats and jumps unrolled).
#[derive(Debug, Clone, Copy, PartialEq, uniffi::Record)]
pub struct PlayedBarInfo {
    pub master_bar: u32,
    pub occurrence: u32,
    pub start_secs: f64,
    pub end_secs: f64,
}

/// Where playback is, for drawing the cursor.
#[derive(Debug, Clone, Copy, PartialEq, uniffi::Record)]
pub struct PlaybackPosition {
    pub score_secs: f64,
    pub played_bar: u32,
    pub master_bar: u32,
    /// `None` until `layout` has been called, or if the bar isn't laid out.
    pub cursor: Option<Cursor>,
}

struct SessionState {
    layout: DisplayList,
    sync: SyncMap,
    /// The looped span in score seconds: the cursor glides from its end back to its start.
    loop_span: Option<(f64, f64)>,
}

/// How long before a downbeat (score seconds) the cursor starts sliding into the next bar, so it
/// lands on the bar's first beat as that beat plays. Never starts before the bar's last onset.
const GLIDE_SECS: f64 = 0.12;

/// One open tab. Thread-safe; the per-frame methods (`position_at_*`) don't allocate beyond the
/// returned record.
#[derive(uniffi::Object)]
pub struct TabSession {
    id: TabId,
    score: Score,
    timeline: Timeline,
    state: Mutex<SessionState>,
}

impl TabSession {
    pub fn new(id: TabId, score: Score) -> Self {
        let timeline = Timeline::new(&score);
        let sync = SyncMap::new(&SyncSpec::default(), &timeline);
        Self {
            id,
            score,
            timeline,
            state: Mutex::new(SessionState {
                layout: DisplayList::default(),
                sync,
                loop_span: None,
            }),
        }
    }

    pub fn score(&self) -> &Score {
        &self.score
    }

    /// In the last moment of a played bar, slide `cursor` towards the first beat of the bar
    /// played next (the loop start when the bar ends the loop), arriving on the downbeat.
    /// Hot path: no allocation.
    fn glide_into_next_bar(
        &self,
        state: &SessionState,
        cursor: Cursor,
        played_bar: usize,
        score_secs: f64,
    ) -> Cursor {
        let bars = self.timeline.played_bars();
        let bar = &bars[played_bar];
        let remaining = bar.end_secs - score_secs;
        if !(remaining > 0.0 && remaining < GLIDE_SECS) {
            return cursor;
        }
        let next = match state.loop_span {
            Some((start, end)) if (bar.end_secs - end).abs() < 1e-6 => {
                self.timeline.position_at_secs(start).map(|p| p.played_bar)
            }
            _ => Some(played_bar + 1),
        };
        let Some(next) = next.and_then(|i| bars.get(i)) else {
            return cursor;
        };
        let bar_ticks = (bar.end_tick - bar.start_tick) as u32;
        let last_onset_secs = layout::last_onset(&state.layout, bar.master_bar)
            .filter(|&t| t < bar_ticks)
            .map_or(bar.start_secs, |t| {
                self.timeline.tick_to_secs(bar.start_tick + u64::from(t))
            });
        let window = GLIDE_SECS.min(bar.end_secs - last_onset_secs);
        if window <= 0.0 || remaining >= window {
            return cursor;
        }
        let next_ticks = (next.end_tick - next.start_tick) as u32;
        match layout::cursor(&state.layout, next.master_bar, 0, next_ticks) {
            Some(to) => layout::glide(cursor, to, (1.0 - remaining / window) as f32),
            None => cursor,
        }
    }
}

#[uniffi::export]
impl TabSession {
    pub fn info(&self) -> TabInfo {
        let tracks = self
            .score
            .tracks
            .iter()
            .enumerate()
            .map(|(i, t)| TrackInfo {
                index: i as u32,
                label: track_label(&self.score.tracks, i),
                strings: t.tuning.len() as u32,
                tuning: t
                    .tuning
                    .iter()
                    .map(|&p| crate::model::note_name(p).to_string())
                    .collect(),
                capo: u32::from(t.capo),
                is_percussion: t.is_percussion,
            })
            .collect();
        let sections = self
            .score
            .master_bars
            .iter()
            .enumerate()
            .filter_map(|(i, mb)| {
                let s = mb.section.as_ref()?;
                let label = match (s.letter.is_empty(), s.text.is_empty()) {
                    (false, false) => format!("{} · {}", s.letter, s.text),
                    (true, _) => s.text.clone(),
                    (false, true) => s.letter.clone(),
                };
                Some(SectionInfo {
                    master_bar: i as u32,
                    label,
                    score_secs: self.score_secs_for_bar(i as u32),
                })
            })
            .collect();
        TabInfo {
            id: self.id,
            title: self.score.title.clone(),
            artist: self.score.artist.clone(),
            tracks,
            bar_count: self.score.master_bars.len() as u32,
            played_bar_count: self.timeline.played_bars().len() as u32,
            duration_secs: self.timeline.duration_secs(),
            initial_tempo: self.score.initial_tempo,
            sections,
        }
    }

    /// Lay out a track (call again on resize/zoom/track change). The session keeps the latest
    /// layout for cursor positioning and hit-testing.
    pub fn layout(&self, options: LayoutOptions) -> DisplayList {
        let list = layout::layout(&self.score, &options);
        self.state.lock().layout = list.clone();
        list
    }

    pub fn set_sync(&self, spec: SyncSpec) {
        self.state.lock().sync = SyncMap::new(&spec, &self.timeline);
    }

    /// The current sync map expanded to one point per played bar (for the sync editor).
    pub fn sync_points(&self) -> Vec<SyncPoint> {
        self.state.lock().sync.to_points(&self.timeline)
    }

    /// The sync points stored in the tab file itself, if any.
    pub fn file_sync(&self) -> Option<SyncSpec> {
        sync::from_file_sync_points(&self.score, &self.timeline)
    }

    pub fn video_to_score(&self, video_secs: f64) -> f64 {
        self.state.lock().sync.video_to_score(video_secs)
    }

    pub fn score_to_video(&self, score_secs: f64) -> f64 {
        self.state.lock().sync.score_to_video(score_secs)
    }

    pub fn position_at_score(&self, score_secs: f64) -> Option<PlaybackPosition> {
        let pos = self.timeline.position_at_secs(score_secs)?;
        let bar = self.timeline.played_bars()[pos.played_bar];
        let bar_ticks = (bar.end_tick - bar.start_tick) as u32;
        let state = self.state.lock();
        let cursor = layout::cursor(&state.layout, pos.master_bar, pos.tick_in_bar, bar_ticks)
            .map(|c| self.glide_into_next_bar(&state, c, pos.played_bar, score_secs));
        Some(PlaybackPosition {
            score_secs,
            played_bar: pos.played_bar as u32,
            master_bar: pos.master_bar,
            cursor,
        })
    }

    /// The looped span (score seconds) playback wraps around, so the cursor glides from its
    /// end to its start rather than on into the bar after it. `None` clears it.
    pub fn set_loop(&self, start_secs: Option<f64>, end_secs: Option<f64>) {
        self.state.lock().loop_span = start_secs.zip(end_secs).filter(|(s, e)| s < e);
    }

    pub fn position_at_video(&self, video_secs: f64) -> Option<PlaybackPosition> {
        let score_secs = self.video_to_score(video_secs);
        self.position_at_score(score_secs)
    }

    /// Every played bar in order, with its score-time span.
    pub fn played_bars(&self) -> Vec<PlayedBarInfo> {
        self.timeline
            .played_bars()
            .iter()
            .map(|b| PlayedBarInfo {
                master_bar: b.master_bar,
                occurrence: b.occurrence,
                start_secs: b.start_secs,
                end_secs: b.end_secs,
            })
            .collect()
    }

    /// Score seconds at the first time `master_bar` is played (0 if it's never played).
    pub fn score_secs_for_bar(&self, master_bar: u32) -> f64 {
        self.timeline
            .played_index(master_bar, 0)
            .map_or(0.0, |i| self.timeline.played_bars()[i].start_secs)
    }

    /// Score seconds of the start of a played bar (clamped).
    pub fn score_secs_for_played_bar(&self, played_bar: u32) -> f64 {
        let bars = self.timeline.played_bars();
        bars.get(played_bar as usize)
            .or(bars.last())
            .map_or(0.0, |b| b.start_secs)
    }

    /// A synthesizer for hearing this tab, using the SoundFont at `soundfont_path`. Fails with
    /// `NotFound` if there's no SoundFont there (tab audio is then unavailable). Blocking
    /// (the first call loads the SoundFont): call from a background task.
    pub fn synth(&self, soundfont_path: String) -> Result<Arc<TabSynth>, CoreError> {
        TabSynth::new(&self.score, &self.timeline, Path::new(&soundfont_path))
            .map(Arc::new)
            .map_err(|e| match e {
                SynthError::NotFound(_) => CoreError::NotFound,
                e => CoreError::Library(e.to_string()),
            })
    }

    /// Score seconds for a click in layout coordinates. Picks the occurrence of the clicked bar
    /// nearest to `near_score_secs` (the current position), so clicking inside a repeat stays on
    /// the current pass.
    pub fn score_secs_at_point(&self, x: f32, y: f32, near_score_secs: f64) -> Option<f64> {
        let (master_bar, tick) = layout::hit_test(&self.state.lock().layout, x, y)?;
        let bars = self.timeline.played_bars();
        let bar = bars
            .iter()
            .filter(|b| b.master_bar == master_bar)
            .min_by(|a, b| {
                let da = (a.start_secs - near_score_secs).abs();
                let db = (b.start_secs - near_score_secs).abs();
                da.total_cmp(&db)
            })?;
        Some(self.timeline.tick_to_secs(bar.start_tick + u64::from(tick)))
    }
}

/// Parse a YouTube URL or bare id into its 11-character id.
#[uniffi::export]
pub fn youtube_id(input: String) -> Option<String> {
    library::parse_youtube_id(&input)
}

/// Sync points from a per-bar `points` array (one video time per played bar,
/// in played order; a trailing end time is ignored).
#[uniffi::export]
pub fn bar_points_sync(points: Vec<f64>) -> SyncSpec {
    sync::from_bar_points(&points)
}

#[cfg(test)]
mod track_label_tests {
    use super::track_label;
    use crate::model::{Instrument, Track};

    fn track(name: &str, program: u8, tuning: &[u8]) -> Track {
        Track {
            name: name.into(),
            program,
            tuning: tuning.to_vec(),
            ..Default::default()
        }
    }

    const GUITAR: &[u8] = &[64, 59, 55, 50, 45, 40];
    const BASS: &[u8] = &[43, 38, 33, 28];

    #[test]
    fn names_the_instrument_not_the_player() {
        let tracks = [track("Flea", 33, BASS)];
        assert_eq!(track_label(&tracks, 0), "Bass · E A D G");
    }

    #[test]
    fn numbers_duplicates_and_shows_capo() {
        let mut capo = track("Rhythm", 29, GUITAR);
        capo.capo = 2;
        let tracks = [track("Lead", 30, GUITAR), track("Bass", 34, BASS), capo];
        assert_eq!(track_label(&tracks, 0), "Guitar 1 · E A D G B E");
        assert_eq!(track_label(&tracks, 1), "Bass · E A D G");
        assert_eq!(track_label(&tracks, 2), "Guitar 2 · E A D G B E · capo 2");
    }

    #[test]
    fn file_hint_beats_program_and_tuning_decides_last() {
        let mut hinted = track("x", 0, GUITAR);
        hinted.instrument_hint = Some(Instrument::Bass);
        assert_eq!(hinted.instrument(), Instrument::Bass);
        assert_eq!(track("x", 105, BASS).instrument(), Instrument::Bass);
        assert_eq!(track("x", 105, GUITAR).instrument(), Instrument::Guitar);
        let drums = Track {
            is_percussion: true,
            ..track("Drums", 0, &[0; 6])
        };
        assert_eq!(track_label(&[drums], 0), "Drums");
    }

    #[test]
    fn unknown_instruments_keep_their_name() {
        let tracks = [track("Violin", 40, &[]), track("", 40, &[])];
        assert_eq!(track_label(&tracks, 0), "Violin");
        assert_eq!(track_label(&tracks, 1), "Track 2");
    }

    #[test]
    fn gpif_refs() {
        for (r, i) in [
            ("e-gtr6", Instrument::Guitar),
            ("a-gtr7", Instrument::Guitar),
            ("e-bass4", Instrument::Bass),
            ("drmkt", Instrument::Drums),
            ("pno-gs", Instrument::Keys),
        ] {
            assert_eq!(Instrument::from_gpif_ref(r), Some(i), "{r}");
        }
        assert_eq!(Instrument::from_gpif_ref("vln"), None);
    }
}
