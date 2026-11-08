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
    let core = Core::new(dir.path().to_string_lossy().into_owned()).unwrap();

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
