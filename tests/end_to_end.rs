//! Whole-pipeline checks through the FFI surface: import → open → layout → cursor → sync.

use std::path::{Path, PathBuf};

use tabster_core::Core;
use tabster_core::layout::LayoutOptions;
use tabster_core::library::TabFilter;
use tabster_core::sync::{SyncPoint, SyncSpec};

fn fixture(name: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join(name)
        .to_string_lossy()
        .into_owned()
}

fn run_pipeline(path: &str) {
    let dir = tempfile::tempdir().unwrap();
    // A private config, so the test never reads or watches the user's folders.
    let core = Core::with_paths(dir.path(), &dir.path().join("tabster.toml")).unwrap();

    let outcomes = core.import_paths(vec![path.to_string()]);
    assert_eq!(outcomes.len(), 1);
    assert!(
        outcomes[0].error.is_none(),
        "import failed: {:?}",
        outcomes[0].error
    );
    let id = outcomes[0].id.unwrap();

    let tabs = core.list_tabs(TabFilter::default()).unwrap();
    assert_eq!(tabs.len(), 1);
    assert!(tabs[0].duration_secs > 0.0);

    let session = core.open_tab(id).unwrap();
    let info = session.info();
    assert!(!info.tracks.is_empty());
    assert!(info.duration_secs > 0.0);

    let track = info
        .tracks
        .iter()
        .find(|t| !t.is_percussion)
        .map_or(0, |t| t.index);
    let list = session.layout(LayoutOptions {
        track,
        width: 900.0,
        scale: 1.0,
        show_rhythm: true,
    });
    assert!(!list.systems.is_empty());

    // The cursor moves forward through the song.
    let mut last_bar = 0;
    let steps = 50;
    for i in 0..steps {
        let t = info.duration_secs * f64::from(i) / f64::from(steps);
        let pos = session.position_at_score(t).expect("position");
        assert!(pos.played_bar >= last_bar);
        last_bar = pos.played_bar;
        assert!(
            pos.cursor.is_some(),
            "no cursor at {t}s (bar {})",
            pos.master_bar
        );
    }

    // Clicking where the cursor is lands at (about) the same time.
    let mid = session
        .position_at_score(info.duration_secs / 3.0)
        .unwrap()
        .cursor
        .unwrap();
    let secs =
        session.score_secs_at_point(mid.x, mid.y + mid.height / 2.0, info.duration_secs / 3.0);
    assert!(secs.is_some());

    // Offset sync: video time = score time + 4s.
    session.set_sync(SyncSpec::Offset {
        offset_secs: 4.0,
        stretch: 1.0,
    });
    assert!((session.score_to_video(1.0) - 5.0).abs() < 1e-9);
    let a = session
        .position_at_video(4.0 + info.duration_secs / 2.0)
        .unwrap();
    let b = session.position_at_score(info.duration_secs / 2.0).unwrap();
    assert_eq!(a.played_bar, b.played_bar);

    // Points sync round-trips through the library.
    let video = core
        .add_video(
            id,
            "https://youtu.be/cJRFMfztgBg?t=4".into(),
            "Original".into(),
        )
        .unwrap();
    let points: Vec<SyncPoint> = session.sync_points();
    assert_eq!(points.len() as u32, info.played_bar_count);
    core.set_video_sync(
        video,
        SyncSpec::Points {
            points: points.clone(),
        },
    )
    .unwrap();
    let videos = core.videos(id).unwrap();
    assert_eq!(videos[0].youtube_id, "cJRFMfztgBg");
    assert!(videos[0].is_default);
    assert_eq!(videos[0].sync, SyncSpec::Points { points });
}

#[test]
fn gp5_pipeline() {
    run_pipeline(&fixture("gp5/basics.gp5"));
}

#[test]
fn gp5_v500_pipeline() {
    run_pipeline(&fixture("gp5/v500.gp5"));
}

/// The user's real GP8 tab (not committed). `cargo test real_tab -- --ignored`
#[test]
#[ignore]
fn real_tab_pipeline() {
    let path: PathBuf =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("Men I Trust-Show Me How-09-08-2026.gp");
    if path.exists() {
        run_pipeline(&path.to_string_lossy());
    }
}

/// Approaching a downbeat, the cursor slides into the next played bar and lands on its first
/// beat as that bar starts; with a loop, the last bar slides back to the loop's first.
#[test]
fn cursor_glides_into_the_next_bar() {
    let dir = tempfile::tempdir().unwrap();
    let core = Core::with_paths(dir.path(), &dir.path().join("tabster.toml")).unwrap();
    let id = core.import_paths(vec![fixture("gp5/basics.gp5")])[0]
        .id
        .unwrap();
    let session = core.open_tab(id).unwrap();
    session.layout(LayoutOptions {
        track: 0,
        width: 900.0,
        scale: 1.0,
        show_rhythm: true,
    });
    let bars = session.played_bars();
    assert!(bars.len() >= 3);
    let x_at = |t: f64| session.position_at_score(t).unwrap().cursor.unwrap();

    for pair in bars.windows(2) {
        let (bar, next) = (pair[0], pair[1]);
        let landing = x_at(next.start_secs);
        let just_before = x_at(bar.end_secs - 1e-5);
        assert_eq!(just_before.master_bar, bar.master_bar);
        assert!(
            (just_before.x - landing.x).abs() < 1.0 && (just_before.y - landing.y).abs() < 1.0,
            "bar {} ends at {just_before:?}, next starts at {landing:?}",
            bar.master_bar
        );
        // Well before the end, it's still inside its own bar.
        let earlier = x_at(bar.end_secs - 0.5_f64.min((bar.end_secs - bar.start_secs) / 2.0));
        let frame = earlier.bar_frame;
        assert!(earlier.x >= frame.x && earlier.x <= frame.x + frame.width);
    }

    let (first, second) = (bars[0], bars[1]);
    session.set_loop(Some(first.start_secs), Some(second.end_secs));
    let landing = x_at(first.start_secs);
    let just_before = x_at(second.end_secs - 1e-5);
    assert!((just_before.x - landing.x).abs() < 1.0);
    session.set_loop(None, None);
    assert!((x_at(second.end_secs - 1e-5).x - x_at(bars[2].start_secs).x).abs() < 1.0);
}
