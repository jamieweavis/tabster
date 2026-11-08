//! `TabSynth::render` of one 512-frame audio callback on a dense 5-track score (four tracks of
//! 16th-note triads plus 16th-note drums, looping). Target: < 2 ms in release (the callback's
//! real-time budget is 11.6 ms). Needs the SoundFont (`just soundfont`); skipped without it.

use std::hint::black_box;
use std::path::Path;

use criterion::{Criterion, criterion_group, criterion_main};
use tabster_core::model::{Bar, Beat, Duration, MasterBar, Note, Score, Track, Voice};
use tabster_core::playback::TabSynth;
use tabster_core::timeline::Timeline;

fn sixteenths(notes: impl Fn(usize) -> Vec<Note>) -> Bar {
    Bar {
        voices: vec![Voice {
            beats: (0..16)
                .map(|i| Beat {
                    duration: Duration {
                        value: 16,
                        dots: 0,
                        tuplet: None,
                    },
                    notes: notes(i),
                    ..Default::default()
                })
                .collect(),
        }],
    }
}

fn note(string: u8, fret: u8) -> Note {
    Note {
        string,
        fret,
        velocity: 100,
        ..Default::default()
    }
}

fn dense_score(bars: usize) -> Score {
    let guitar = |program: u8, base: u8| Track {
        program,
        tuning: vec![64, 59, 55, 50, 45, 40],
        bars: vec![
            sixteenths(|i| {
                let f = base + (i % 5) as u8;
                vec![note(3, f), note(4, f + 2), note(5, f + 2)]
            });
            bars
        ],
        ..Default::default()
    };
    let drums = Track {
        is_percussion: true,
        tuning: vec![0; 6],
        bars: vec![
            sixteenths(|i| {
                let mut n = vec![note(1, 42)];
                if i % 4 == 0 {
                    n.push(note(2, if i % 8 == 0 { 36 } else { 38 }));
                }
                n
            });
            bars
        ],
        ..Default::default()
    };
    Score {
        initial_tempo: 140.0,
        tracks: vec![
            guitar(29, 0),
            guitar(30, 3),
            guitar(33, 5),
            guitar(0, 7),
            drums,
        ],
        master_bars: vec![MasterBar::default(); bars],
        ..Default::default()
    }
}

fn bench(c: &mut Criterion) {
    let soundfont = Path::new(env!("CARGO_MANIFEST_DIR")).join("Resources/Bundle/SoundFont.sf2");
    if !soundfont.exists() {
        eprintln!("skipping synth bench: no SoundFont (run `just soundfont`)");
        return;
    }
    let score = dense_score(16);
    let synth = TabSynth::new(&score, &Timeline::new(&score), &soundfont).unwrap();
    synth.set_loop(None, None);
    synth.set_loop(Some(0.0), None);
    synth.set_metronome(true);
    synth.set_playing(true);
    // Warm up so the voices are sounding.
    let mut buf = vec![0.0f32; 1024];
    for _ in 0..200 {
        synth.render_into(&mut buf);
    }
    c.bench_function("render 512 frames (render_into)", |b| {
        b.iter(|| synth.render_into(black_box(&mut buf)));
    });
    c.bench_function("render 512 frames (Vec, FFI shape)", |b| {
        b.iter(|| black_box(synth.render(512)));
    });
}

criterion_group!(benches, bench);
criterion_main!(benches);
