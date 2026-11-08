//! The tab library: SQLite catalogue (WAL, migrations via `PRAGMA user_version`, FTS5 search),
//! a content-addressed file store (`<data dir>/tabs/<sha256>.<ext>`), a parsed-score cache
//! (`<data dir>/cache/<sha256>.bin`, postcard), tags, setlists, attached videos and their sync.
//!
//! Frozen contract: the public types and `Library` method signatures. Owner: Phase 1 agent E.
//! The database schema is the agent's to design (see PLAN.md §1 for the intended shape).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::formats::{Format, ParseError};
use crate::model::Score;
use crate::sync::SyncSpec;

pub type TabId = i64;
pub type VideoId = i64;

#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    #[error("database error: {0}")]
    Db(String),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Parse(#[from] ParseError),
    #[error("not found")]
    NotFound,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct TabSummary {
    pub id: TabId,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub format: Format,
    /// e.g. "E A D G B E", "Drop D". Of the first non-percussion track.
    pub tuning: String,
    pub track_count: u32,
    pub bar_count: u32,
    pub duration_secs: f64,
    pub favourite: bool,
    pub has_video: bool,
    pub tags: Vec<String>,
    /// Unix seconds.
    pub imported_at: i64,
    pub last_opened_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, uniffi::Enum)]
pub enum SortOrder {
    #[default]
    Title,
    Artist,
    RecentlyAdded,
    RecentlyOpened,
}

#[derive(Debug, Clone, Default, PartialEq, uniffi::Record)]
pub struct TabFilter {
    /// Full-text search over title/artist/album; empty = all.
    pub query: String,
    pub favourites_only: bool,
    pub tag: Option<String>,
    pub setlist: Option<i64>,
    pub sort: SortOrder,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ImportOutcome {
    pub path: String,
    /// The tab's id (also set when the file was a duplicate of an existing tab).
    pub id: Option<TabId>,
    pub duplicate: bool,
    /// Human-readable reason when the import failed.
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Video {
    pub id: VideoId,
    pub tab_id: TabId,
    /// The 11-character YouTube video id.
    pub youtube_id: String,
    pub label: String,
    pub is_default: bool,
    pub sync: SyncSpec,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Setlist {
    pub id: i64,
    pub name: String,
    pub tab_count: u32,
}

/// Per-tab practice state, restored when a tab is reopened.
#[derive(Debug, Clone, Default, PartialEq, uniffi::Record)]
pub struct PracticeState {
    pub track: u32,
    pub speed: f64,
    /// Inclusive master-bar range.
    pub loop_start_bar: Option<u32>,
    pub loop_end_bar: Option<u32>,
    pub last_position_secs: f64,
}

pub struct Library {
    data_dir: PathBuf,
}

impl Library {
    /// Open (creating and migrating if needed) the library in `data_dir`.
    pub fn open(data_dir: &Path) -> Result<Self, LibraryError> {
        // Phase 0 placeholder so the shell can start; agent E replaces this.
        std::fs::create_dir_all(data_dir)?;
        Ok(Self {
            data_dir: data_dir.to_path_buf(),
        })
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Import one file: hash, dedupe, copy into the store, parse, catalogue, cache. Safe to call
    /// from several threads at once.
    pub fn import_file(&self, path: &Path) -> ImportOutcome {
        let _ = path;
        todo!("Phase 1 agent E")
    }

    pub fn list(&self, filter: &TabFilter) -> Result<Vec<TabSummary>, LibraryError> {
        let _ = filter;
        Ok(Vec::new()) // Phase 0 placeholder; agent E replaces this.
    }

    pub fn get(&self, id: TabId) -> Result<TabSummary, LibraryError> {
        let _ = id;
        todo!("Phase 1 agent E")
    }

    /// Load the parsed score (from cache, re-parsing the stored file on a cache miss) and mark
    /// the tab as opened.
    pub fn load_score(&self, id: TabId) -> Result<Score, LibraryError> {
        let _ = id;
        todo!("Phase 1 agent E")
    }

    /// Path of the stored tab file.
    pub fn file_path(&self, id: TabId) -> Result<PathBuf, LibraryError> {
        let _ = id;
        todo!("Phase 1 agent E")
    }

    pub fn delete(&self, id: TabId) -> Result<(), LibraryError> {
        let _ = id;
        todo!("Phase 1 agent E")
    }

    pub fn set_favourite(&self, id: TabId, favourite: bool) -> Result<(), LibraryError> {
        let _ = (id, favourite);
        todo!("Phase 1 agent E")
    }

    /// Replace the tab's tags.
    pub fn set_tags(&self, id: TabId, tags: &[String]) -> Result<(), LibraryError> {
        let _ = (id, tags);
        todo!("Phase 1 agent E")
    }

    /// All tags in use, alphabetical.
    pub fn tags(&self) -> Result<Vec<String>, LibraryError> {
        todo!("Phase 1 agent E")
    }

    pub fn setlists(&self) -> Result<Vec<Setlist>, LibraryError> {
        todo!("Phase 1 agent E")
    }

    pub fn create_setlist(&self, name: &str) -> Result<i64, LibraryError> {
        let _ = name;
        todo!("Phase 1 agent E")
    }

    pub fn delete_setlist(&self, id: i64) -> Result<(), LibraryError> {
        let _ = id;
        todo!("Phase 1 agent E")
    }

    /// Replace the setlist's contents with `tabs`, in order.
    pub fn set_setlist_tabs(&self, id: i64, tabs: &[TabId]) -> Result<(), LibraryError> {
        let _ = (id, tabs);
        todo!("Phase 1 agent E")
    }

    pub fn videos(&self, tab: TabId) -> Result<Vec<Video>, LibraryError> {
        let _ = tab;
        todo!("Phase 1 agent E")
    }

    /// Attach a video. The first video attached to a tab becomes its default.
    pub fn add_video(
        &self,
        tab: TabId,
        youtube_id: &str,
        label: &str,
    ) -> Result<VideoId, LibraryError> {
        let _ = (tab, youtube_id, label);
        todo!("Phase 1 agent E")
    }

    pub fn remove_video(&self, id: VideoId) -> Result<(), LibraryError> {
        let _ = id;
        todo!("Phase 1 agent E")
    }

    pub fn set_default_video(&self, id: VideoId) -> Result<(), LibraryError> {
        let _ = id;
        todo!("Phase 1 agent E")
    }

    pub fn set_video_sync(&self, id: VideoId, sync: &SyncSpec) -> Result<(), LibraryError> {
        let _ = (id, sync);
        todo!("Phase 1 agent E")
    }

    pub fn practice_state(&self, tab: TabId) -> Result<PracticeState, LibraryError> {
        let _ = tab;
        todo!("Phase 1 agent E")
    }

    pub fn set_practice_state(
        &self,
        tab: TabId,
        state: &PracticeState,
    ) -> Result<(), LibraryError> {
        let _ = (tab, state);
        todo!("Phase 1 agent E")
    }
}

/// Extract the 11-character video id from a YouTube URL (watch, youtu.be, shorts, embed,
/// music.youtube.com) or a bare id.
pub fn parse_youtube_id(input: &str) -> Option<String> {
    let _ = input;
    todo!("Phase 1 agent E")
}
