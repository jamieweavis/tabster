//! Hot-path lookups: `Timeline::position_at_secs` and `SyncMap::video_to_score`, on a 300-bar
//! score with repeats and tempo changes and a 1000-point sync map. Target: < 1 µs each.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use tabster_core::model::{Bar, Beat, MasterBar, Score, TempoChange, Track, Voice};
use tabster_core::sync::{SyncMap, from_bar_points};
use tabster_core::timeline::Timeline;

fn score(bars: usize) -> Score {
    let bar = Bar {
        voices: vec![Voice {
            beats: vec![Beat::default(); 4],
        }],
    };
    let mut master_bars = vec![MasterBar::default(); bars];
    // A repeated section every 20 bars (8 bars played twice).
    for start in (0..bars.saturating_sub(10)).step_by(20) {
        master_bars[start].repeat_start = true;
        master_bars[start + 7].repeat_end = 2;
    }
    let tempo_changes = (0..bars as u32)
        .step_by(7)
        .map(|bar| TempoChange {
            bar,
            position: 0.5,
            bpm: 90.0 + f64::from(bar % 60),
        })
        .collect();
    Score {
        initial_tempo: 120.0,
        tracks: vec![Track {
            tuning: vec![64, 59, 55, 50, 45, 40],
            bars: vec![bar; bars],
            ..Default::default()
        }],
        master_bars,
        tempo_changes,
        ..Default::default()
    }
}

fn bench(c: &mut Criterion) {
    let timeline = Timeline::new(&score(300));
    let played = timeline.played_bars().len();
    let duration = timeline.duration_secs();

    // 1000 points with a slowly drifting tempo (the timeline has ~420 played bars; the rest
    // past the end are dropped, so also build a map on a longer timeline to get 1000 anchors).
    let long = Timeline::new(&score(1000));
    let video: Vec<f64> = long
        .played_bars()
        .iter()
        .take(1000)
        .enumerate()
        .map(|(i, b)| 3.0 + b.start_secs * (1.0 + 0.01 * ((i as f64) * 0.05).sin()))
        .collect();
    assert_eq!(video.len(), 1000);
    let map = SyncMap::new(&from_bar_points(&video), &long);
    let video_end = map.score_to_video(long.duration_secs());

    let mut group = c.benchmark_group("timeline");
    let mut x = 0.0;
    group.bench_function(format!("position_at_secs/{played}_played_bars"), |b| {
        b.iter(|| {
            x = (x + 1.37) % duration;
            black_box(timeline.position_at_secs(black_box(x)))
        })
    });
    let mut v = 0.0;
    group.bench_function("video_to_score/1000_points", |b| {
        b.iter(|| {
            v = (v + 1.37) % video_end;
            black_box(map.video_to_score(black_box(v)))
        })
    });
    let mut v = 0.0;
    group.bench_function("video_to_score+position_at_secs", |b| {
        b.iter(|| {
            v = (v + 1.37) % video_end;
            black_box(long.position_at_secs(map.video_to_score(black_box(v))))
        })
    });
    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
