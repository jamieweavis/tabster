//! Tabster core: everything the macOS shell needs, exposed over UniFFI as the [`Core`] object
//! (library management) and [`TabSession`] objects (one open tab: layout, timing, sync).

uniffi::setup_scaffolding!();

pub mod config;
pub mod formats;
pub mod layout;
pub mod library;
pub mod logging;
pub mod model;
pub mod sync;
pub mod timeline;

use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::{Mutex, RwLock};

use crate::layout::{Cursor, DisplayList, LayoutOptions};
use crate::library::{
    ImportOutcome, Library, LibraryError, PracticeState, Setlist, TabFilter, TabId, TabSummary,
    Video, VideoId,
};
use crate::model::Score;
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

#[derive(uniffi::Object)]
pub struct Core {
    library: Library,
    observer: RwLock<Option<Arc<dyn LibraryObserver>>>,
}

#[uniffi::export]
impl Core {
    /// `data_dir`: the app's data directory (`~/Library/Application Support/Tabster`).
    #[uniffi::constructor]
    pub fn new(data_dir: String) -> Result<Arc<Self>, CoreError> {
        logging::init();
        let library = Library::open(&PathBuf::from(data_dir))?;
        Ok(Arc::new(Self {
            library,
            observer: RwLock::new(None),
        }))
    }

    pub fn version(&self) -> String {
        env!("CARGO_PKG_VERSION").to_string()
    }

    pub fn set_library_observer(&self, observer: Option<Arc<dyn LibraryObserver>>) {
        *self.observer.write() = observer;
    }

    /// Import files (and the tab files inside any folders, recursively). Blocking: call from a
    /// background task. Reports progress through the observer.
    pub fn import_paths(&self, paths: Vec<String>) -> Vec<ImportOutcome> {
        // Phase 2 agent I: parallelise with rayon and expand folders.
        let total = paths.len() as u32;
        let observer = self.observer.read().clone();
        let mut outcomes = Vec::with_capacity(paths.len());
        for (i, path) in paths.iter().enumerate() {
            outcomes.push(self.library.import_file(&PathBuf::from(path)));
            if let Some(o) = &observer {
                o.import_progress(i as u32 + 1, total);
            }
        }
        self.changed();
        outcomes
    }

    pub fn list_tabs(&self, filter: TabFilter) -> Result<Vec<TabSummary>, CoreError> {
        Ok(self.library.list(&filter)?)
    }

    pub fn tab(&self, id: TabId) -> Result<TabSummary, CoreError> {
        Ok(self.library.get(id)?)
    }

    pub fn tab_file_path(&self, id: TabId) -> Result<String, CoreError> {
        Ok(self.library.file_path(id)?.to_string_lossy().into_owned())
    }

    pub fn delete_tab(&self, id: TabId) -> Result<(), CoreError> {
        self.library.delete(id)?;
        self.changed();
        Ok(())
    }

    pub fn set_favourite(&self, id: TabId, favourite: bool) -> Result<(), CoreError> {
        self.library.set_favourite(id, favourite)?;
        self.changed();
        Ok(())
    }

    pub fn set_tags(&self, id: TabId, tags: Vec<String>) -> Result<(), CoreError> {
        self.library.set_tags(id, &tags)?;
        self.changed();
        Ok(())
    }

    pub fn tags(&self) -> Result<Vec<String>, CoreError> {
        Ok(self.library.tags()?)
    }

    pub fn setlists(&self) -> Result<Vec<Setlist>, CoreError> {
        Ok(self.library.setlists()?)
    }

    pub fn create_setlist(&self, name: String) -> Result<i64, CoreError> {
        let id = self.library.create_setlist(&name)?;
        self.changed();
        Ok(id)
    }

    pub fn delete_setlist(&self, id: i64) -> Result<(), CoreError> {
        self.library.delete_setlist(id)?;
        self.changed();
        Ok(())
    }

    pub fn set_setlist_tabs(&self, id: i64, tabs: Vec<TabId>) -> Result<(), CoreError> {
        self.library.set_setlist_tabs(id, &tabs)?;
        self.changed();
        Ok(())
    }

    pub fn videos(&self, tab: TabId) -> Result<Vec<Video>, CoreError> {
        Ok(self.library.videos(tab)?)
    }

    /// Attach a YouTube video given a URL or bare id.
    pub fn add_video(
        &self,
        tab: TabId,
        url_or_id: String,
        label: String,
    ) -> Result<VideoId, CoreError> {
        let youtube_id = library::parse_youtube_id(&url_or_id).ok_or(CoreError::InvalidVideo)?;
        let id = self.library.add_video(tab, &youtube_id, &label)?;
        self.changed();
        Ok(id)
    }

    pub fn remove_video(&self, id: VideoId) -> Result<(), CoreError> {
        self.library.remove_video(id)?;
        self.changed();
        Ok(())
    }

    pub fn set_default_video(&self, id: VideoId) -> Result<(), CoreError> {
        Ok(self.library.set_default_video(id)?)
    }

    pub fn set_video_sync(&self, id: VideoId, sync: SyncSpec) -> Result<(), CoreError> {
        Ok(self.library.set_video_sync(id, &sync)?)
    }

    pub fn practice_state(&self, tab: TabId) -> Result<PracticeState, CoreError> {
        Ok(self.library.practice_state(tab)?)
    }

    pub fn set_practice_state(&self, tab: TabId, state: PracticeState) -> Result<(), CoreError> {
        Ok(self.library.set_practice_state(tab, &state)?)
    }

    /// Open a tab for viewing and playback.
    pub fn open_tab(&self, id: TabId) -> Result<Arc<TabSession>, CoreError> {
        let score = self.library.load_score(id)?;
        Ok(Arc::new(TabSession::new(id, score)))
    }
}

impl Core {
    fn changed(&self) {
        if let Some(o) = self.observer.read().clone() {
            o.library_changed();
        }
    }
}

// ---------------------------------------------------------------------------------------------
// TabSession
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct TrackInfo {
    pub index: u32,
    pub name: String,
    pub strings: u32,
    /// Note names, string 1 (highest) first, e.g. ["E", "B", "G", "D", "A", "E"].
    pub tuning: Vec<String>,
    pub capo: u32,
    pub is_percussion: bool,
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
}

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
            }),
        }
    }

    pub fn score(&self) -> &Score {
        &self.score
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
                name: t.name.clone(),
                strings: t.tuning.len() as u32,
                tuning: t.tuning.iter().map(|&p| note_name(p).to_string()).collect(),
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
        Some(PlaybackPosition {
            score_secs,
            played_bar: pos.played_bar as u32,
            master_bar: pos.master_bar,
            cursor: layout::cursor(&state.layout, pos.master_bar, pos.tick_in_bar, bar_ticks),
        })
    }

    pub fn position_at_video(&self, video_secs: f64) -> Option<PlaybackPosition> {
        let score_secs = self.video_to_score(video_secs);
        self.position_at_score(score_secs)
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

/// Note name of a MIDI pitch, sharps only.
fn note_name(pitch: u8) -> &'static str {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    NAMES[usize::from(pitch % 12)]
}

/// Parse a YouTube URL or bare id into its 11-character id.
#[uniffi::export]
pub fn youtube_id(input: String) -> Option<String> {
    library::parse_youtube_id(&input)
}
