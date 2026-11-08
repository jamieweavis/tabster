use proptest::prelude::*;

use super::*;
use crate::model::FileSyncPoint;
use crate::timeline::test_scores::ScoreBuilder;

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

/// `bars` bars of 2 s each.
fn timeline(bars: usize) -> Timeline {
    Timeline::new(&ScoreBuilder::new(bars).build())
}

fn points(pairs: &[(u32, f64)]) -> SyncSpec {
    SyncSpec::Points {
        points: pairs
            .iter()
            .map(|&(played_bar, video_secs)| SyncPoint {
                played_bar,
                video_secs,
            })
            .collect(),
    }
}

#[test]
fn default_map_is_identity() {
    let m = SyncMap::default();
    assert_eq!(m.score_to_video(3.5), 3.5);
    assert_eq!(m.video_to_score(3.5), 3.5);
    let m = SyncMap::new(&SyncSpec::default(), &timeline(4));
    assert!(close(m.score_to_video(3.5), 3.5));
}

#[test]
fn offset_mode() {
    let t = timeline(4);
    let m = SyncMap::new(
        &SyncSpec::Offset {
            offset_secs: 12.5,
            stretch: 1.0,
        },
        &t,
    );
    assert!(close(m.score_to_video(0.0), 12.5));
    assert!(close(m.score_to_video(4.0), 16.5));
    assert!(close(m.video_to_score(16.5), 4.0));
    // Before the video's bar 1 the score time is negative (count-in / intro).
    assert!(close(m.video_to_score(10.0), -2.5));
}

#[test]
fn stretch() {
    let t = timeline(4);
    let m = SyncMap::new(
        &SyncSpec::Offset {
            offset_secs: 1.0,
            stretch: 1.1,
        },
        &t,
    );
    assert!(close(m.score_to_video(10.0), 12.0));
    assert!(close(m.video_to_score(12.0), 10.0));
    for bad in [0.0, -2.0, f64::NAN, f64::INFINITY] {
        let m = SyncMap::new(
            &SyncSpec::Offset {
                offset_secs: 1.0,
                stretch: bad,
            },
            &t,
        );
        assert!(close(m.score_to_video(10.0), 11.0));
    }
}

#[test]
fn points_interpolation_and_extrapolation() {
    let t = timeline(4); // bar starts at 0, 2, 4, 6 s; ends at 8.
    let m = SyncMap::new(&points(&[(1, 10.0), (3, 13.0)]), &t);
    assert!(close(m.score_to_video(2.0), 10.0));
    assert!(close(m.score_to_video(4.0), 11.5));
    assert!(close(m.score_to_video(6.0), 13.0));
    // Extrapolation at slope 1.
    assert!(close(m.score_to_video(0.0), 8.0));
    assert!(close(m.score_to_video(8.0), 15.0));
    assert!(close(m.video_to_score(11.5), 4.0));
    assert!(close(m.video_to_score(7.0), -1.0));
    assert!(close(m.video_to_score(20.0), 13.0));
}

#[test]
fn single_point_is_an_offset() {
    let m = SyncMap::new(&points(&[(2, 30.0)]), &timeline(4));
    assert!(close(m.score_to_video(0.0), 26.0));
    assert!(close(m.video_to_score(31.0), 5.0));
}

#[test]
fn zero_points_is_identity() {
    let m = SyncMap::new(&points(&[]), &timeline(4));
    assert!(close(m.score_to_video(3.0), 3.0));
    // Only out-of-range / non-finite points: also identity.
    let m = SyncMap::new(&points(&[(9, 1.0), (1, f64::NAN)]), &timeline(4));
    assert!(close(m.video_to_score(3.0), 3.0));
}

#[test]
fn non_monotonic_points_dropped() {
    let t = timeline(6);
    // Point 2 is a typo (way too late); dropping it keeps the rest.
    let m = SyncMap::new(
        &points(&[(0, 1.0), (1, 3.0), (2, 99.0), (3, 7.0), (4, 9.0), (4, 50.0)]),
        &t,
    );
    let pts = m.to_points(&t);
    assert!(close(pts[0].video_secs, 1.0));
    assert!(close(pts[1].video_secs, 3.0));
    assert!(close(pts[2].video_secs, 5.0)); // interpolated
    assert!(close(pts[3].video_secs, 7.0));
    assert!(close(pts[4].video_secs, 9.0)); // first duplicate wins
    assert!(close(pts[5].video_secs, 11.0)); // extrapolated
    // Unsorted input and equal video times.
    let m = SyncMap::new(&points(&[(3, 7.0), (0, 1.0), (1, 1.0)]), &t);
    assert!(close(m.score_to_video(6.0), 7.0));
    // Exactly one of the two equal video times survives.
    assert!(close(m.score_to_video(0.0), 1.0) != close(m.score_to_video(2.0), 1.0));
}

#[test]
fn to_points_from_offset() {
    let t = timeline(3);
    let m = SyncMap::new(
        &SyncSpec::Offset {
            offset_secs: 5.0,
            stretch: 2.0,
        },
        &t,
    );
    let pts = m.to_points(&t);
    assert_eq!(pts.len(), 3);
    let expected = [5.0, 9.0, 13.0];
    for (i, p) in pts.iter().enumerate() {
        assert_eq!(p.played_bar, i as u32);
        assert!(close(p.video_secs, expected[i]));
    }
    // Round trip through points mode gives the same map on the timeline.
    let m2 = SyncMap::new(&SyncSpec::Points { points: pts }, &t);
    assert!(close(m2.score_to_video(3.0), m.score_to_video(3.0)));
}

#[test]
fn bar_points_import() {
    let spec = from_bar_points(&[0.5, 2.5, 4.4, 6.6]);
    let SyncSpec::Points { points } = &spec else {
        panic!("expected points");
    };
    assert_eq!(points.len(), 4);
    assert_eq!(points[3].played_bar, 3);
    // Three bars: the 4th value is the end time and is dropped.
    let t = timeline(3);
    let m = SyncMap::new(&spec, &t);
    assert!(close(m.score_to_video(4.0), 4.4));
    assert!(close(m.score_to_video(6.0), 6.4)); // slope 1 past the last bar start
}

#[test]
fn file_sync_points() {
    let mut score = ScoreBuilder::new(4).repeat(0, 1, 2).build();
    // Played: 0 1 0 1 2 3 at 2 s each.
    score.sync_points = vec![
        FileSyncPoint {
            bar: 0,
            bar_occurrence: 0,
            bar_position: 0.0,
            millisecond_offset: 1000.0,
        },
        // Second pass of bar 0 (played bar 2, score 4 s), halfway through (score 5 s).
        FileSyncPoint {
            bar: 0,
            bar_occurrence: 1,
            bar_position: 0.5,
            millisecond_offset: 7000.0,
        },
        // Never played.
        FileSyncPoint {
            bar: 3,
            bar_occurrence: 1,
            bar_position: 0.0,
            millisecond_offset: 9000.0,
        },
    ];
    let t = Timeline::new(&score);
    let Some(SyncSpec::Points { points }) = from_file_sync_points(&score, &t) else {
        panic!("expected points");
    };
    assert_eq!(points.len(), 2);
    assert_eq!(points[0].played_bar, 0);
    assert!(close(points[0].video_secs, 1.0));
    // Slope from the previous point: (7 - 1) / (5 - 0) = 1.2; 1 s of score back → 5.8.
    assert_eq!(points[1].played_bar, 2);
    assert!(close(points[1].video_secs, 5.8));

    score.sync_points.clear();
    assert_eq!(from_file_sync_points(&score, &t), None);
}

/// A realistic recording: each bar takes 0.5–4 s of video. Returns (bar count, points in
/// shuffled order, outliers).
#[allow(clippy::type_complexity)]
fn recording() -> impl Strategy<Value = (usize, Vec<(u32, f64)>, Vec<(u32, f64)>)> {
    (1usize..60).prop_flat_map(|bars| {
        (
            Just(bars),
            -100.0f64..100.0,
            prop::collection::vec((0.5f64..4.0, any::<bool>()), bars),
            prop::collection::vec((0..bars as u32 + 3, -100.0f64..400.0), 0..6),
            any::<u64>(),
        )
            .prop_map(|(bars, base, steps, outliers, seed)| {
                let mut video = base;
                let mut pts = Vec::new();
                for (i, (step, keep)) in steps.into_iter().enumerate() {
                    if keep {
                        pts.push((i as u32, video));
                    }
                    video += step;
                }
                // Deterministic shuffle.
                let mut state = seed | 1;
                for i in (1..pts.len()).rev() {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    pts.swap(i, (state % (i as u64 + 1)) as usize);
                }
                (bars, pts, outliers)
            })
    })
}

fn check_map(m: &SyncMap, t: &Timeline, xs: &[f64], tolerance: f64) -> Result<(), TestCaseError> {
    let mut xs = xs.to_vec();
    xs.sort_by(f64::total_cmp);
    let mut prev = f64::NEG_INFINITY;
    for &x in &xs {
        let v = m.score_to_video(x);
        prop_assert!(v >= prev, "not monotonic at {x}: {v} < {prev}");
        prev = v;
        let back = m.video_to_score(v);
        prop_assert!((back - x).abs() < tolerance, "{x} -> {v} -> {back}");
        let fwd = m.score_to_video(m.video_to_score(x));
        prop_assert!((fwd - x).abs() < tolerance, "{x} -> {fwd}");
    }
    let pts = m.to_points(t);
    prop_assert!(pts.windows(2).all(|w| w[0].video_secs < w[1].video_secs));
    Ok(())
}

proptest! {
    #[test]
    fn monotonic_and_invertible(
        (bars, pts, _) in recording(),
        xs in prop::collection::vec(-200.0f64..400.0, 1..40),
    ) {
        let t = timeline(bars);
        let m = SyncMap::new(&points(&pts), &t);
        check_map(&m, &t, &xs, 1e-9)?;
        // Every well-formed point is kept.
        for &(bar, video) in &pts {
            let start = t.played_bars()[bar as usize].start_secs;
            prop_assert!((m.score_to_video(start) - video).abs() < 1e-9);
        }
    }

    #[test]
    fn monotonic_with_bad_points(
        (bars, mut pts, outliers) in recording(),
        xs in prop::collection::vec(-200.0f64..400.0, 1..40),
    ) {
        pts.extend(outliers);
        let t = timeline(bars);
        let m = SyncMap::new(&points(&pts), &t);
        // Outliers can create very flat or steep segments, which amplify rounding.
        check_map(&m, &t, &xs, 1e-6)?;
    }

    #[test]
    fn offset_invertible(offset in -100.0f64..100.0, stretch in 0.5f64..2.0, x in -100.0f64..1000.0) {
        let m = SyncMap::new(&SyncSpec::Offset { offset_secs: offset, stretch }, &timeline(2));
        prop_assert!((m.video_to_score(m.score_to_video(x)) - x).abs() < 1e-9);
    }
}
