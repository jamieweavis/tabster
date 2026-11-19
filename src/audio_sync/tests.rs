//! The aligner against synthetic recordings: the tab's own notes played by a crude additive
//! synth along a warped time axis (an intro, a stretch, a slow tempo wobble), with an unrelated
//! melody and noise on top. These check the plumbing and the maths, not how real music fares.

use super::*;
use crate::model::{Bar, Beat, Duration, MasterBar, Note, Track, Voice};

const RATE: f64 = 22_050.0;

/// Deterministic pseudo-random numbers (the tests must not depend on a seed of the day).
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) as u32
    }

    fn unit(&mut self) -> f64 {
        f64::from(self.next()) / f64::from(u32::MAX >> 1)
    }
}

/// `bars` bars of 4/4 in eighths at 110 bpm: single notes, power chords and rests over a chord
/// progression, different in every bar.
fn riff_score(bars: usize) -> Score {
    let mut rng = Lcg(7);
    // Frets on the two lowest strings for the progression's roots.
    let roots: [(u8, u8); 6] = [(6, 0), (6, 3), (5, 0), (5, 3), (5, 5), (6, 5)];
    let eighth = Duration {
        value: 8,
        dots: 0,
        tuplet: None,
    };
    let note = |string: u8, fret: u8| Note {
        string,
        fret,
        velocity: 100,
        ..Default::default()
    };
    let track_bars = (0..bars)
        .map(|b| {
            let (string, fret) = roots[(b / 2 + (rng.next() % 2) as usize) % roots.len()];
            let beats = (0..8)
                .map(|_| {
                    let notes = match rng.next() % 10 {
                        0 | 1 => vec![],
                        2..=5 => vec![note(string, fret)],
                        6 | 7 => vec![note(string, fret), note(string - 1, fret + 2)],
                        _ => vec![note(string - 2, fret + (rng.next() % 4) as u8)],
                    };
                    Beat {
                        duration: eighth,
                        notes,
                        ..Default::default()
                    }
                })
                .collect();
            Bar {
                voices: vec![Voice { beats }],
            }
        })
        .collect();
    Score {
        title: "Riff".into(),
        initial_tempo: 110.0,
        tracks: vec![Track {
            name: "Guitar".into(),
            tuning: vec![64, 59, 55, 50, 45, 40],
            bars: track_bars,
            ..Default::default()
        }],
        master_bars: vec![MasterBar::default(); bars],
        ..Default::default()
    }
}

/// Score seconds → recording seconds.
#[derive(Clone, Copy)]
struct Warp {
    intro: f64,
    stretch: f64,
    wobble: f64,
}

impl Warp {
    fn at(&self, s: f64) -> f64 {
        self.intro + s * self.stretch + self.wobble * (std::f64::consts::TAU * s / 37.0).sin()
    }
}

/// A recording of `score` along `warp`, `semitones` above the tab, from recording time `from`.
fn record(score: &Score, warp: Warp, semitones: f64, from: f64) -> Vec<f32> {
    let timeline = Timeline::new(score);
    let events = events::build(score, &timeline);
    let length = warp.at(events.duration_secs) + 4.0 - from;
    let mut out = vec![0.0f64; (length * RATE) as usize];

    let mut starts: std::collections::HashMap<u8, f64> = std::collections::HashMap::new();
    for e in &events.notes {
        if e.is_on() {
            starts.insert(e.key, e.secs);
            continue;
        }
        let Some(on) = starts.remove(&e.key) else {
            continue;
        };
        let (t0, t1) = (warp.at(on) - from, warp.at(e.secs) - from + 0.08);
        let hz = 440.0 * 2f64.powf((f64::from(e.key) + semitones - 69.0) / 12.0);
        let first = (t0.max(0.0) * RATE) as usize;
        let last = ((t1 * RATE).max(0.0) as usize).min(out.len());
        for (i, sample) in out.iter_mut().enumerate().take(last).skip(first) {
            let t = i as f64 / RATE - t0;
            // Smooth attack and release: a kink in the envelope is a click, and clicks are onsets.
            let ease = |x: f64| 0.5 - 0.5 * (std::f64::consts::PI * x.clamp(0.0, 1.0)).cos();
            let envelope = ease(t / 0.005) * (-t / 0.6).exp() * ease((t1 - t0 - t) / 0.08);
            let phase = std::f64::consts::TAU * hz * t;
            let tone: f64 = (1..=4)
                .map(|h| (phase * f64::from(h)).sin() / f64::from(h))
                .sum();
            *sample += 0.15 * envelope * tone;
        }
    }
    add_distractor(&mut out, from);
    out.into_iter().map(|v| v as f32).collect()
}

/// An unrelated melody (the "vocals") and noise.
fn add_distractor(out: &mut [f64], from: f64) {
    let mut rng = Lcg(99);
    let mut noise = Lcg(1234);
    let melody: Vec<f64> = (0..4096).map(|_| 300.0 + 500.0 * rng.unit()).collect();
    let mut phase = 0.0f64;
    for (i, sample) in out.iter_mut().enumerate() {
        let t = i as f64 / RATE + from;
        let hz = melody[((t / 0.7) as usize) % melody.len()];
        phase += std::f64::consts::TAU * hz / RATE;
        *sample += 0.08 * phase.sin() + 0.01 * (noise.unit() - 0.5);
    }
}

/// Feed `audio` in half-second chunks; the first sample is heard at video time `video_start`.
fn capture(audio: &[f32], video_start: f64) -> AudioCapture {
    let capture = AudioCapture::new(RATE);
    for (i, chunk) in audio.chunks((RATE / 2.0) as usize).enumerate() {
        capture.push_samples(chunk, Some(video_start + i as f64 * 0.5));
    }
    capture
}

fn bar_errors(found: &AudioSync, timeline: &Timeline, expected: impl Fn(f64) -> f64) -> Vec<f64> {
    let SyncSpec::Points { points } = &found.sync else {
        panic!("expected per-bar points");
    };
    let bars = timeline.played_bars();
    assert_eq!(points.len(), bars.len());
    (found.first_bar..=found.last_bar)
        .map(|k| points[k as usize].video_secs - expected(bars[k as usize].start_secs))
        .collect()
}

fn worst(errors: &[f64]) -> f64 {
    errors.iter().fold(0.0, |m, e| m.max(e.abs()))
}

fn median(errors: &[f64]) -> f64 {
    let mut sorted: Vec<f64> = errors.iter().map(|e| e.abs()).collect();
    sorted.sort_by(f64::total_cmp);
    sorted[sorted.len() / 2]
}

const WARP: Warp = Warp {
    intro: 7.3,
    stretch: 1.03,
    wobble: 0.2,
};

#[test]
fn finds_every_bar_of_a_warped_recording() {
    let score = riff_score(40);
    let timeline = Timeline::new(&score);
    let audio = record(&score, WARP, 0.0, 0.0);
    // The capture started 5 s into the video.
    let found = sync_from_capture(&score, &timeline, &capture(&audio, 5.0)).unwrap();

    let errors = bar_errors(&found, &timeline, |s| 5.0 + WARP.at(s));
    eprintln!(
        "bars {}..={} matched {} confidence {:.2} stretch {:.3} median {:.1} ms worst {:.1} ms",
        found.first_bar,
        found.last_bar,
        found.matched_bars,
        found.confidence,
        found.stretch,
        median(&errors) * 1000.0,
        worst(&errors) * 1000.0
    );
    assert!(found.first_bar <= 1 && found.last_bar >= 38, "{found:?}");
    assert_eq!(found.transpose, 0);
    assert!(
        (found.stretch - 1.03).abs() < 0.01,
        "stretch {}",
        found.stretch
    );
    assert!(median(&errors) < 0.02, "median error {}", median(&errors));
    assert!(worst(&errors) < 0.06, "worst error {}", worst(&errors));
    assert!(found.confidence > 0.5, "confidence {}", found.confidence);
}

#[test]
fn aligns_a_capture_of_the_middle_of_the_song() {
    let score = riff_score(40);
    let timeline = Timeline::new(&score);
    // Listening started 30 s into the recording and stopped 40 s later.
    let audio = record(&score, WARP, 0.0, 30.0);
    let audio = &audio[..(40.0 * RATE) as usize];
    let found = sync_from_capture(&score, &timeline, &capture(audio, 30.0)).unwrap();

    let errors = bar_errors(&found, &timeline, |s| WARP.at(s));
    eprintln!(
        "bars {}..={} median {:.1} ms worst {:.1} ms",
        found.first_bar,
        found.last_bar,
        median(&errors) * 1000.0,
        worst(&errors) * 1000.0
    );
    let bars = timeline.played_bars();
    assert!(
        WARP.at(bars[found.first_bar as usize].start_secs) < 34.0,
        "{found:?}"
    );
    assert!(
        WARP.at(bars[found.last_bar as usize].start_secs) > 66.0,
        "{found:?}"
    );
    assert!(worst(&errors) < 0.06, "worst error {}", worst(&errors));
}

#[test]
fn hears_a_recording_tuned_a_semitone_down() {
    let score = riff_score(40);
    let timeline = Timeline::new(&score);
    let audio = record(&score, WARP, -1.0, 0.0);
    let found = sync_from_capture(&score, &timeline, &capture(&audio, 0.0)).unwrap();

    assert_eq!(found.transpose, -1);
    let errors = bar_errors(&found, &timeline, |s| WARP.at(s));
    assert!(worst(&errors) < 0.06, "worst error {}", worst(&errors));
}

/// A tab written a minor third away from the recording (a capo, a transposing instrument):
/// no key is assumed, each is tried.
#[test]
fn hears_a_recording_in_a_distant_key() {
    let score = riff_score(40);
    let timeline = Timeline::new(&score);
    let audio = record(&score, WARP, 3.0, 0.0);
    let found = sync_from_capture(&score, &timeline, &capture(&audio, 0.0)).unwrap();

    assert_eq!(found.transpose, 3);
    let errors = bar_errors(&found, &timeline, |s| WARP.at(s));
    assert!(worst(&errors) < 0.06, "worst error {}", worst(&errors));
}

#[test]
fn rejects_captures_it_cannot_use() {
    let score = riff_score(40);
    let timeline = Timeline::new(&score);
    let sync = |capture: &AudioCapture| sync_from_capture(&score, &timeline, capture);

    let short = vec![0.1f32; (5.0 * RATE) as usize];
    assert_eq!(sync(&capture(&short, 0.0)), Err(AudioSyncError::TooShort));

    let silent = vec![0.0f32; (30.0 * RATE) as usize];
    assert_eq!(sync(&capture(&silent, 0.0)), Err(AudioSyncError::Silent));

    // Something else entirely was playing.
    let mut other = vec![0.0f64; (60.0 * RATE) as usize];
    add_distractor(&mut other, 0.0);
    let other: Vec<f32> = other.into_iter().map(|v| v as f32).collect();
    assert_eq!(sync(&capture(&other, 0.0)), Err(AudioSyncError::NoMatch));

    // The video jumped 20 s halfway through the capture.
    let audio = record(&score, WARP, 0.0, 0.0);
    let seeked = AudioCapture::new(RATE);
    for (i, chunk) in audio.chunks((RATE / 2.0) as usize).enumerate() {
        let jump = if i >= 60 { 20.0 } else { 0.0 };
        seeked.push_samples(chunk, Some(i as f64 * 0.5 + jump));
    }
    assert_eq!(sync(&seeked), Err(AudioSyncError::UnsteadyClock));
}

#[test]
fn streaming_chunk_size_does_not_change_the_features() {
    let score = riff_score(8);
    let audio = record(&score, WARP, 0.0, 0.0);
    let mut whole = Extractor::new(RATE);
    whole.push(&audio);
    let mut pieces = Extractor::new(RATE);
    for chunk in audio.chunks(777) {
        pieces.push(chunk);
    }
    assert_eq!(whole.features(), pieces.features());
    // 100 onset frames and 10 chroma frames a second.
    let secs = audio.len() as f64 / RATE;
    assert!((whole.features().onset.len() as f64 - secs * 100.0).abs() < 12.0);
    assert!((whole.features().chroma.len() as f64 - secs * 10.0).abs() < 2.0);
}

/// Every bar's error (seconds) for `score` recorded along `warp` and captured from its start.
fn all_bar_errors(score: &Score, warp: Warp) -> (AudioSync, Vec<f64>) {
    let timeline = Timeline::new(score);
    let audio = record(score, warp, 0.0, 0.0);
    let found = sync_from_capture(score, &timeline, &capture(&audio, 0.0)).unwrap();
    let SyncSpec::Points { points } = &found.sync else {
        panic!("expected per-bar points");
    };
    let errors = timeline
        .played_bars()
        .iter()
        .zip(points)
        .map(|(bar, point)| point.video_secs - warp.at(bar.start_secs))
        .collect();
    (found, errors)
}

#[test]
fn a_long_video_intro_does_not_matter() {
    // A minute and a half of something else first, and the band plays 4% fast.
    let warp = Warp {
        intro: 90.0,
        stretch: 0.96,
        wobble: 0.1,
    };
    let (found, errors) = all_bar_errors(&riff_score(40), warp);
    assert!(worst(&errors) < 0.06, "{found:?} {errors:?}");
}

#[test]
fn keeps_time_through_a_sparse_intro() {
    // 16 bars of one held chord each (too little for chroma to pin down), then the riff.
    let mut score = riff_score(56);
    for bar in &mut score.tracks[0].bars[..16] {
        let mut notes = bar.voices[0]
            .beats
            .iter()
            .find(|b| !b.notes.is_empty())
            .unwrap()
            .notes
            .clone();
        notes.truncate(1);
        bar.voices[0].beats = vec![Beat {
            duration: Duration {
                value: 1,
                dots: 0,
                tuplet: None,
            },
            notes,
            ..Default::default()
        }];
    }
    let warp = Warp {
        intro: 20.0,
        stretch: 1.02,
        wobble: 0.1,
    };
    let (found, errors) = all_bar_errors(&score, warp);
    assert!(worst(&errors) < 0.06, "{found:?} {errors:?}");
}

#[test]
fn rests_the_tab_opens_with_keep_their_written_length() {
    // The tab rests for 16 bars, written to fill the recording's intro; the recording runs
    // 2% slow once it plays.
    let mut score = riff_score(56);
    for bar in &mut score.tracks[0].bars[..16] {
        for beat in &mut bar.voices[0].beats {
            beat.notes.clear();
        }
    }
    let warp = Warp {
        intro: 3.0,
        stretch: 1.02,
        wobble: 0.1,
    };
    let (found, errors) = all_bar_errors(&score, warp);
    assert!(found.first_bar >= 15, "{found:?}");
    assert!(
        worst(&errors[found.first_bar as usize..]) < 0.06,
        "{errors:?}"
    );
    // Nothing to hear in the rests, so no tempo to take from them: they stay as written,
    // counted back from the first bar with notes.
    let SyncSpec::Points { points } = &found.sync else {
        panic!("{found:?}");
    };
    let timeline = Timeline::new(&score);
    let bars = timeline.played_bars();
    let written = bars[16].start_secs - bars[0].start_secs;
    let placed = points[16].video_secs - points[0].video_secs;
    assert!((placed - written).abs() < 1e-6, "{placed} vs {written}");
}

#[test]
fn a_short_listen_to_a_repeating_riff_starts_the_tab_where_the_music_starts() {
    // The same four bars over and over: twenty seconds of it fit the tab in many places.
    let mut score = riff_score(48);
    let pattern: Vec<Bar> = score.tracks[0].bars[..4].to_vec();
    for (i, bar) in score.tracks[0].bars.iter_mut().enumerate() {
        *bar = pattern[i % 4].clone();
    }
    let timeline = Timeline::new(&score);
    let warp = Warp {
        intro: 6.0,
        stretch: 1.0,
        wobble: 0.0,
    };
    // Listening started with the video and stopped 14 s into the song.
    let audio = record(&score, warp, 0.0, 0.0);
    let audio = &audio[..(20.0 * RATE) as usize];
    let found = sync_from_capture(&score, &timeline, &capture(audio, 0.0)).unwrap();

    let SyncSpec::Points { points } = &found.sync else {
        panic!("expected per-bar points");
    };
    assert!(found.first_bar <= 1, "{found:?}");
    // Every bar, heard or not: the tempo carries the rest of the tab.
    for (bar, point) in timeline.played_bars().iter().zip(points) {
        let error = point.video_secs - warp.at(bar.start_secs);
        assert!(
            error.abs() < 0.06,
            "bar at {} is {error} s off",
            bar.start_secs
        );
    }
}

/// Bars the capture should hold but the alignment never reached count against it, like bars
/// in reach whose onsets weren't found.
#[test]
fn confidence_counts_the_bars_the_capture_should_hold() {
    let all = confidence(0.6, 100, 100);
    assert!((0.8..1.0).contains(&all), "{all}");
    // The first verse of a full listen, however well it fits.
    let verse = confidence(0.6, 11, 100);
    assert!(verse < 0.65 * all, "{verse} vs {all}");
    assert!((confidence(0.6, 50, 100) - 0.75 * all).abs() < 1e-9);
    // Nothing alike is nothing, however many onsets line up.
    assert_eq!(confidence(ALIKE_FLOOR, 100, 100), 0.0);
    assert!(confidence(0.6, 0, 0).is_finite());
}
