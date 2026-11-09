//! Lookups against a finished [`DisplayList`]: the playback cursor and click hit-testing.
//! Both rely on systems being ordered top to bottom and bars by master bar index (each master bar
//! appears once), so they are binary searches and never allocate.

use super::{BarBox, Cursor, DisplayList, Rect};

/// The system index and laid-out bar for a master bar.
fn find_bar(list: &DisplayList, master_bar: u32) -> Option<(usize, &BarBox)> {
    let si = list
        .systems
        .partition_point(|s| s.bars.last().is_some_and(|b| b.master_bar < master_bar));
    let system = list.systems.get(si)?;
    let bi = system
        .bars
        .binary_search_by_key(&master_bar, |b| b.master_bar)
        .ok()?;
    Some((si, &system.bars[bi]))
}

pub(super) fn last_onset(list: &DisplayList, master_bar: u32) -> Option<u32> {
    find_bar(list, master_bar)?
        .1
        .beats
        .last()
        .map(|b| b.tick_in_bar)
}

pub(super) fn glide(from: Cursor, to: Cursor, t: f32) -> Cursor {
    let t = t.clamp(0.0, 1.0);
    let e = t * t * (3.0 - 2.0 * t);
    let mix = |a: f32, b: f32| a + (b - a) * e;
    Cursor {
        master_bar: from.master_bar,
        // The destination's system, so following scrolls there as the glide starts.
        system: to.system,
        x: mix(from.x, to.x),
        y: mix(from.y, to.y),
        height: mix(from.height, to.height),
        bar_frame: Rect {
            x: mix(from.bar_frame.x, to.bar_frame.x),
            y: mix(from.bar_frame.y, to.bar_frame.y),
            width: mix(from.bar_frame.width, to.bar_frame.width),
            height: mix(from.bar_frame.height, to.bar_frame.height),
        },
    }
}

pub(super) fn cursor(
    list: &DisplayList,
    master_bar: u32,
    tick_in_bar: u32,
    bar_ticks: u32,
) -> Option<Cursor> {
    let (si, bar) = find_bar(list, master_bar)?;
    let frame = bar.frame;
    let right = frame.x + frame.width;

    // Onsets bracketing the tick; after the last onset, glide to the bar's right edge.
    let k = bar.beats.partition_point(|b| b.tick_in_bar <= tick_in_bar);
    let x = match k.checked_sub(1).map(|p| bar.beats[p]) {
        None => bar.beats.first().map_or(frame.x, |b| b.x),
        Some(prev) => {
            let (t1, x1) = bar
                .beats
                .get(k)
                .map_or((bar_ticks, right), |b| (b.tick_in_bar, b.x));
            if t1 <= prev.tick_in_bar {
                // Degenerate bar length: the last onset is at or past the bar's end.
                right
            } else {
                let span = (t1 - prev.tick_in_bar) as f32;
                let frac = (tick_in_bar - prev.tick_in_bar) as f32 / span;
                prev.x + (x1 - prev.x) * frac.clamp(0.0, 1.0)
            }
        }
    };

    Some(Cursor {
        master_bar,
        system: si as u32,
        x,
        y: frame.y,
        height: frame.height,
        bar_frame: frame,
    })
}

pub(super) fn hit_test(list: &DisplayList, x: f32, y: f32) -> Option<(u32, u32)> {
    let si = list
        .systems
        .partition_point(|s| s.frame.y + s.frame.height <= y);
    let system = list.systems.get(si)?;
    if y < system.frame.y {
        return None;
    }
    let bi = system
        .bars
        .partition_point(|b| b.frame.x + b.frame.width <= x);
    let bar = system.bars.get(bi)?;
    if x < bar.frame.x {
        return None;
    }
    let k = bar.beats.partition_point(|b| b.x < x);
    let before = k.checked_sub(1).map(|p| &bar.beats[p]);
    let beat = match (before, bar.beats.get(k)) {
        (Some(b), Some(a)) if x - b.x <= a.x - x => b,
        (_, Some(a)) => a,
        (Some(b), None) => b,
        (None, None) => return Some((bar.master_bar, 0)),
    };
    Some((bar.master_bar, beat.tick_in_bar))
}
