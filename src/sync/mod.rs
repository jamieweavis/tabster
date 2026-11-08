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
    /// (score_secs, video_secs), strictly increasing in both. Always at least one anchor
    /// (except for `SyncMap::default()`, which behaves as the identity).
    anchors: Vec<(f64, f64)>,
    /// d(video)/d(score) before the first anchor and after the last one.
    slope_before: f64,
    slope_after: f64,
}

/// Evaluate the piecewise-linear map through `anchors` (projected with `from`/`to`), with
/// linear extrapolation outside them.
#[inline]
fn eval(
    anchors: &[(f64, f64)],
    x: f64,
    from: impl Fn(&(f64, f64)) -> f64,
    to: impl Fn(&(f64, f64)) -> f64,
    slope_before: f64,
    slope_after: f64,
) -> f64 {
    let (Some(first), Some(last)) = (anchors.first(), anchors.last()) else {
        return x;
    };
    let k = anchors.partition_point(|a| from(a) <= x);
    if k == 0 {
        to(first) + (x - from(first)) * slope_before
    } else if k == anchors.len() {
        to(last) + (x - from(last)) * slope_after
    } else {
        let (p, q) = (&anchors[k - 1], &anchors[k]);
        to(p) + (x - from(p)) * (to(q) - to(p)) / (from(q) - from(p))
    }
}

/// Indices of a longest strictly increasing subsequence of `values` (O(n log n)).
fn longest_increasing(values: &[f64]) -> Vec<usize> {
    // tails[l] = index of the smallest tail of an increasing subsequence of length l + 1.
    let mut tails: Vec<usize> = Vec::new();
    let mut prev: Vec<Option<usize>> = vec![None; values.len()];
    for (i, &v) in values.iter().enumerate() {
        let l = tails.partition_point(|&t| values[t] < v);
        prev[i] = l.checked_sub(1).map(|j| tails[j]);
        if l == tails.len() {
            tails.push(i);
        } else {
            tails[l] = i;
        }
    }
    let mut out = Vec::with_capacity(tails.len());
    let mut cur = tails.last().copied();
    while let Some(i) = cur {
        out.push(i);
        cur = prev[i];
    }
    out.reverse();
    out
}

impl SyncMap {
    /// Build the map. Points referring to played bars past the end are ignored; points that
    /// would make the map non-monotonic are dropped.
    ///
    /// - Offset mode: a non-finite offset counts as 0; a stretch ≤ 0 or non-finite as 1.0.
    /// - Points mode: points are sorted by played bar; for duplicate played bars the first one
    ///   wins; non-finite video times are dropped. Of the rest, the longest subsequence with
    ///   strictly increasing video times is kept (so a single mistyped point is dropped rather
    ///   than everything after it). No usable points → identity (`Offset { 0, 1 }`).
    pub fn new(spec: &SyncSpec, timeline: &Timeline) -> Self {
        match spec {
            SyncSpec::Offset {
                offset_secs,
                stretch,
            } => {
                let offset = if offset_secs.is_finite() {
                    *offset_secs
                } else {
                    0.0
                };
                let stretch = if stretch.is_finite() && *stretch > 0.0 {
                    *stretch
                } else {
                    1.0
                };
                Self {
                    anchors: vec![(0.0, offset)],
                    slope_before: stretch,
                    slope_after: stretch,
                }
            }
            SyncSpec::Points { points } => {
                let bars = timeline.played_bars();
                let mut pts: Vec<SyncPoint> = points
                    .iter()
                    .copied()
                    .filter(|p| (p.played_bar as usize) < bars.len() && p.video_secs.is_finite())
                    .collect();
                pts.sort_by_key(|p| p.played_bar);
                pts.dedup_by_key(|p| p.played_bar);
                // Score seconds of played-bar starts are strictly increasing, so only video
                // times can violate monotonicity.
                let videos: Vec<f64> = pts.iter().map(|p| p.video_secs).collect();
                let anchors: Vec<(f64, f64)> = longest_increasing(&videos)
                    .into_iter()
                    .map(|i| {
                        (
                            bars[pts[i].played_bar as usize].start_secs,
                            pts[i].video_secs,
                        )
                    })
                    .collect();
                if anchors.is_empty() {
                    return Self::new(&SyncSpec::default(), timeline);
                }
                Self {
                    anchors,
                    slope_before: 1.0,
                    slope_after: 1.0,
                }
            }
        }
    }

    pub fn score_to_video(&self, score_secs: f64) -> f64 {
        eval(
            &self.anchors,
            score_secs,
            |a| a.0,
            |a| a.1,
            self.slope_before,
            self.slope_after,
        )
    }

    pub fn video_to_score(&self, video_secs: f64) -> f64 {
        eval(
            &self.anchors,
            video_secs,
            |a| a.1,
            |a| a.0,
            1.0 / self.slope_before,
            1.0 / self.slope_after,
        )
    }

    /// Expand any spec into explicit per-played-bar points (used when switching the editor
    /// from offset mode to points mode).
    pub fn to_points(&self, timeline: &Timeline) -> Vec<SyncPoint> {
        timeline
            .played_bars()
            .iter()
            .enumerate()
            .map(|(i, b)| SyncPoint {
                played_bar: i as u32,
                video_secs: self.score_to_video(b.start_secs),
            })
            .collect()
    }
}

/// Points from a per-bar `points` array: one video timestamp per played bar,
/// in played order. A trailing extra value (end time) is accepted and ignored.
///
/// The bar count isn't known here, so every value becomes a point (`played_bar` = its index).
/// A trailing end-time value lands on `played_bar == played bar count`, which
/// [`SyncMap::new`] drops as out of range.
pub fn from_bar_points(points: &[f64]) -> SyncSpec {
    SyncSpec::Points {
        points: points
            .iter()
            .enumerate()
            .map(|(i, &video_secs)| SyncPoint {
                played_bar: i as u32,
                video_secs,
            })
            .collect(),
    }
}

/// Points from the sync points stored in a score file (GP8), resolved against the timeline.
/// `None` if the score has none.
///
/// File points may sit anywhere inside a bar; a [`SyncPoint`] marks a bar start, so a mid-bar
/// point is projected back to its bar's start using the video/score slope towards the next
/// file point (or from the previous one for the last point; slope 1 if there's only one).
/// Points whose (bar, occurrence) is never played are dropped; `None` if nothing remains.
pub fn from_file_sync_points(score: &Score, timeline: &Timeline) -> Option<SyncSpec> {
    let bars = timeline.played_bars();
    // (played bar, score secs of the point, video secs, score secs of the bar start)
    let mut resolved: Vec<(usize, f64, f64, f64)> = score
        .sync_points
        .iter()
        .filter(|p| p.millisecond_offset.is_finite())
        .filter_map(|p| {
            let index = timeline.played_index(p.bar, p.bar_occurrence)?;
            let bar = &bars[index];
            let fraction = if p.bar_position.is_finite() {
                p.bar_position.clamp(0.0, 1.0)
            } else {
                0.0
            };
            let len = (bar.end_tick - bar.start_tick) as f64;
            let tick = bar.start_tick + (fraction * len).round() as u64;
            Some((
                index,
                timeline.tick_to_secs(tick),
                p.millisecond_offset / 1000.0,
                bar.start_secs,
            ))
        })
        .collect();
    if resolved.is_empty() {
        return None;
    }
    resolved.sort_by(|a, b| a.1.total_cmp(&b.1));
    let slope_between = |a: &(usize, f64, f64, f64), b: &(usize, f64, f64, f64)| {
        let slope = (b.2 - a.2) / (b.1 - a.1);
        (slope.is_finite() && slope > 0.0).then_some(slope)
    };
    let points = (0..resolved.len())
        .map(|i| {
            let p = &resolved[i];
            let slope = resolved
                .get(i + 1)
                .and_then(|next| slope_between(p, next))
                .or_else(|| {
                    i.checked_sub(1)
                        .and_then(|j| slope_between(&resolved[j], p))
                })
                .unwrap_or(1.0);
            SyncPoint {
                played_bar: p.0 as u32,
                video_secs: p.2 - (p.1 - p.3) * slope,
            }
        })
        .collect();
    Some(SyncSpec::Points { points })
}

#[cfg(test)]
mod tests;
