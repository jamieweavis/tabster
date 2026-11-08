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
//!
//! See [`unroll`] for the repeat/ending/jump semantics.

use crate::model::{Score, TICKS_PER_QUARTER};

#[cfg(test)]
pub(crate) mod test_scores;
#[cfg(test)]
mod tests;
pub mod unroll;

/// Fallback tempo when the score's is missing or invalid.
const DEFAULT_BPM: f64 = 120.0;
/// Fallback bar length when neither the beats nor the time signature give one (4/4).
const DEFAULT_BAR_TICKS: u64 = 4 * TICKS_PER_QUARTER as u64;

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

/// A constant-tempo stretch of the played timeline, from `tick` until the next segment.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Segment {
    tick: u64,
    secs: f64,
    secs_per_tick: f64,
}

impl Segment {
    fn secs_at(&self, tick: u64) -> f64 {
        self.secs + (tick - self.tick) as f64 * self.secs_per_tick
    }
}

#[derive(Debug, Clone, Default)]
pub struct Timeline {
    bars: Vec<PlayedBar>,
    /// Tempo segments, strictly increasing in `tick` and `secs`; the first starts at tick 0.
    /// Empty iff `bars` is empty.
    segments: Vec<Segment>,
    /// For each master bar, the indices of its played bars in order of occurrence.
    occurrences: Vec<Vec<usize>>,
}

/// Valid tempos are clamped to this range, so score seconds stay finite and strictly increasing
/// however absurd the file's tempo (e.g. GPIF's `1e-300`).
const MIN_BPM: f64 = 1.0;
const MAX_BPM: f64 = 10_000.0;

fn secs_per_tick(bpm: f64) -> f64 {
    60.0 / (bpm.clamp(MIN_BPM, MAX_BPM) * f64::from(TICKS_PER_QUARTER))
}

fn valid_bpm(bpm: f64) -> bool {
    bpm.is_finite() && bpm > 0.0
}

/// Actual length of a master bar: the longest track's beats, else the time signature.
fn bar_ticks(score: &Score, index: usize) -> u64 {
    let actual = score
        .tracks
        .iter()
        .filter_map(|t| t.bars.get(index))
        .map(|b| u64::from(b.ticks()))
        .max()
        .unwrap_or(0);
    if actual > 0 {
        return actual;
    }
    let nominal = u64::from(score.master_bars[index].time_signature.bar_ticks());
    if nominal > 0 {
        nominal
    } else {
        DEFAULT_BAR_TICKS
    }
}

impl Timeline {
    /// Build the timeline. Never fails: malformed repeats and jumps degrade to linear playback,
    /// and unrolling is capped so a broken jump can't loop forever.
    ///
    /// Tempo changes take effect every time their bar is played; the tempo then carries over to
    /// whatever bar is played next (including after a repeat or jump). Invalid tempos (≤ 0,
    /// non-finite) are ignored; an invalid initial tempo becomes 120. Valid tempos are clamped to
    /// 1–10,000 bpm.
    pub fn new(score: &Score) -> Self {
        let n = score.master_bars.len();
        let order = unroll::unroll(&score.master_bars);
        let lengths: Vec<u64> = (0..n).map(|i| bar_ticks(score, i)).collect();

        let mut changes: Vec<Vec<(f64, f64)>> = vec![Vec::new(); n];
        for tc in &score.tempo_changes {
            let Some(list) = changes.get_mut(tc.bar as usize) else {
                continue;
            };
            if valid_bpm(tc.bpm) {
                let pos = if tc.position.is_finite() {
                    tc.position.clamp(0.0, 1.0)
                } else {
                    0.0
                };
                list.push((pos, tc.bpm));
            }
        }
        for list in &mut changes {
            list.sort_by(|a, b| a.0.total_cmp(&b.0));
        }

        let initial_bpm = if valid_bpm(score.initial_tempo) {
            score.initial_tempo
        } else {
            DEFAULT_BPM
        };
        let mut current = Segment {
            tick: 0,
            secs: 0.0,
            secs_per_tick: secs_per_tick(initial_bpm),
        };

        let mut bars = Vec::with_capacity(order.len());
        let mut segments = Vec::new();
        let mut occurrences = vec![Vec::new(); n];
        if !order.is_empty() {
            segments.push(current);
        }
        let mut tick = 0u64;
        for &m in &order {
            let m_idx = m as usize;
            let len = lengths[m_idx];
            let start_tick = tick;
            let end_tick = start_tick + len;
            let start_secs = current.secs_at(start_tick);
            for &(pos, bpm) in &changes[m_idx] {
                let at = start_tick + ((pos * len as f64).round() as u64).min(len);
                current = Segment {
                    tick: at,
                    secs: current.secs_at(at),
                    secs_per_tick: secs_per_tick(bpm),
                };
                match segments.last_mut() {
                    Some(last) if last.tick == at => *last = current,
                    _ => segments.push(current),
                }
            }
            let occ = &mut occurrences[m_idx];
            bars.push(PlayedBar {
                master_bar: m,
                occurrence: occ.len() as u32,
                start_tick,
                end_tick,
                start_secs,
                end_secs: current.secs_at(end_tick),
            });
            occ.push(bars.len() - 1);
            tick = end_tick;
        }
        Self {
            bars,
            segments,
            occurrences,
        }
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
        if self.segments.is_empty() {
            return 0.0;
        }
        let tick = tick.min(self.total_ticks());
        let k = self.segments.partition_point(|s| s.tick <= tick);
        self.segments[k.saturating_sub(1)].secs_at(tick)
    }

    /// Played tick at score seconds. Clamped to the timeline. Rounds down to the tick that
    /// contains `secs` (with a tiny tolerance so `secs_to_tick(tick_to_secs(t)) == t`).
    pub fn secs_to_tick(&self, secs: f64) -> u64 {
        if self.segments.is_empty() || secs.is_nan() || secs <= 0.0 {
            return 0;
        }
        let total = self.total_ticks();
        if secs >= self.duration_secs() {
            return total;
        }
        let k = self.segments.partition_point(|s| s.secs <= secs);
        let seg = &self.segments[k.saturating_sub(1)];
        let offset = ((secs - seg.secs) / seg.secs_per_tick + 1e-6).floor();
        (seg.tick + offset.max(0.0) as u64).min(total)
    }

    /// Position at score seconds. `None` only if the timeline is empty. Clamped to the timeline:
    /// at or past the end, it's the end of the last bar (`fraction` 1.0).
    pub fn position_at_secs(&self, secs: f64) -> Option<TimelinePosition> {
        let last = self.bars.len().checked_sub(1)?;
        let tick = self.secs_to_tick(secs);
        let index = self.bars.partition_point(|b| b.end_tick <= tick).min(last);
        let bar = &self.bars[index];
        let len = bar.end_tick - bar.start_tick;
        let tick_in_bar = tick.saturating_sub(bar.start_tick).min(len);
        Some(TimelinePosition {
            played_bar: index,
            master_bar: bar.master_bar,
            tick_in_bar: tick_in_bar as u32,
            fraction: tick_in_bar as f64 / len as f64,
        })
    }

    /// Index of the played bar for (master bar, occurrence), if it's played that many times.
    pub fn played_index(&self, master_bar: u32, occurrence: u32) -> Option<usize> {
        self.occurrences
            .get(master_bar as usize)?
            .get(occurrence as usize)
            .copied()
    }
}
