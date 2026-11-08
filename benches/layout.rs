//! Layout performance: a 300-bar, 6-string track with ~8 beats per bar (plus runs of empty bars
//! that collapse into multi-bar rests, and tied notes) must lay out in < 5 ms.

use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;
use tabster_core::layout::{self, LayoutOptions};
use tabster_core::model::{
    Bar, Beat, Duration, MasterBar, Note, NoteEffects, Score, Section, Track, Voice,
};

fn score(bars: usize) -> Score {
    let mut track = Track {
        name: "Guitar".into(),
        tuning: vec![64, 59, 55, 50, 45, 40],
        ..Default::default()
    };
    let mut master_bars = Vec::with_capacity(bars);
    for i in 0..bars {
        master_bars.push(MasterBar {
            section: (i % 16 == 0).then(|| Section {
                letter: "A".into(),
                text: "Verse".into(),
            }),
            repeat_start: i % 8 == 0,
            repeat_end: if i % 8 == 7 { 2 } else { 0 },
            ..Default::default()
        });
        if i % 20 >= 17 {
            // Three empty bars: a multi-bar rest.
            track.bars.push(Bar::default());
            continue;
        }
        let beats = (0..8u8)
            .map(|k| Beat {
                duration: Duration {
                    value: 8,
                    dots: 0,
                    tuplet: None,
                },
                notes: (0..1 + k % 3)
                    .map(|n| Note {
                        string: 1 + (k + n) % 6,
                        fret: (i as u8 % 15) + k,
                        velocity: 100,
                        tie_destination: k % 4 == 1,
                        effects: NoteEffects {
                            palm_mute: i % 4 == 0,
                            hammer_pull: k == 3,
                            ..Default::default()
                        },
                    })
                    .collect(),
                ..Default::default()
            })
            .collect();
        track.bars.push(Bar {
            voices: vec![Voice { beats }],
        });
    }
    Score {
        initial_tempo: 120.0,
        tracks: vec![track],
        master_bars,
        ..Default::default()
    }
}

fn bench(c: &mut Criterion) {
    let score = score(300);
    let options = LayoutOptions {
        track: 0,
        width: 1200.0,
        scale: 1.0,
        show_rhythm: true,
    };
    c.bench_function("layout 300 bars", |b| {
        b.iter(|| layout::layout(black_box(&score), black_box(&options)))
    });
    let list = layout::layout(&score, &options);
    c.bench_function("cursor", |b| {
        b.iter(|| layout::cursor(black_box(&list), black_box(237), black_box(1700), 3840))
    });
    c.bench_function("hit_test", |b| {
        b.iter(|| layout::hit_test(black_box(&list), black_box(600.0), black_box(5000.0)))
    });
}

criterion_group!(benches, bench);
criterion_main!(benches);
