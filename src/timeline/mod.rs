//! The played timeline: repeats, alternate endings and D.S./D.C./coda jumps unrolled into a
//! linear sequence of *played bars*, plus the tempo map for converting between score ticks and
//! score seconds.
//!
//! Two time axes exist:
//! - **played ticks**: ticks along the unrolled performance, starting at 0 at the start of the
//!   first played bar.
//! - **score seconds**: wall time of the performance at the score's own tempo (speed 1.0).
//!
//! Video time is a third axis, handled by `crate::sync`.
//!
//! Frozen contract: the public API below. Owner of the implementation: Phase 1 agent C.

use crate::model::Score;

/// One bar as actually played, after unrolling repeats and jumps.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayedBar {
    /// Index into `Score::master_bars`.
    pub master_bar: u32,
    /// Which pass through this master bar (0 = first time it's played).
    pub occurrence: u32,
    pub start_tick: u64,
    /// Exclusive. `end_tick - start_tick` is the bar's actual length (a pickup bar is shorter).
    pub end_tick: u64,
    pub start_secs: f64,
    pub end_secs: f64,
}

/// A position on the played timeline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimelinePosition {
    /// Index into `Timeline::played_bars()`.
    pub played_bar: usize,
    pub master_bar: u32,
    /// Ticks from the start of the bar.
    pub tick_in_bar: u32,
    /// Fraction 0.0..1.0 through the bar, in ticks.
    pub fraction: f64,
}

#[derive(Debug, Clone, Default)]
pub struct Timeline {
    bars: Vec<PlayedBar>,
}

impl Timeline {
    /// Build the timeline. Never fails: malformed repeats and jumps degrade to linear playback,
    /// and unrolling is capped so a broken jump can't loop forever.
    pub fn new(score: &Score) -> Self {
        let _ = score;
        todo!("Phase 1 agent C")
    }

    pub fn played_bars(&self) -> &[PlayedBar] {
        &self.bars
    }

    pub fn total_ticks(&self) -> u64 {
        self.bars.last().map_or(0, |b| b.end_tick)
    }

    pub fn duration_secs(&self) -> f64 {
        self.bars.last().map_or(0.0, |b| b.end_secs)
    }

    /// Score seconds at a played tick. Clamped to the timeline.
    pub fn tick_to_secs(&self, tick: u64) -> f64 {
        let _ = tick;
        todo!("Phase 1 agent C")
    }

    /// Played tick at score seconds. Clamped to the timeline.
    pub fn secs_to_tick(&self, secs: f64) -> u64 {
        let _ = secs;
        todo!("Phase 1 agent C")
    }

    /// Position at score seconds. `None` only if the timeline is empty. Clamped to the timeline.
    pub fn position_at_secs(&self, secs: f64) -> Option<TimelinePosition> {
        let _ = secs;
        todo!("Phase 1 agent C")
    }

    /// Index of the played bar for (master bar, occurrence), if it's played that many times.
    pub fn played_index(&self, master_bar: u32, occurrence: u32) -> Option<usize> {
        let _ = (master_bar, occurrence);
        todo!("Phase 1 agent C")
    }
}
