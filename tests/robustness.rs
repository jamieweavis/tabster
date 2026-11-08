//! Pathological-but-valid (and hostile) inputs: everything must finish quickly, stay within
//! sane memory, and never panic.

use std::time::{Duration as Elapsed, Instant};

use tabster_core::formats;
use tabster_core::layout::{self, LayoutOptions};
use tabster_core::model::{
    Bar, Beat, BeatEffects, Direction, Duration, MasterBar, Note, Score, TempoChange,
    TimeSignature, Track, Voice,
};
use tabster_core::playback::events;
use tabster_core::sync::{SyncMap, SyncPoint, SyncSpec};
use tabster_core::timeline::Timeline;

// ---------------------------------------------------------------------------------------------
// Helpers

fn note(string: u8, fret: u8) -> Note {
    Note {
        string,
        fret,
        velocity: 95,
        ..Default::default()
    }
}

fn beat(value: u8, tuplet: Option<(u8, u8)>, notes: Vec<Note>) -> Beat {
    Beat {
        duration: Duration {
            value,
            dots: 0,
            tuplet,
        },
        notes,
        ..Default::default()
    }
}

/// One track with `tuning`, `bars` copies of `bar`, 4/4 at `bpm`.
fn score(tuning: Vec<u8>, bars: usize, bar: Bar, bpm: f64) -> Score {
    Score {
        title: "Robustness".into(),
        initial_tempo: bpm,
        tracks: vec![Track {
            name: "Guitar".into(),
            tuning,
            bars: vec![bar; bars],
            ..Default::default()
        }],
        master_bars: vec![MasterBar::default(); bars],
        ..Default::default()
    }
}

fn quarters(notes: Vec<Note>) -> Bar {
    Bar {
        voices: vec![Voice {
            beats: vec![beat(4, None, notes); 4],
        }],
    }
}

const STANDARD: [u8; 6] = [64, 59, 55, 50, 45, 40];

/// Run everything downstream of a parsed score, checking time and basic invariants.
fn exercise(score: &Score, budget: Elapsed) {
    let start = Instant::now();
    let timeline = Timeline::new(score);
    let duration = timeline.duration_secs();
    assert!(
        duration.is_finite() && duration >= 0.0,
        "duration {duration}"
    );
    for b in timeline.played_bars() {
        assert!(b.start_secs.is_finite() && b.end_secs.is_finite());
        assert!(b.end_secs >= b.start_secs);
    }
    for secs in [
        -1.0,
        0.0,
        duration / 3.0,
        duration,
        duration + 5.0,
        f64::NAN,
    ] {
        if let Some(p) = timeline.position_at_secs(secs) {
            assert!(
                p.fraction.is_finite()
                    || timeline.played_bars()[p.played_bar].end_tick
                        == timeline.played_bars()[p.played_bar].start_tick
            );
        }
    }
    let map = SyncMap::new(
        &SyncSpec::Points {
            points: vec![
                SyncPoint {
                    played_bar: 0,
                    video_secs: 1.0,
                },
                SyncPoint {
                    played_bar: 3,
                    video_secs: 9.0,
                },
            ],
        },
        &timeline,
    );
    assert!(map.video_to_score(5.0).is_finite());
    for track in 0..score.tracks.len() as u32 {
        for width in [1.0, 320.0, 1400.0] {
            let list = layout::layout(
                score,
                &LayoutOptions {
                    track,
                    width,
                    scale: 1.0,
                    show_rhythm: true,
                },
            );
            assert!(list.height.is_finite());
        }
    }
    let ev = events::build(score, &timeline);
    for e in ev.notes.iter().chain(&ev.clicks) {
        assert!(e.secs.is_finite(), "event at {}", e.secs);
        assert!(e.key <= 127);
    }
    let elapsed = start.elapsed();
    assert!(elapsed < budget, "took {elapsed:?} (budget {budget:?})");
}

// ---------------------------------------------------------------------------------------------
// Scores built in memory

#[test]
fn largest_importable_score_lays_out_and_plays_in_bounded_time() {
    // At the importers' bar limit: 10,000 bars of 32 sixteenths (≈ 1.3M notes over two tracks).
    let bar = Bar {
        voices: vec![Voice {
            beats: vec![beat(16, None, vec![note(1, 5), note(2, 7)]); 32],
        }],
    };
    let mut s = score(STANDARD.to_vec(), 10_000, bar, 200.0);
    s.master_bars[0].time_signature = TimeSignature {
        numerator: 8,
        denominator: 4,
    };
    s.tracks.push(s.tracks[0].clone());
    exercise(
        &s,
        Elapsed::from_secs(if cfg!(debug_assertions) { 120 } else { 15 }),
    );
}

#[test]
fn jumps_on_every_bar_unroll_quickly() {
    // The GP5 bar limit, with a jump and a target on every bar.
    let n = 10_000;
    let mut s = score(STANDARD.to_vec(), n, Bar::default(), 120.0);
    for (i, mb) in s.master_bars.iter_mut().enumerate() {
        mb.directions = vec![
            if i % 2 == 0 {
                Direction::DalSegnoAlCoda
            } else {
                Direction::DaCapoAlFine
            },
            Direction::Segno,
            Direction::DaCoda,
        ];
        mb.repeat_end = 255;
    }
    s.master_bars[n - 1].directions.push(Direction::Coda);
    let start = Instant::now();
    let timeline = Timeline::new(&s);
    assert!(timeline.played_bars().len() <= n * 10 + 1000);
    assert!(
        start.elapsed() < Elapsed::from_secs(2),
        "{:?}",
        start.elapsed()
    );
}

#[test]
fn many_bars_with_repeats_and_dal_segno_stay_capped() {
    let n = 2_000;
    let mut s = score(STANDARD.to_vec(), n, quarters(vec![note(1, 3)]), 120.0);
    // A repeat every 4 bars (×4), then a D.S. back to a Segno near the start: the unrolled
    // timeline must stay within the unroll cap.
    for i in (0..n).step_by(4) {
        s.master_bars[i].repeat_start = true;
        s.master_bars[i + 3].repeat_end = 4;
    }
    s.master_bars[1].directions.push(Direction::Segno);
    s.master_bars[n - 1].directions.push(Direction::DalSegno);
    s.master_bars[n - 2].directions.push(Direction::DaCapo);
    let timeline = Timeline::new(&s);
    assert!(timeline.played_bars().len() <= n * 10 + 1000);
    exercise(&s, Elapsed::from_secs(10));
}

#[test]
fn sixty_fourth_tuplets_everywhere_layout_is_bounded() {
    // 4/4 filled with 64th-note 7:4 septuplets in two voices, chords on six strings.
    let chord: Vec<Note> = (1..=6).map(|s| note(s, 12)).collect();
    let septuplet = beat(64, Some((7, 4)), chord);
    let beats = vec![septuplet; 7 * 16];
    let bar = Bar {
        voices: vec![
            Voice {
                beats: beats.clone(),
            },
            Voice { beats },
        ],
    };
    let s = score(STANDARD.to_vec(), 32, bar, 120.0);
    // Warm up, then time a single layout.
    let opts = LayoutOptions {
        track: 0,
        width: 1200.0,
        scale: 1.0,
        show_rhythm: true,
    };
    let _ = layout::layout(&s, &opts);
    let start = Instant::now();
    let list = layout::layout(&s, &opts);
    let elapsed = start.elapsed();
    assert!(!list.systems.is_empty());
    // Generous in debug builds; the 50 ms target is for release.
    let budget = if cfg!(debug_assertions) { 2_000 } else { 50 };
    assert!(
        elapsed < Elapsed::from_millis(budget),
        "layout took {elapsed:?}"
    );
    exercise(&s, Elapsed::from_secs(20));
}

#[test]
fn zero_length_and_empty_bars() {
    // A beat whose duration rounds to 0 ticks, an empty bar, and a voice-less bar.
    let zero = beat(128, Some((255, 1)), vec![note(1, 0)]);
    assert_eq!(zero.duration.ticks(), 0);
    let mut s = score(
        STANDARD.to_vec(),
        3,
        Bar {
            voices: vec![Voice {
                beats: vec![zero; 8],
            }],
        },
        120.0,
    );
    s.tracks[0].bars[1] = Bar::default();
    s.tracks[0].bars[2] = Bar {
        voices: vec![Voice::default(), Voice::default()],
    };
    s.master_bars[0].time_signature = TimeSignature {
        numerator: 0,
        denominator: 0,
    };
    exercise(&s, Elapsed::from_secs(5));
}

#[test]
fn absurd_tempos() {
    for bpm in [
        0.0,
        -120.0,
        f64::NAN,
        f64::INFINITY,
        f64::MIN_POSITIVE,
        1e-300,
        1e-3,
        1e9,
        1e300,
    ] {
        let mut s = score(STANDARD.to_vec(), 8, quarters(vec![note(2, 5)]), bpm);
        s.tempo_changes = vec![
            TempoChange {
                bar: 2,
                position: 0.5,
                bpm,
            },
            TempoChange {
                bar: 4,
                position: f64::NAN,
                bpm: 1e-300,
            },
            TempoChange {
                bar: 5,
                position: 0.0,
                bpm: 1e300,
            },
            TempoChange {
                bar: 99,
                position: 0.0,
                bpm: 60.0,
            },
        ];
        exercise(&s, Elapsed::from_secs(5));
    }
}

#[test]
fn string_counts_and_extreme_frets() {
    for strings in [0usize, 1, 7, 30] {
        let tuning: Vec<u8> = (0..strings)
            .map(|i| 90u8.saturating_sub(i as u8 * 3))
            .collect();
        let notes = (1..=40u8).map(|s| note(s, 255)).collect();
        let mut bar = quarters(notes);
        bar.voices[0].beats[1].notes = vec![note(0, 0), note(255, 255)];
        let mut s = score(tuning.clone(), 4, bar, 120.0);
        let percussion = Track {
            is_percussion: true,
            tuning,
            ..s.tracks[0].clone()
        };
        s.tracks.push(percussion);
        exercise(&s, Elapsed::from_secs(5));
    }
}

#[test]
fn tremolo_on_huge_beats_is_bounded() {
    // A whole note stretched by a 1:255 tuplet with 32nd tremolo picking: tens of thousands of
    // re-picks per note if unbounded.
    let mut b = beat(1, Some((1, 255)), vec![note(1, 0), note(2, 0), note(3, 0)]);
    b.effects = BeatEffects {
        tremolo_picking: Some(32),
        ..Default::default()
    };
    let bar = Bar {
        voices: vec![Voice { beats: vec![b; 64] }],
    };
    let mut s = score(STANDARD.to_vec(), 50, bar, 120.0);
    for mb in &mut s.master_bars {
        mb.repeat_start = true;
        mb.repeat_end = 255;
    }
    let timeline = Timeline::new(&s);
    let start = Instant::now();
    let ev = events::build(&s, &timeline);
    assert!(start.elapsed() < Elapsed::from_secs(10));
    assert!(ev.notes.len() <= 2 * 4_000_000, "{} events", ev.notes.len());
    assert!(ev.clicks.len() <= 4_000_000, "{} clicks", ev.clicks.len());
}

#[test]
fn layout_with_hostile_options() {
    let s = score(STANDARD.to_vec(), 40, quarters(vec![note(1, 1)]), 120.0);
    for (width, scale) in [
        (f32::INFINITY, 1.0),
        (1e9, 1.0),
        (1e30, 1e30),
        (900.0, 1e-30),
        (900.0, f32::MIN_POSITIVE),
        (900.0, 1e6),
        (-5.0, 1.0),
        (f32::NAN, f32::NAN),
        (0.0, 0.0),
        (1e-6, 1.0),
    ] {
        let start = Instant::now();
        let list = layout::layout(
            &s,
            &LayoutOptions {
                track: 0,
                width,
                scale,
                show_rhythm: true,
            },
        );
        let _ = layout::cursor(&list, 3, 100, 3840);
        let _ = layout::hit_test(&list, 10.0, 10.0);
        assert!(
            start.elapsed() < Elapsed::from_secs(2),
            "width {width} scale {scale}: {:?}",
            start.elapsed()
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Hostile files

/// A GP5 5.00 header (everything before the master bars) declaring `bars` bars and `tracks`
/// tracks; the caller appends whatever follows.
fn gp5_header(bars: i32, tracks: i32) -> Vec<u8> {
    let mut b = Vec::new();
    let version = b"FICHIER GUITAR PRO v5.00";
    b.push(version.len() as u8);
    b.extend_from_slice(version);
    b.resize(31, 0);
    let empty_string = |b: &mut Vec<u8>| {
        b.extend_from_slice(&1i32.to_le_bytes());
        b.push(0);
    };
    for _ in 0..9 {
        empty_string(&mut b);
    }
    b.extend_from_slice(&0i32.to_le_bytes()); // notice lines
    b.extend_from_slice(&0i32.to_le_bytes()); // lyrics track
    for _ in 0..5 {
        b.extend_from_slice(&0i32.to_le_bytes());
        b.extend_from_slice(&0i32.to_le_bytes());
    }
    b.extend_from_slice(&[0; 30]); // page setup
    for _ in 0..10 {
        empty_string(&mut b);
    }
    empty_string(&mut b); // tempo name
    b.extend_from_slice(&120i32.to_le_bytes());
    b.push(0); // key
    b.extend_from_slice(&[0; 4]); // octave
    for _ in 0..64 {
        b.extend_from_slice(&25i32.to_le_bytes());
        b.extend_from_slice(&[0; 8]);
    }
    b.extend_from_slice(&[0xff; 19 * 2]); // directions: none
    b.extend_from_slice(&[0; 4]); // master reverb
    b.extend_from_slice(&bars.to_le_bytes());
    b.extend_from_slice(&tracks.to_le_bytes());
    b
}

#[test]
fn gp5_huge_counts_with_tiny_data_fail_fast() {
    for (bars, tracks) in [
        (100_000, 1),
        (1, 100_000),
        (i32::MAX, i32::MAX),
        (-1, 1),
        (10_000, 128),
    ] {
        let bytes = gp5_header(bars, tracks);
        let start = Instant::now();
        assert!(formats::parse(&bytes, Some("gp5")).is_err());
        assert!(
            start.elapsed() < Elapsed::from_millis(500),
            "{bars}×{tracks}"
        );
    }
}

/// A `.gp`-less GPIF document: one track, `master_bars` master bars all pointing at bar 0, whose
/// voice lists beat 0 `beats_per_voice` times.
fn gpif_amplifier(master_bars: usize, beats_per_voice: usize) -> Vec<u8> {
    let mut x = String::from(
        "<GPIF><MasterTrack><Tracks>0</Tracks></MasterTrack>\
         <Tracks><Track id=\"0\"><Name>T</Name></Track></Tracks><MasterBars>",
    );
    for _ in 0..master_bars {
        x.push_str("<MasterBar><Time>4/4</Time><Bars>0</Bars></MasterBar>");
    }
    x.push_str("</MasterBars><Bars><Bar id=\"0\"><Voices>0</Voices></Bar></Bars>");
    x.push_str("<Voices><Voice id=\"0\"><Beats>");
    for _ in 0..beats_per_voice {
        x.push_str("0 ");
    }
    x.push_str("</Beats></Voice></Voices>");
    x.push_str(
        "<Beats><Beat id=\"0\"><Rhythm ref=\"0\"/><Notes>0 0 0 0</Notes></Beat></Beats>\
         <Notes><Note id=\"0\"><Properties><Property name=\"Midi\"><Number>60</Number>\
         </Property></Properties></Note></Notes>\
         <Rhythms><Rhythm id=\"0\"><NoteValue>128th</NoteValue></Rhythm></Rhythms></GPIF>",
    );
    x.into_bytes()
}

#[test]
fn gpif_reference_amplification_is_rejected() {
    // ~1 MB of XML that would otherwise expand to 2.5 billion beats.
    let xml = gpif_amplifier(10_000, 250_000);
    let start = Instant::now();
    let result = formats::gpif::parse_gpif(&xml);
    assert!(result.is_err(), "expected an error");
    assert!(
        start.elapsed() < Elapsed::from_secs(5),
        "{:?}",
        start.elapsed()
    );
}

#[test]
fn gpif_too_many_master_bars_or_tracks_is_rejected() {
    let xml = gpif_amplifier(200_000, 1);
    assert!(formats::gpif::parse_gpif(&xml).is_err());

    let mut x = String::from("<GPIF><Tracks>");
    for i in 0..5_000 {
        x.push_str(&format!("<Track id=\"{i}\"/>"));
    }
    x.push_str("</Tracks><MasterBars>");
    for _ in 0..1_000 {
        x.push_str("<MasterBar><Bars>0</Bars></MasterBar>");
    }
    x.push_str("</MasterBars></GPIF>");
    assert!(formats::gpif::parse_gpif(x.as_bytes()).is_err());
}

#[test]
fn gpif_deeply_nested_and_huge_xml() {
    // 200k levels of nesting: must not overflow the stack.
    let depth = 200_000;
    let mut x = String::from("<GPIF>");
    x.push_str(&"<a>".repeat(depth));
    x.push_str(&"</a>".repeat(depth));
    x.push_str("<Score><Title>deep</Title></Score></GPIF>");
    let start = Instant::now();
    let score = formats::gpif::parse_gpif(x.as_bytes()).unwrap();
    assert_eq!(score.title, "deep");
    // An enormous attribute and text node.
    let big = "x".repeat(8 << 20);
    let x = format!("<GPIF a=\"{big}\"><Score><Title>{big}</Title></Score></GPIF>");
    let score = formats::gpif::parse_gpif(x.as_bytes()).unwrap();
    assert_eq!(score.title.len(), big.len());
    // Unclosed deep nesting is an error, not a crash.
    let x = format!("<GPIF>{}", "<a>".repeat(depth));
    assert!(formats::gpif::parse_gpif(x.as_bytes()).is_err());
    assert!(start.elapsed() < Elapsed::from_secs(10));
}

#[test]
fn gpif_absurd_values_survive_the_pipeline() {
    let xml = br#"<GPIF>
      <MasterTrack><Tracks>0</Tracks><Automations>
        <Automation><Type>Tempo</Type><Bar>0</Bar><Position>0</Position><Value>1e-300 2</Value></Automation>
        <Automation><Type>Tempo</Type><Bar>1</Bar><Position>0.5</Position><Value>1e300 2</Value></Automation>
        <Automation><Type>SyncPoint</Type><Bar>0</Bar><Position>0</Position>
          <Value><BarIndex>0</BarIndex><BarOccurrence>0</BarOccurrence><FrameOffset>1e308</FrameOffset></Value></Automation>
      </Automations></MasterTrack>
      <Tracks><Track id="0"><Properties><Property name="Tuning"><Pitches>40 45 50 55 59 64 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24</Pitches></Property></Properties></Track></Tracks>
      <MasterBars>
        <MasterBar><Time>255/1</Time><Bars>0</Bars></MasterBar>
        <MasterBar><Time>1/128</Time><Bars>0</Bars><Repeat start="true" end="true" count="999999"/></MasterBar>
      </MasterBars>
      <Bars><Bar id="0"><Voices>0 0 0 0 0 0</Voices></Bar></Bars>
      <Voices><Voice id="0"><Beats>0 1</Beats></Voice></Voices>
      <Beats>
        <Beat id="0"><Rhythm ref="0"/><Notes>0</Notes><Tremolo>1/8</Tremolo></Beat>
        <Beat id="1"><Rhythm ref="1"/><Notes>0</Notes></Beat>
      </Beats>
      <Notes><Note id="0"><Properties>
        <Property name="String"><String>29</String></Property>
        <Property name="Fret"><Fret>255</Fret></Property>
      </Properties></Note></Notes>
      <Rhythms>
        <Rhythm id="0"><NoteValue>Whole</NoteValue><AugmentationDot count="2"/><PrimaryTuplet num="1" den="255"/></Rhythm>
        <Rhythm id="1"><NoteValue>128th</NoteValue><PrimaryTuplet num="255" den="1"/></Rhythm>
      </Rhythms>
    </GPIF>"#;
    let score = formats::gpif::parse_gpif(xml).unwrap();
    exercise(&score, Elapsed::from_secs(10));
}
