//! Mapping between score time and video time.
//!
//! A [`SyncMap`] is a monotonic, piecewise-linear function from score seconds (see
//! `crate::timeline`) to video seconds, and its inverse. It's built from a [`SyncSpec`], the
//! persisted form, which is one of:
//! - **Offset**: `video = offset + score_secs * stretch`. A single "tap bar 1" sets `offset`;
//!   `stretch` corrects recordings that are uniformly slightly fast or slow (1.0 = exact).
//! - **Points**: one video timestamp per *played bar* start (the model popular tab players use; the same idea
//!   as GP8 sync points). Between points, time is interpolated linearly. Before the first and
//!   after the last point, the map extrapolates at the score's tempo (slope 1).
//!
//! Frozen contract: the public API below. Owner of the implementation: Phase 1 agent C.

use serde::{Deserialize, Serialize};

use crate::model::Score;
use crate::timeline::Timeline;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Enum)]
pub enum SyncSpec {
    Offset { offset_secs: f64, stretch: f64 },
    Points { points: Vec<SyncPoint> },
}

impl Default for SyncSpec {
    fn default() -> Self {
        SyncSpec::Offset {
            offset_secs: 0.0,
            stretch: 1.0,
        }
    }
}

/// The video time at which a played bar starts.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct SyncPoint {
    /// Index into `Timeline::played_bars()`.
    pub played_bar: u32,
    pub video_secs: f64,
}

#[derive(Debug, Clone, Default)]
pub struct SyncMap {
    /// (score_secs, video_secs), strictly increasing in both. Always at least one anchor.
    #[allow(dead_code)] // Phase 0 stub; agent C removes this.
    anchors: Vec<(f64, f64)>,
}

impl SyncMap {
    /// Build the map. Points referring to played bars past the end are ignored; points that
    /// would make the map non-monotonic are dropped.
    pub fn new(spec: &SyncSpec, timeline: &Timeline) -> Self {
        let _ = (spec, timeline);
        todo!("Phase 1 agent C")
    }

    pub fn score_to_video(&self, score_secs: f64) -> f64 {
        let _ = score_secs;
        todo!("Phase 1 agent C")
    }

    pub fn video_to_score(&self, video_secs: f64) -> f64 {
        let _ = video_secs;
        todo!("Phase 1 agent C")
    }

    /// Expand any spec into explicit per-played-bar points (used when switching the editor
    /// from offset mode to points mode).
    pub fn to_points(&self, timeline: &Timeline) -> Vec<SyncPoint> {
        let _ = timeline;
        todo!("Phase 1 agent C")
    }
}

/// Points from a per-bar `points` array: one video timestamp per played bar,
/// in played order. A trailing extra value (end time) is accepted and ignored.
pub fn from_bar_points(points: &[f64]) -> SyncSpec {
    let _ = points;
    todo!("Phase 1 agent C")
}

/// Points from the sync points stored in a score file (GP8), resolved against the timeline.
/// `None` if the score has none.
pub fn from_file_sync_points(score: &Score, timeline: &Timeline) -> Option<SyncSpec> {
    let _ = (score, timeline);
    todo!("Phase 1 agent C")
}
