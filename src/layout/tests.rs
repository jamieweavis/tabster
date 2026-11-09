use super::*;
use crate::model::{
    Bar, Beat, Bend, Direction, Duration, Harmonic, MasterBar, Note, Score, Section, Slide,
    TempoChange, TimeSignature, Track, Voice,
};

// ---------------------------------------------------------------------------------------------
// Score builder

struct ScoreBuilder {
    score: Score,
}

impl ScoreBuilder {
    fn new(strings: usize) -> Self {
        let tuning = [64, 59, 55, 50, 45, 40, 35, 30][..strings].to_vec();
        Self {
            score: Score {
                title: "Test".into(),
                initial_tempo: 120.0,
                tracks: vec![Track {
                    name: "Guitar".into(),
                    tuning,
                    ..Default::default()
                }],
                ..Default::default()
            },
        }
    }

    fn bar(self, beats: Vec<Beat>) -> Self {
        self.bar_with(MasterBar::default(), vec![beats])
    }

    fn bar_with(mut self, mb: MasterBar, voices: Vec<Vec<Beat>>) -> Self {
        let mb = if self.score.master_bars.is_empty() || mb != MasterBar::default() {
            mb
        } else {
            // Inherit the previous time signature.
            MasterBar {
                time_signature: self.score.master_bars.last().unwrap().time_signature,
                ..mb
            }
        };
        self.score.master_bars.push(mb);
        self.score.tracks[0].bars.push(Bar {
            voices: voices.into_iter().map(|beats| Voice { beats }).collect(),
        });
        self
    }

    fn build(self) -> Score {
        self.score
    }
}

fn dur(value: u8) -> Duration {
    Duration {
        value,
        dots: 0,
        tuplet: None,
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

fn beat(value: u8, notes: &[(u8, u8)]) -> Beat {
    Beat {
        duration: dur(value),
        notes: notes.iter().map(|&(s, f)| note(s, f)).collect(),
        ..Default::default()
    }
}

fn rest(value: u8) -> Beat {
    Beat {
        duration: dur(value),
        ..Default::default()
    }
}

/// Apply `f` to every note of the beat.
fn fx(mut b: Beat, f: impl Fn(&mut Note)) -> Beat {
    b.notes.iter_mut().for_each(f);
    b
}

fn eighths(frets: &[(u8, u8)]) -> Vec<Beat> {
    frets.iter().map(|&n| beat(8, &[n])).collect()
}

fn opts(width: f32) -> LayoutOptions {
    LayoutOptions {
        track: 0,
        width,
        scale: 1.0,
        show_rhythm: true,
    }
}

/// A plain score of `bars` bars of eight eighth notes.
fn simple(bars: usize) -> Score {
    let mut b = ScoreBuilder::new(6);
    for i in 0..bars {
        let f = (i % 12) as u8;
        b = b.bar(eighths(&[
            (6, f),
            (5, f + 2),
            (4, f + 2),
            (3, f + 1),
            (2, f),
            (1, f + 3),
            (2, f),
            (3, f + 1),
        ]));
    }
    b.build()
}

/// A score exercising most of what the layout draws.
fn demo() -> Score {
    let ts68 = TimeSignature {
        numerator: 6,
        denominator: 8,
    };
    let pm = |n: &mut Note| n.effects.palm_mute = true;
    let ring = |n: &mut Note| n.effects.let_ring = true;
    let hp = |n: &mut Note| n.effects.hammer_pull = true;
    let mut triplet = beat(8, &[(3, 7)]);
    triplet.duration.tuplet = Some((3, 2));
    let mut dotted = beat(8, &[(2, 8)]);
    dotted.duration.dots = 1;
    let mut chord = beat(4, &[(1, 0), (2, 1), (3, 0), (4, 2), (5, 3)]);
    chord.chord = Some("C".into());
    let mut bend = beat(4, &[(2, 15)]);
    bend.notes[0].effects.bend = Some(Bend {
        points: vec![(0.0, 0.0), (0.5, 2.0), (1.0, 2.0)],
    });
    bend.notes[0].effects.vibrato = true;
    let mut half_bend = beat(4, &[(3, 14)]);
    half_bend.notes[0].effects.bend = Some(Bend {
        points: vec![(0.0, 0.0), (1.0, 1.0)],
    });
    let mut slide = beat(8, &[(3, 5)]);
    slide.notes[0].effects.slide = Some(Slide::Legato);
    let mut harmonic = beat(2, &[(3, 12), (2, 12)]);
    harmonic
        .notes
        .iter_mut()
        .for_each(|n| n.effects.harmonic = Some(Harmonic::Natural));
    let mut tie = beat(4, &[(1, 12)]);
    tie.notes[0].tie_destination = true;
    let mut ghost = beat(8, &[(4, 5)]);
    ghost.notes[0].effects.ghost = true;
    let mut dead = beat(16, &[(4, 0), (5, 0)]);
    dead.notes.iter_mut().for_each(|n| n.effects.dead = true);
    let section = |letter: &str, text: &str| {
        Some(Section {
            letter: letter.into(),
            text: text.into(),
        })
    };

    let mut score = ScoreBuilder::new(6)
        .bar_with(
            MasterBar {
                section: section("A", "Intro"),
                repeat_start: true,
                directions: vec![Direction::Segno],
                ..Default::default()
            },
            vec![
                [
                    (6, 0),
                    (6, 0),
                    (6, 3),
                    (6, 0),
                    (6, 5),
                    (6, 0),
                    (6, 3),
                    (6, 0),
                ]
                .iter()
                .map(|&n| fx(beat(8, &[n]), pm))
                .collect(),
            ],
        )
        .bar_with(
            MasterBar {
                repeat_end: 3,
                ..Default::default()
            },
            vec![vec![
                fx(beat(16, &[(6, 0)]), pm),
                fx(beat(16, &[(6, 0)]), pm),
                fx(beat(8, &[(6, 0)]), pm),
                fx(beat(8, &[(5, 2)]), hp),
                beat(8, &[(5, 4)]),
                dead.clone(),
                dead,
                beat(8, &[(5, 2), (4, 2)]),
                rest(4),
            ]],
        )
        .bar_with(
            MasterBar {
                section: section("B", "Verse"),
                ..Default::default()
            },
            vec![vec![
                fx(beat(4, &[(5, 3), (4, 5), (3, 5)]), ring),
                fx(beat(8, &[(4, 5)]), ring),
                fx(beat(8, &[(3, 4)]), ring),
                chord,
                rest(8),
                ghost,
            ]],
        )
        .bar(vec![
            triplet.clone(),
            fx(triplet.clone(), hp),
            {
                let mut t = triplet.clone();
                t.notes[0].fret = 5;
                t
            },
            dotted,
            beat(16, &[(2, 7)]),
            slide,
            beat(8, &[(3, 9)]),
            beat(4, &[(2, 10)]),
        ])
        .bar_with(
            MasterBar {
                repeat_start: true,
                ..Default::default()
            },
            vec![
                vec![
                    bend,
                    half_bend,
                    fx(beat(8, &[(1, 12)]), hp),
                    beat(8, &[(1, 15)]),
                    fx(beat(8, &[(1, 15)]), hp),
                    beat(8, &[(1, 12)]),
                ],
                vec![beat(2, &[(6, 0)]), beat(2, &[(6, 3)])],
            ],
        )
        .bar_with(
            MasterBar {
                alternate_endings: 1,
                repeat_end: 2,
                ..Default::default()
            },
            vec![vec![harmonic, tie, rest(4)]],
        )
        .bar_with(
            MasterBar {
                alternate_endings: 2,
                double_bar: true,
                directions: vec![Direction::DalSegnoAlCoda],
                ..Default::default()
            },
            vec![vec![beat(1, &[(6, 0), (5, 2), (4, 2)])]],
        )
        .bar_with(
            MasterBar {
                time_signature: ts68,
                section: section("C", "Bridge"),
                directions: vec![Direction::Coda],
                ..Default::default()
            },
            vec![eighths(&[(3, 2), (2, 3), (1, 2), (3, 2), (2, 3), (1, 2)])],
        );
    for _ in 0..3 {
        score = score.bar_with(
            MasterBar {
                time_signature: ts68,
                ..Default::default()
            },
            vec![vec![
                {
                    let mut b = beat(4, &[(4, 0), (3, 2)]);
                    b.duration.dots = 1;
                    b
                },
                beat(8, &[(2, 3)]),
                beat(16, &[(2, 5)]),
                beat(16, &[(2, 3)]),
                beat(8, &[(1, 2)]),
            ]],
        );
    }
    score = score
        .bar(vec![beat(2, &[(6, 3), (5, 2), (1, 3)]), rest(2)])
        .bar(vec![rest(1)]);
    let mut score = score.build();
    // Time signatures for the 4/4 bars after the 6/8 section.
    let n = score.master_bars.len();
    score.master_bars[n - 2].time_signature = TimeSignature::default();
    score.master_bars[n - 1].time_signature = TimeSignature::default();
    score.tempo_changes = vec![TempoChange {
        bar: 7,
        position: 0.0,
        bpm: 96.0,
    }];
    score
}

// ---------------------------------------------------------------------------------------------
// Helpers

fn all_bars(list: &DisplayList) -> impl Iterator<Item = &BarBox> {
    list.systems.iter().flat_map(|s| s.bars.iter())
}

fn right(r: &Rect) -> f32 {
    r.x + r.width
}

fn r1(v: f32) -> f32 {
    (v * 10.0).round() / 10.0
}

fn round_rect(r: Rect) -> Rect {
    Rect {
        x: r1(r.x),
        y: r1(r.y),
        width: r1(r.width),
        height: r1(r.height),
    }
}

/// The list with every coordinate rounded to 0.1pt, for stable snapshots.
fn rounded(list: &DisplayList) -> DisplayList {
    let mut list = list.clone();
    list.width = r1(list.width);
    list.height = r1(list.height);
    for s in &mut list.systems {
        s.frame = round_rect(s.frame);
        for b in &mut s.bars {
            b.frame = round_rect(b.frame);
            for beat in &mut b.beats {
                beat.x = r1(beat.x);
            }
        }
        for p in &mut s.primitives {
            match p {
                Primitive::Line {
                    x1,
                    y1,
                    x2,
                    y2,
                    width,
                    ..
                } => {
                    for v in [x1, y1, x2, y2, width] {
                        *v = r1(*v);
                    }
                }
                Primitive::Rect { rect, .. } | Primitive::Knockout { rect } => {
                    *rect = round_rect(*rect)
                }
                Primitive::Text { x, y, size, .. } => {
                    for v in [x, y, size] {
                        *v = r1(*v);
                    }
                }
                Primitive::Curve {
                    x1,
                    y1,
                    cx,
                    cy,
                    x2,
                    y2,
                    width,
                    ..
                } => {
                    for v in [x1, y1, cx, cy, x2, y2, width] {
                        *v = r1(*v);
                    }
                }
                Primitive::Dot { x, y, radius, .. } => {
                    for v in [x, y, radius] {
                        *v = r1(*v);
                    }
                }
            }
        }
    }
    list
}

fn check_invariants(score: &Score, list: &DisplayList) {
    let ids: Vec<u32> = all_bars(list).map(|b| b.master_bar).collect();
    let expected: Vec<u32> = (0..score.master_bars.len() as u32).collect();
    assert_eq!(ids, expected, "every master bar exactly once, in order");
    let mut prev_bottom = 0.0;
    for s in &list.systems {
        assert!(!s.bars.is_empty());
        assert!(
            s.frame.y >= prev_bottom - 0.01,
            "systems stack top to bottom"
        );
        prev_bottom = s.frame.y + s.frame.height;
        for w in s.bars.windows(2) {
            assert!(
                (right(&w[0].frame) - w[1].frame.x).abs() < 0.01,
                "bars abut"
            );
        }
        for b in &s.bars {
            assert!(!b.beats.is_empty());
            assert!(b.frame.y >= s.frame.y && right(&b.frame) <= right(&s.frame) + 0.01);
            for w in b.beats.windows(2) {
                assert!(w[0].tick_in_bar < w[1].tick_in_bar);
                assert!(w[0].x < w[1].x, "beat x strictly increasing");
            }
            for beat in &b.beats {
                assert!(beat.x > b.frame.x && beat.x < right(&b.frame));
            }
        }
    }
    assert!((list.height - prev_bottom).abs() < 0.01);
}

// ---------------------------------------------------------------------------------------------
// Tests

#[test]
fn empty_score_and_bad_track() {
    assert_eq!(
        layout(&Score::default(), &opts(800.0)),
        DisplayList::default()
    );
    let score = simple(4);
    let bad = LayoutOptions {
        track: 3,
        ..opts(800.0)
    };
    assert_eq!(layout(&score, &bad), DisplayList::default());
    assert_eq!(layout(&score, &opts(0.0)), DisplayList::default());
    let list = DisplayList::default();
    assert_eq!(cursor(&list, 0, 0, 960), None);
    assert_eq!(hit_test(&list, 10.0, 10.0), None);
}

#[test]
fn line_breaking_by_width() {
    let score = simple(24);
    let mut prev_systems = usize::MAX;
    for width in [250.0, 400.0, 700.0, 1000.0, 1600.0, 3000.0] {
        let list = layout(&score, &opts(width));
        check_invariants(&score, &list);
        let n = list.systems.len();
        assert!(n <= prev_systems, "wider never needs more systems");
        prev_systems = n;
        assert_eq!(list.width, width);
    }
    // A narrow view puts one bar per system and compresses it to fit.
    let list = layout(&score, &opts(150.0));
    check_invariants(&score, &list);
    assert_eq!(list.systems.len(), 24);
}

#[test]
fn justification_fills_width() {
    let score = simple(10);
    let width = 900.0;
    let list = layout(&score, &opts(width));
    assert!(list.systems.len() > 1);
    let margin = 16.0;
    for (k, s) in list.systems.iter().enumerate() {
        let end = right(&s.bars.last().unwrap().frame);
        if k + 1 < list.systems.len() {
            assert!(
                (end - (width - margin)).abs() < 0.5,
                "system {k} ends at {end}"
            );
        } else {
            assert!(end <= width - margin + 0.5);
        }
    }
}

#[test]
fn last_system_not_overstretched() {
    let score = simple(1);
    let list = layout(&score, &opts(2000.0));
    let bar = &list.systems[0].bars[0];
    assert!(bar.frame.width < 600.0, "one bar doesn't fill 2000pt");
}

#[test]
fn demo_score_invariants() {
    let score = demo();
    for width in [300.0, 800.0, 1200.0] {
        for show_rhythm in [true, false] {
            let o = LayoutOptions {
                show_rhythm,
                ..opts(width)
            };
            check_invariants(&score, &layout(&score, &o));
        }
    }
}

#[test]
fn multi_voice_onsets_merged() {
    let score = ScoreBuilder::new(6)
        .bar_with(
            MasterBar::default(),
            vec![
                vec![beat(2, &[(1, 0)]), beat(2, &[(1, 2)])],
                vec![beat(4, &[(6, 0)]); 4],
            ],
        )
        .build();
    let list = layout(&score, &opts(800.0));
    let ticks: Vec<u32> = list.systems[0].bars[0]
        .beats
        .iter()
        .map(|b| b.tick_in_bar)
        .collect();
    assert_eq!(ticks, [0, 960, 1920, 2880]);

    // Voice 0 empty: falls back to the other voice.
    let score = ScoreBuilder::new(6)
        .bar_with(
            MasterBar::default(),
            vec![vec![], vec![beat(4, &[(6, 0)]); 4]],
        )
        .build();
    let list = layout(&score, &opts(800.0));
    assert_eq!(list.systems[0].bars[0].beats.len(), 4);
}

#[test]
fn cursor_interpolates() {
    let score = simple(6);
    let list = layout(&score, &opts(900.0));
    let bar = list.systems[0].bars[1].clone();
    let x = |t| cursor(&list, 1, t, 3840).unwrap().x;
    let b = &bar.beats;
    assert_eq!(x(0), b[0].x);
    assert!((x(240) - (b[0].x + b[1].x) / 2.0).abs() < 0.01);
    assert_eq!(x(480), b[1].x);
    let end = right(&bar.frame);
    let last = b.last().unwrap();
    assert!((x(3360 + 240) - (last.x + end) / 2.0).abs() < 0.01);
    assert_eq!(x(3840), end);
    assert_eq!(x(9999), end, "past the end clamps to the right edge");

    let c = cursor(&list, 1, 0, 3840).unwrap();
    assert_eq!(c.master_bar, 1);
    assert_eq!(c.system, 0);
    assert_eq!(c.bar_frame, bar.frame);
    assert_eq!((c.y, c.height), (bar.frame.y, bar.frame.height));

    // A bar on a later system.
    let last_sys = list.systems.len() - 1;
    let mb = list.systems[last_sys].bars[0].master_bar;
    assert_eq!(cursor(&list, mb, 0, 3840).unwrap().system, last_sys as u32);
    assert_eq!(cursor(&list, 99, 0, 3840), None);
}

#[test]
fn hit_test_round_trips_with_cursor() {
    let score = demo();
    let list = layout(&score, &opts(800.0));
    for bar in all_bars(&list) {
        let y = bar.frame.y + bar.frame.height / 2.0;
        for beat in &bar.beats {
            assert_eq!(
                hit_test(&list, beat.x, y),
                Some((bar.master_bar, beat.tick_in_bar))
            );
            let c = cursor(&list, bar.master_bar, beat.tick_in_bar, 3840).unwrap();
            assert_eq!(c.x, beat.x);
            assert_eq!(
                hit_test(&list, c.x + 0.5, c.y + 1.0),
                Some((bar.master_bar, beat.tick_in_bar))
            );
        }
    }
    // Outside any bar.
    assert_eq!(hit_test(&list, 2.0, 60.0), None);
    assert_eq!(hit_test(&list, 400.0, list.height + 10.0), None);
    assert_eq!(hit_test(&list, 400.0, -5.0), None);
}

#[test]
fn notes_have_knockouts_and_labels() {
    let score = demo();
    let list = layout(&score, &opts(1200.0));
    let texts: Vec<&str> = list
        .systems
        .iter()
        .flat_map(|s| &s.primitives)
        .filter_map(|p| match p {
            Primitive::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    for expected in [
        "A · Intro",
        "P.M.",
        "let ring",
        "x3",
        "1.",
        "2.",
        "x",
        "(5)",
        "<12>",
        "(12)",
        "full",
        "½",
        "H",
        "P",
        "3",
        "♩ = 120",
        "♩ = 96",
        "D.S. al Coda",
        "Segno",
        "C",
        "G",
    ] {
        assert!(texts.contains(&expected), "missing {expected:?}");
    }
    let knockouts = list
        .systems
        .iter()
        .flat_map(|s| &s.primitives)
        .filter(|p| matches!(p, Primitive::Knockout { .. }))
        .count();
    assert!(knockouts > 50);
}

#[test]
fn scale_and_rhythm_change_height() {
    let score = simple(8);
    let base = layout(&score, &opts(800.0));
    let no_rhythm = layout(
        &score,
        &LayoutOptions {
            show_rhythm: false,
            ..opts(800.0)
        },
    );
    assert!(no_rhythm.height < base.height);
    let big = layout(
        &score,
        &LayoutOptions {
            scale: 2.0,
            ..opts(800.0)
        },
    );
    assert!(big.systems.len() > base.systems.len());
}

#[test]
fn percussion_track() {
    let mut score = simple(4);
    score.tracks[0].is_percussion = true;
    score.tracks[0].tuning.clear();
    let list = layout(&score, &opts(800.0));
    check_invariants(&score, &list);
    let lines = list.systems[0]
        .primitives
        .iter()
        .filter(|p| matches!(p, Primitive::Line { y1, y2, .. } if y1 == y2))
        .count();
    assert!(lines >= 5);
}

#[test]
fn snapshot_small_score() {
    let score = ScoreBuilder::new(6)
        .bar_with(
            MasterBar {
                section: Some(Section {
                    letter: "A".into(),
                    text: "Riff".into(),
                }),
                ..Default::default()
            },
            vec![vec![
                beat(4, &[(6, 0), (5, 2)]),
                beat(8, &[(4, 2)]),
                beat(8, &[(3, 12)]),
                rest(4),
                beat(4, &[(1, 3)]),
            ]],
        )
        .bar(vec![
            beat(16, &[(2, 1)]),
            beat(16, &[(2, 3)]),
            beat(8, &[(2, 5)]),
            beat(2, &[(1, 0)]),
            beat(4, &[(3, 2)]),
        ])
        .bar_with(
            MasterBar {
                repeat_end: 2,
                ..Default::default()
            },
            vec![vec![beat(2, &[(6, 3)]), beat(2, &[(5, 2)])]],
        )
        .bar(vec![beat(1, &[(6, 0), (5, 2), (4, 2)])])
        .build();
    let list = layout(&score, &opts(500.0));
    insta::assert_debug_snapshot!(rounded(&list));
}

#[test]
fn svg_renders() {
    let score = demo();
    let list = layout(&score, &opts(1000.0));
    let svg = svg::to_svg(&list);
    assert!(svg.starts_with("<svg"));
    assert!(svg.contains("A · Intro"));
    // `TABSTER_LAYOUT_SVG=/path/out.svg cargo test svg_renders` writes the demo for eyeballing.
    if let Ok(path) = std::env::var("TABSTER_LAYOUT_SVG") {
        std::fs::write(path, svg).unwrap();
    }
}

/// Shaped like a real GP8 export: 145 bars of 4/4 at 166 bpm, sections every 8–32 bars, busy
/// eighth/sixteenth guitar lines with ties, plus a 4-string bass and a drum track.
fn song_like() -> Score {
    let sections = [
        (0, "Intro"),
        (8, "Verse 1"),
        (24, "Chorus"),
        (40, "Post-Chorus"),
        (48, "Verse 2"),
        (64, "Chorus"),
        (80, "Post-Chorus"),
        (88, "Bridge"),
        (120, "Chorus"),
        (136, "Outro"),
    ];
    let mut score = Score {
        title: "Calibration".into(),
        initial_tempo: 166.0,
        ..Default::default()
    };
    let mut guitar = Track {
        name: "Guitar".into(),
        tuning: vec![64, 59, 55, 50, 45, 40],
        ..Default::default()
    };
    let mut bass = Track {
        name: "Bass".into(),
        tuning: vec![43, 38, 33, 28],
        ..Default::default()
    };
    let mut drums = Track {
        name: "Drums".into(),
        is_percussion: true,
        ..Default::default()
    };
    for i in 0..145usize {
        let section = sections.iter().position(|&(b, _)| b == i).map(|k| Section {
            letter: char::from(b'A' + k as u8).to_string(),
            text: sections[k].1.into(),
        });
        score.master_bars.push(MasterBar {
            section,
            ..Default::default()
        });
        let f = (i % 7) as u8 + 2;
        let mut g = Vec::new();
        for k in 0..6u8 {
            let value = if k % 3 == 2 { 8 } else { 16 };
            let mut b = beat(value, &[(k % 3 + 2, f + (k % 2) * 2)]);
            if k == 0 && i % 4 == 1 {
                b.notes[0].tie_destination = true;
            }
            g.push(b);
        }
        g.extend([
            beat(8, &[(1, f + 10)]),
            beat(8, &[(2, f + 8)]),
            beat(4, &[(3, f)]),
        ]);
        guitar.bars.push(Bar {
            voices: vec![Voice { beats: g }],
        });
        let b = vec![
            beat(4, &[(4, f)]),
            beat(8, &[(4, f)]),
            beat(8, &[(3, f)]),
            beat(2, &[(4, f + 2)]),
        ];
        bass.bars.push(Bar {
            voices: vec![Voice { beats: b }],
        });
        drums.bars.push(Bar {
            voices: vec![Voice {
                beats: (0..8).map(|k| beat(8, &[(1, 36 + k)])).collect(),
            }],
        });
    }
    score.tracks = vec![guitar, bass, drums];
    score
}

#[test]
fn song_like_all_tracks() {
    let score = song_like();
    for track in 0..3 {
        for width in [320.0, 900.0, 1400.0] {
            let list = layout(
                &score,
                &LayoutOptions {
                    track,
                    ..opts(width)
                },
            );
            check_invariants(&score, &list);
        }
    }
    // `TABSTER_LAYOUT_SVG_DIR=/dir cargo test song_like` writes each track for eyeballing.
    if let Ok(dir) = std::env::var("TABSTER_LAYOUT_SVG_DIR") {
        for (track, name) in [(0, "guitar"), (1, "bass"), (2, "drums")] {
            let list = layout(
                &score,
                &LayoutOptions {
                    track,
                    ..opts(1100.0)
                },
            );
            std::fs::write(format!("{dir}/song_{name}.svg"), svg::to_svg(&list)).unwrap();
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Rests, ties, drums, labels

fn texts(list: &DisplayList) -> Vec<(f32, f32, &str)> {
    list.systems
        .iter()
        .flat_map(|s| &s.primitives)
        .filter_map(|p| match p {
            Primitive::Text { x, y, text, .. } => Some((*x, *y, text.as_str())),
            _ => None,
        })
        .collect()
}

/// Texts below the tab staff (multi-rest counts, tuplet numbers).
fn below_staff(list: &DisplayList) -> Vec<&str> {
    list.systems
        .iter()
        .flat_map(|s| {
            let bottom = s.bars[0].frame.y + s.bars[0].frame.height;
            s.primitives.iter().filter_map(move |p| match p {
                Primitive::Text { y, text, .. } if *y > bottom => Some(text.as_str()),
                _ => None,
            })
        })
        .collect()
}

fn curves(list: &DisplayList) -> usize {
    list.systems
        .iter()
        .flat_map(|s| &s.primitives)
        .filter(|p| matches!(p, Primitive::Curve { .. }))
        .count()
}

/// 2 busy bars, `n` whole-rest bars, 2 busy bars.
fn with_rests(n: usize) -> Score {
    let mut score = simple(2);
    for _ in 0..n {
        score.master_bars.push(MasterBar::default());
        score.tracks[0].bars.push(Bar {
            voices: vec![Voice {
                beats: vec![rest(1)],
            }],
        });
    }
    let tail = simple(2);
    score.master_bars.extend(tail.master_bars);
    score.tracks[0].bars.extend(tail.tracks[0].bars.clone());
    score
}

#[test]
fn multi_rest_bars_cover_each_bar_and_cursor_glides() {
    let score = with_rests(6);
    for width in [300.0, 800.0, 1400.0] {
        let list = layout(&score, &opts(width));
        check_invariants(&score, &list);
        let bars: Vec<&BarBox> = all_bars(&list)
            .filter(|b| (2..8).contains(&b.master_bar))
            .collect();
        assert_eq!(bars.len(), 6, "one slice per collapsed bar");
        let sys = |mb: u32| cursor(&list, mb, 0, 3840).unwrap().system;
        assert!((2..8).all(|mb| sys(mb) == sys(2)), "multi-rest never split");
        assert_eq!(below_staff(&list), ["6"], "bar count drawn once");
        let mut last_x = f32::MIN;
        for mb in 2..8u32 {
            for t in (0..=3840).step_by(240) {
                let c = cursor(&list, mb, t, 3840).unwrap();
                assert!(c.x >= last_x - 0.01, "cursor moves forward ({mb}, {t})");
                assert!(c.x >= c.bar_frame.x && c.x <= right(&c.bar_frame) + 0.01);
                last_x = c.x;
            }
        }
        // Clicking a slice seeks to its bar.
        for b in &bars {
            let y = b.frame.y + b.frame.height / 2.0;
            let x = b.frame.x + b.frame.width / 2.0;
            assert_eq!(hit_test(&list, x, y), Some((b.master_bar, 0)));
        }
        if list.systems.len() == 1 {
            let total: f32 = bars.iter().map(|b| b.frame.width).sum();
            assert!(total < 160.0, "multi-rest is compact ({total})");
        }
    }

    // A section breaks the run in two; so does a repeat end.
    let mut score = with_rests(6);
    score.master_bars[5].section = Some(Section {
        letter: String::new(),
        text: "Verse".into(),
    });
    let list = layout(&score, &opts(1200.0));
    check_invariants(&score, &list);
    assert_eq!(below_staff(&list), ["3", "3"]);
    assert!(texts(&list).iter().any(|t| t.2 == "Verse"));
    score.master_bars[3].repeat_end = 2;
    let list = layout(&score, &opts(1200.0));
    check_invariants(&score, &list);
    assert_eq!(below_staff(&list), ["2", "3"]);

    // A single rest bar isn't a multi-rest.
    let score = with_rests(1);
    let list = layout(&score, &opts(1200.0));
    check_invariants(&score, &list);
    assert!(below_staff(&list).is_empty());
}

#[test]
fn rest_only_bars_are_narrow() {
    // Busy, busy, rest, busy, busy, rest (two half rests).
    let mut score = with_rests(1);
    score.master_bars.push(MasterBar::default());
    score.tracks[0].bars.push(Bar {
        voices: vec![Voice {
            beats: vec![rest(2), rest(2)],
        }],
    });
    let list = layout(&score, &opts(1600.0));
    check_invariants(&score, &list);
    let width = |mb: u32| {
        all_bars(&list)
            .find(|b| b.master_bar == mb)
            .unwrap()
            .frame
            .width
    };
    let busy = width(1);
    for rest_bar in [2, 5] {
        assert!(
            width(rest_bar) < busy * 0.4,
            "rest bar {rest_bar}: {} vs {busy}",
            width(rest_bar)
        );
    }
    // A full system holding a rest bar still fills the width.
    let score = with_rests(1);
    let list = layout(&score, &opts(700.0));
    assert!(list.systems.len() > 1);
    let s = &list.systems[0];
    assert!((right(&s.bars.last().unwrap().frame) - (700.0 - 16.0)).abs() < 0.5);
}

/// One bar holding an open chord then tying it over, then whole-bar ties.
fn tied_chords(bars: usize) -> Score {
    let shape = [(1, 0), (2, 1), (3, 0), (4, 2), (5, 3)];
    let tie = |n: &mut Note| n.tie_destination = true;
    let mut b = ScoreBuilder::new(6).bar(vec![
        beat(2, &shape),
        fx(beat(4, &shape), tie),
        fx(beat(4, &shape), tie),
    ]);
    for _ in 1..bars {
        b = b.bar(vec![fx(beat(1, &shape), tie)]);
    }
    b.build()
}

fn parens(s: &System) -> usize {
    s.primitives
        .iter()
        .filter(|p| matches!(p, Primitive::Text { text, .. } if text.starts_with('(')))
        .count()
}

#[test]
fn ties_are_arcs_except_at_system_start() {
    // One system: the chord is written once and every tie is an arc.
    let score = tied_chords(3);
    let list = layout(&score, &opts(1200.0));
    assert_eq!(list.systems.len(), 1);
    assert_eq!(parens(&list.systems[0]), 0, "no (n) within a system");
    assert_eq!(texts(&list).iter().filter(|t| t.2 == "3").count(), 1);
    assert_eq!(curves(&list), 5 * 4, "one arc per tied note");

    // Narrow: every bar starts a system, so its tied notes are written as "(n)".
    let list = layout(&score, &opts(150.0));
    assert_eq!(list.systems.len(), 3);
    assert_eq!(parens(&list.systems[0]), 0);
    for s in &list.systems[1..] {
        assert_eq!(parens(s), 5, "tied chord written out at system start");
    }
    // Ties running off the end of a system get an open arc.
    let end = right(&list.systems[0].bars[0].frame);
    let open = list.systems[0]
        .primitives
        .iter()
        .filter(|p| matches!(p, Primitive::Curve { x2, .. } if *x2 > end))
        .count();
    assert_eq!(open, 5);
}

#[test]
fn track_without_tab_gets_label() {
    // Like an untuned (vocal, synth) track: a zero-pitched placeholder string.
    let mut score = simple(6);
    score.tracks[0].tuning = vec![0];
    let list = layout(&score, &opts(900.0));
    check_invariants(&score, &list);
    let t = texts(&list);
    assert!(t.iter().any(|t| t.2 == "No tab for this track"));
    assert_eq!(below_staff(&list), ["6"], "one multi-rest");
    assert!(!t.iter().any(|t| t.2 == "12"), "no fret numbers");
    assert_eq!(list.systems.len(), 1);
    let staff_lines = list.systems[0]
        .primitives
        .iter()
        .filter(
            |p| matches!(p, Primitive::Line { y1, y2, x1, x2, .. } if y1 == y2 && x2 - x1 > 120.0),
        )
        .count();
    assert!(staff_lines >= 6, "an empty staff is drawn");

    // Notes off the staff (string 0, or past the last string) are skipped.
    let mut score = simple(2);
    let beats = &mut score.tracks[0].bars[0].voices[0].beats;
    beats[0].notes[0].string = 9;
    beats[1].notes[0].string = 0;
    let list = layout(&score, &opts(900.0));
    check_invariants(&score, &list);
    assert!(!texts(&list).iter().any(|t| t.2 == "No tab for this track"));
}

#[test]
fn labels_above_staff_do_not_collide() {
    let mut score = with_rests(4);
    let sec = |text: &str| {
        Some(Section {
            letter: String::new(),
            text: text.into(),
        })
    };
    score.master_bars[0].section = sec("Intro");
    score.tracks[0].bars[0].voices[0].beats[0].chord = Some("Am7".into());
    score.master_bars[2].section = sec("Post-Chorus");
    score.master_bars[6].section = sec("Outro");
    score.tempo_changes = vec![TempoChange {
        bar: 6,
        position: 0.0,
        bpm: 90.0,
    }];
    score.tracks[0].bars[6].voices[0].beats[0].chord = Some("Cmaj7".into());
    for width in [500.0, 1200.0] {
        let list = layout(&score, &opts(width));
        check_invariants(&score, &list);
        for s in &list.systems {
            let staff_top = s.bars[0].frame.y;
            let boxes: Vec<(f32, f32, f32, f32)> = s
                .primitives
                .iter()
                .filter_map(|p| match p {
                    Primitive::Text {
                        x,
                        y,
                        text,
                        size,
                        anchor,
                        ..
                    } if *y < staff_top => {
                        let w = text.chars().count() as f32 * CHAR_WIDTH * size;
                        let (x0, y0) = match anchor {
                            TextAnchor::Center => (x - w / 2.0, y - size / 2.0),
                            TextAnchor::Leading => (*x, y - size * 0.8),
                        };
                        Some((x0, y0, x0 + w, y0 + size))
                    }
                    _ => None,
                })
                .collect();
            for (a, p) in boxes.iter().enumerate() {
                for q in &boxes[a + 1..] {
                    let overlap =
                        p.0 < q.2 - 0.5 && q.0 < p.2 - 0.5 && p.1 < q.3 - 0.5 && q.1 < p.3 - 0.5;
                    assert!(!overlap, "labels overlap: {p:?} {q:?}");
                }
            }
        }
    }
}

#[test]
fn drums_placed_by_instrument() {
    let mut score = simple(1);
    score.tracks[0] = Track {
        name: "Drums".into(),
        is_percussion: true,
        tuning: vec![0; 6],
        bars: vec![Bar {
            voices: vec![Voice {
                beats: vec![
                    beat(4, &[(1, 42), (2, 36)]),
                    beat(4, &[(1, 42), (2, 38)]),
                    beat(4, &[(1, 49), (2, 36)]),
                    beat(4, &[(1, 51), (2, 43)]),
                ],
            }],
        }],
        ..Default::default()
    };
    let list = layout(&score, &opts(800.0));
    check_invariants(&score, &list);
    let prims = &list.systems[0].primitives;
    let xs: Vec<f32> = prims
        .iter()
        .filter_map(|p| match p {
            Primitive::Text { y, text, .. } if text == "x" => Some(*y),
            _ => None,
        })
        .collect();
    assert_eq!(xs.len(), 4, "hi-hats, crash and ride are x heads");
    let staff_top = list.systems[0].bars[0].frame.y + 13.0 / 2.0;
    assert!(xs[0] < staff_top, "hi-hat above the staff");
    assert!(xs[2] < xs[0], "crash above the hi-hat");
    assert!((xs[3] - staff_top).abs() < 0.01, "ride on the top line");
    let heads: Vec<f32> = prims
        .iter()
        .filter_map(|p| match p {
            Primitive::Dot { y, radius, .. } if *radius > 3.0 => Some(*y),
            _ => None,
        })
        .collect();
    assert_eq!(heads.len(), 4);
    assert!(heads[1] < heads[0], "snare above kick");
    assert!(
        heads[3] > heads[1] && heads[3] < heads[0],
        "floor tom between"
    );
}

#[test]
fn rest_shapes_distinct() {
    let score = ScoreBuilder::new(6)
        .bar(vec![rest(2), beat(4, &[(1, 0)]), rest(4)])
        .bar(vec![
            rest(8),
            beat(8, &[(1, 0)]),
            rest(16),
            beat(16, &[(1, 0)]),
            beat(8, &[(1, 0)]),
            beat(2, &[(1, 0)]),
        ])
        .bar(vec![rest(1)])
        .bar(vec![beat(1, &[(1, 0)])])
        .build();
    let list = layout(&score, &opts(1000.0));
    check_invariants(&score, &list);
    let blocks: Vec<Rect> = list.systems[0]
        .primitives
        .iter()
        .filter_map(|p| match p {
            Primitive::Rect { rect, .. } if rect.width > 8.0 && rect.height > 3.0 => Some(*rect),
            _ => None,
        })
        .collect();
    assert_eq!(blocks.len(), 2, "only whole and half rests are blocks");
    // The half rest sits on its line; the whole rest hangs from its line.
    let lines: Vec<f32> = list.systems[0]
        .primitives
        .iter()
        .filter_map(|p| match p {
            Primitive::Line { y1, y2, x1, x2, .. } if y1 == y2 && x2 - x1 < 20.0 => Some(*y1),
            _ => None,
        })
        .collect();
    let (half, whole) = (blocks[0], blocks[1]);
    assert!(
        lines
            .iter()
            .any(|&y| (y - (half.y + half.height)).abs() < 0.01)
    );
    assert!(lines.iter().any(|&y| (y - whole.y).abs() < 0.01));
}

#[test]
fn snapshot_rests_and_ties() {
    let mut score = tied_chords(2);
    for _ in 0..3 {
        score.master_bars.push(MasterBar::default());
        score.tracks[0].bars.push(Bar {
            voices: vec![Voice {
                beats: vec![rest(1)],
            }],
        });
    }
    let list = layout(&score, &opts(500.0));
    insta::assert_debug_snapshot!(rounded(&list));
}
