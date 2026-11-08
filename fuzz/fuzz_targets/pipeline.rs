//! Everything downstream of an import: parse → timeline → sync maps → layout → playback events,
//! and the first ~30 s of the playback engine (with a null synth).
#![no_main]

use libfuzzer_sys::fuzz_target;
use tabster_core::formats::{self, gpif};
use tabster_core::layout::{self, LayoutOptions};
use tabster_core::playback::{Engine, Sink, events};
use tabster_core::sync::{self, SyncMap, SyncPoint, SyncSpec};
use tabster_core::timeline::Timeline;

struct NullSink;

impl Sink for NullSink {
    fn note_on(&mut self, _: u8, _: u8, _: u8) {}
    fn note_off(&mut self, _: u8, _: u8) {}
    fn all_notes_off(&mut self, _: bool) {}
    fn channel_notes_off(&mut self, _: u8) {}
    fn program_change(&mut self, _: u8, _: u8) {}
    fn set_volume(&mut self, _: f32) {}
    fn render(&mut self, _: &mut [f32], _: &mut [f32]) {}
}

fuzz_target!(|data: &[u8]| {
    // Magic-detected formats first; anything else is tried as GPIF XML so XML seeds mutate well.
    let score = match formats::detect(data, None) {
        Some(_) => formats::parse(data, None),
        None => gpif::parse_gpif(data),
    };
    let Ok(score) = score else { return };

    let timeline = Timeline::new(&score);
    let n = timeline.played_bars().len() as u32;
    let specs = [
        SyncSpec::default(),
        SyncSpec::Offset {
            offset_secs: -3.5,
            stretch: 1.01,
        },
        SyncSpec::Points {
            points: (0..n.min(64))
                .map(|i| SyncPoint {
                    played_bar: i * 2,
                    video_secs: f64::from(i) * 1.7 + 2.0,
                })
                .collect(),
        },
        sync::from_bar_points(&[0.0, 1.0, 0.5, 3.0, f64::NAN, 10.0]),
    ];
    let file_spec = sync::from_file_sync_points(&score, &timeline);
    for spec in specs.iter().chain(file_spec.as_ref()) {
        let map = SyncMap::new(spec, &timeline);
        for v in [-10.0, 0.0, 1.0, 30.0, 1e9] {
            let s = map.video_to_score(v);
            let _ = map.score_to_video(s);
            let _ = timeline.position_at_secs(s);
        }
        let _ = map.to_points(&timeline);
    }

    for track in 0..score.tracks.len().min(4) as u32 {
        for width in [320.0, 1200.0] {
            let list = layout::layout(
                &score,
                &LayoutOptions {
                    track,
                    width,
                    scale: 1.0,
                    show_rhythm: true,
                },
            );
            if let Some(pb) = timeline.played_bars().first() {
                let len = (pb.end_tick - pb.start_tick) as u32;
                let _ = layout::cursor(&list, pb.master_bar, len / 2, len);
            }
            let _ = layout::hit_test(&list, width / 2.0, 40.0);
        }
    }

    let mut engine = Engine::new(NullSink, events::build(&score, &timeline));
    engine.set_metronome(true);
    engine.set_playing(true);
    let mut buf = vec![0.0f32; 2 * 4096];
    // ~30 s at 44.1 kHz.
    for _ in 0..323 {
        engine.render_into(&mut buf);
    }
});
