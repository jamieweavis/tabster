use std::path::Path;

use super::TabSynth;
use super::engine::{Engine, SAMPLE_RATE, Sink};
use super::events::{self, CLICK_ACCENT_KEY, CLICK_KEY, Event, PERCUSSION_CHANNEL};
use crate::model::{
    Bar, Beat, Duration, MasterBar, Note, NoteEffects, Score, Track, TripletFeel, Voice,
};
use crate::timeline::Timeline;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Msg {
    On(u8, u8, u8),
    Off(u8, u8),
    AllOff(bool),
    ChannelOff(u8),
}

/// Records messages with the frame they arrived at.
#[derive(Default)]
struct Recorder {
    frame: usize,
    log: Vec<(usize, Msg)>,
}

impl Recorder {
    fn ons(&self) -> Vec<(usize, u8, u8)> {
        self.log
            .iter()
            .filter_map(|&(f, m)| match m {
                Msg::On(ch, key, _) => Some((f, ch, key)),
                _ => None,
            })
            .collect()
    }
}

impl Sink for Recorder {
    fn note_on(&mut self, channel: u8, key: u8, velocity: u8) {
        self.log.push((self.frame, Msg::On(channel, key, velocity)));
    }
    fn note_off(&mut self, channel: u8, key: u8) {
        self.log.push((self.frame, Msg::Off(channel, key)));
    }
    fn all_notes_off(&mut self, immediate: bool) {
        self.log.push((self.frame, Msg::AllOff(immediate)));
    }
    fn channel_notes_off(&mut self, channel: u8) {
        self.log.push((self.frame, Msg::ChannelOff(channel)));
    }
    fn program_change(&mut self, _: u8, _: u8) {}
    fn set_volume(&mut self, _: f32) {}
    fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
        left.fill(0.0);
        right.fill(0.0);
        self.frame += left.len();
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

fn beat(value: u8, notes: Vec<Note>) -> Beat {
    Beat {
        duration: dur(value),
        notes,
        ..Default::default()
    }
}

fn guitar(bars: Vec<Vec<Beat>>) -> Track {
    Track {
        name: "Guitar".into(),
        program: 25,
        tuning: vec![64, 59, 55, 50, 45, 40],
        bars: bars
            .into_iter()
            .map(|beats| Bar {
                voices: vec![Voice { beats }],
            })
            .collect(),
        ..Default::default()
    }
}

/// 120 bpm, 4/4: a quarter note is 0.5 s, a bar 2 s.
fn score(tracks: Vec<Track>) -> Score {
    let bars = tracks[0].bars.len();
    Score {
        initial_tempo: 120.0,
        tracks,
        master_bars: vec![MasterBar::default(); bars],
        ..Default::default()
    }
}

fn build(score: &Score) -> events::Events {
    events::build(score, &Timeline::new(score))
}

fn ons(events: &[Event]) -> Vec<&Event> {
    events.iter().filter(|e| e.is_on()).collect()
}

fn off_for(events: &[Event], on: &Event) -> f64 {
    events
        .iter()
        .find(|e| !e.is_on() && e.key == on.key && e.channel == on.channel && e.secs >= on.secs)
        .expect("note-off")
        .secs
}

fn quarters(notes: [Note; 4]) -> Vec<Beat> {
    notes.into_iter().map(|n| beat(4, vec![n])).collect()
}

fn approx(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

#[test]
fn pitches_channels_and_lengths() {
    let s = score(vec![guitar(vec![quarters([
        note(1, 0),
        note(6, 3),
        note(2, 1),
        note(3, 2),
    ])])]);
    let ev = build(&s);
    let on = ons(&ev.notes);
    let keys: Vec<u8> = on.iter().map(|e| e.key).collect();
    assert_eq!(keys, [64, 43, 60, 57]);
    assert!(on.iter().all(|e| e.channel == 0 && e.velocity == 100));
    assert!(approx(on[1].secs, 0.5));
    assert!(approx(off_for(&ev.notes, on[1]), 1.0));
    assert_eq!(ev.programs, [(0, 25)]);
    assert!(approx(ev.duration_secs, 2.0));
}

#[test]
fn ties_extend_instead_of_retriggering() {
    let tied = Note {
        tie_destination: true,
        ..note(2, 3)
    };
    let s = score(vec![guitar(vec![
        quarters([note(1, 0), note(2, 3), tied.clone(), note(1, 0)]),
        // Tied across the bar line.
        vec![beat(
            1,
            vec![Note {
                tie_destination: true,
                ..note(1, 0)
            }],
        )],
    ])]);
    let ev = build(&s);
    let on = ons(&ev.notes);
    assert_eq!(on.len(), 3, "{on:?}");
    // D (string 2 fret 3 = 62) at 0.5 s held through the tied beat to 1.5 s.
    let b = on.iter().find(|e| approx(e.secs, 0.5)).unwrap();
    assert!(approx(off_for(&ev.notes, b), 1.5));
    // The last E is tied into the whole of bar 2.
    let last = on.iter().find(|e| approx(e.secs, 1.5)).unwrap();
    assert!(approx(off_for(&ev.notes, last), 4.0));
}

#[test]
fn palm_mute_dead_and_legato() {
    let pm = Note {
        effects: NoteEffects {
            palm_mute: true,
            ..Default::default()
        },
        ..note(6, 0)
    };
    let dead = Note {
        effects: NoteEffects {
            dead: true,
            ..Default::default()
        },
        ..note(5, 0)
    };
    let hammer = Note {
        effects: NoteEffects {
            hammer_pull: true,
            ..Default::default()
        },
        ..note(3, 2)
    };
    let s = score(vec![guitar(vec![quarters([pm, dead, hammer, note(3, 4)])])]);
    let ev = build(&s);
    let on = ons(&ev.notes);
    // Palm mute: 40% of a quarter, quieter.
    assert!(approx(off_for(&ev.notes, on[0]), 0.2));
    assert!(on[0].velocity < 100);
    // Dead: very short and quiet.
    assert!(off_for(&ev.notes, on[1]) - on[1].secs <= 0.05 + 1e-9);
    assert!(on[1].velocity <= 40);
    // The note hammered onto isn't re-picked at full strength.
    assert_eq!(on[2].velocity, 100);
    assert!(on[3].velocity < 100);
}

#[test]
fn let_ring_sustains_until_the_string_is_played_again() {
    let ring = |string, fret| Note {
        effects: NoteEffects {
            let_ring: true,
            ..Default::default()
        },
        ..note(string, fret)
    };
    let s = score(vec![guitar(vec![quarters([
        ring(3, 0),
        ring(2, 0),
        ring(3, 2),
        note(1, 0),
    ])])]);
    let ev = build(&s);
    let on = ons(&ev.notes);
    assert!(approx(off_for(&ev.notes, on[0]), 1.0)); // cut by string 3 again
    assert!(approx(off_for(&ev.notes, on[1]), 2.0)); // bar end
    assert!(approx(off_for(&ev.notes, on[2]), 2.0));
}

#[test]
fn percussion_uses_channel_10_and_the_drum_note() {
    let drums = Track {
        name: "Drums".into(),
        is_percussion: true,
        tuning: vec![0; 6],
        ..guitar(vec![quarters([
            note(1, 36),
            note(1, 38),
            note(1, 36),
            note(2, 42),
        ])])
    };
    // 10 melodic tracks: channel 9 is skipped.
    let mut tracks: Vec<Track> = (0..10)
        .map(|_| {
            guitar(vec![quarters([
                note(1, 0),
                note(1, 0),
                note(1, 0),
                note(1, 0),
            ])])
        })
        .collect();
    tracks.push(drums);
    let ev = build(&score(tracks));
    assert_eq!(&ev.track_channels[8..], [8, 10, PERCUSSION_CHANNEL]);
    let drum_keys: Vec<u8> = ons(&ev.notes)
        .iter()
        .filter(|e| e.track == 10)
        .map(|e| e.key)
        .collect();
    assert_eq!(drum_keys, [36, 38, 36, 42]);
    assert!(
        ons(&ev.notes)
            .iter()
            .filter(|e| e.track == 10)
            .all(|e| e.channel == PERCUSSION_CHANNEL)
    );
    assert!(!ev.programs.iter().any(|&(c, _)| c == PERCUSSION_CHANNEL));
}

#[test]
fn swing_moves_offbeat_eighths() {
    let eighths: Vec<Beat> = (0..8).map(|_| beat(8, vec![note(1, 0)])).collect();
    let mut s = score(vec![guitar(vec![eighths])]);
    s.master_bars[0].triplet_feel = TripletFeel::Triplet8th;
    let ev = build(&s);
    let starts: Vec<f64> = ons(&ev.notes).iter().map(|e| e.secs).collect();
    // Long–short: the off-beat lands 2/3 of the way through each 0.5 s beat.
    assert!(approx(starts[0], 0.0));
    assert!(approx(starts[1], 1.0 / 3.0), "{starts:?}");
    assert!(approx(starts[2], 0.5));
    assert!(approx(starts[3], 0.5 + 1.0 / 3.0));
    assert!(approx(off_for(&ev.notes, ons(&ev.notes)[0]), 1.0 / 3.0));
}

#[test]
fn events_follow_repeats() {
    let mut s = score(vec![guitar(vec![
        quarters([note(1, 0), note(1, 1), note(1, 2), note(1, 3)]),
        quarters([note(1, 5), note(1, 5), note(1, 5), note(1, 5)]),
    ])]);
    s.master_bars[0].repeat_start = true;
    s.master_bars[0].repeat_end = 2;
    let ev = build(&s);
    let on: Vec<(f64, u8)> = ons(&ev.notes).iter().map(|e| (e.secs, e.key)).collect();
    assert_eq!(on.len(), 12);
    assert_eq!(on[4], (2.0, 64));
    assert_eq!(on[8], (4.0, 69));
    assert!(approx(ev.duration_secs, 6.0));
}

#[test]
fn grace_notes_come_just_before_the_beat() {
    let grace = Beat {
        duration: dur(16),
        notes: vec![note(1, 2)],
        effects: crate::model::BeatEffects {
            grace: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let beats = vec![
        beat(4, vec![note(2, 0)]),
        grace,
        beat(4, vec![note(1, 3)]),
        beat(2, vec![]),
    ];
    let ev = build(&score(vec![guitar(vec![beats])]));
    let on = ons(&ev.notes);
    assert_eq!(on.len(), 3);
    assert!(on[1].secs < 0.5 && on[1].secs > 0.4);
    assert!(approx(off_for(&ev.notes, on[1]), 0.5));
    assert!(approx(on[2].secs, 0.5));
}

fn engine(score: &Score) -> Engine<Recorder> {
    Engine::new(Recorder::default(), build(score))
}

fn render_secs(e: &mut Engine<Recorder>, secs: f64) {
    let frames = (secs * f64::from(SAMPLE_RATE)).round() as usize;
    let mut buf = vec![0.0; 2 * 256];
    let mut left = frames;
    while left > 0 {
        let n = left.min(256);
        e.render_into(&mut buf[..n * 2]);
        left -= n;
    }
}

fn four_quarters() -> Score {
    score(vec![guitar(vec![quarters([
        note(1, 0),
        note(1, 2),
        note(1, 4),
        note(1, 5),
    ])])])
}

#[test]
fn render_fires_events_on_their_frame() {
    let mut e = engine(&four_quarters());
    e.set_playing(true);
    render_secs(&mut e, 1.2);
    let frames: Vec<usize> = e.sink().ons().iter().map(|o| o.0).collect();
    assert_eq!(frames, [0, 22050, 44100]);
    assert!(approx(e.position_secs(), 1.2));
}

#[test]
fn speed_scales_the_clock() {
    let mut e = engine(&four_quarters());
    e.set_speed(0.5);
    e.set_playing(true);
    render_secs(&mut e, 1.01);
    assert!(approx(e.position_secs(), 0.505));
    let frames: Vec<usize> = e.sink().ons().iter().map(|o| o.0).collect();
    assert_eq!(frames, [0, 44100]);
}

#[test]
fn paused_renders_without_advancing() {
    let mut e = engine(&four_quarters());
    render_secs(&mut e, 1.0);
    assert_eq!(e.position_secs(), 0.0);
    assert!(e.sink().ons().is_empty());
}

#[test]
fn loop_wraps_inside_render() {
    let mut e = engine(&four_quarters());
    e.set_loop(Some(0.5), Some(1.0));
    e.seek(0.5);
    e.set_playing(true);
    render_secs(&mut e, 1.25);
    let ons = e.sink().ons();
    // 0.5 s → note 2; wraps at 1.0 s (frame 22050) and 2 × 22050.
    let frames: Vec<usize> = ons.iter().map(|o| o.0).collect();
    assert_eq!(frames, [0, 22050, 44100]);
    assert!(ons.iter().all(|o| o.2 == 66));
    assert!(
        e.sink()
            .log
            .iter()
            .any(|&(f, m)| f == 22050 && m == Msg::AllOff(false))
    );
    assert!(approx(e.position_secs(), 0.75));
}

#[test]
fn seek_cuts_voices_and_resumes_from_there() {
    let mut e = engine(&four_quarters());
    e.set_playing(true);
    render_secs(&mut e, 0.1);
    e.seek(1.0);
    let n = e.sink().log.len();
    assert_eq!(e.sink().log[n - 1].1, Msg::AllOff(true));
    render_secs(&mut e, 0.1);
    let keys: Vec<u8> = e.sink().ons().iter().map(|o| o.2).collect();
    assert_eq!(keys, [64, 68]);
}

#[test]
fn mute_and_solo() {
    let mut s = four_quarters();
    s.tracks.push(guitar(vec![quarters([
        note(6, 0),
        note(6, 0),
        note(6, 0),
        note(6, 0),
    ])]));
    let mut e = engine(&s);
    e.set_track_mute(0, true);
    e.set_playing(true);
    render_secs(&mut e, 0.1);
    assert_eq!(e.sink().ons().iter().map(|o| o.2).collect::<Vec<_>>(), [40]);
    e.set_track_mute(0, false);
    e.set_track_solo(0, true);
    assert!(e.sink().log.contains(&(4410, Msg::ChannelOff(1))));
    render_secs(&mut e, 0.5);
    assert_eq!(
        e.sink().ons().iter().map(|o| o.2).collect::<Vec<_>>(),
        [40, 66]
    );
}

#[test]
fn metronome_and_count_in() {
    let mut e = engine(&four_quarters());
    e.set_metronome(true);
    e.count_in(1);
    e.set_playing(true);
    render_secs(&mut e, 2.5);
    let clicks: Vec<(usize, u8)> = e
        .sink()
        .ons()
        .iter()
        .filter(|o| o.1 == PERCUSSION_CHANNEL)
        .map(|o| (o.0, o.2))
        .collect();
    let sec = SAMPLE_RATE as usize;
    // Count-in: 4 clicks over 2 s, then the bar's own clicks from 2 s.
    assert_eq!(
        clicks,
        [
            (0, CLICK_ACCENT_KEY),
            (sec / 2, CLICK_KEY),
            (sec, CLICK_KEY),
            (sec * 3 / 2, CLICK_KEY),
            (sec * 2, CLICK_ACCENT_KEY),
        ]
    );
    // The score clock was held for the count-in.
    assert!(approx(e.position_secs(), 0.5));
    let first_note = e.sink().ons().iter().find(|o| o.1 == 0).unwrap().0;
    assert_eq!(first_note, sec * 2);
}

#[test]
fn stops_at_the_end() {
    let mut e = engine(&four_quarters());
    e.set_playing(true);
    render_secs(&mut e, 3.0);
    assert!(approx(e.position_secs(), 2.0));
    let offs = e
        .sink()
        .log
        .iter()
        .filter(|(_, m)| matches!(m, Msg::Off(..)))
        .count();
    assert_eq!(offs, 4);
}

#[test]
fn missing_soundfont_is_reported() {
    let s = four_quarters();
    let err = TabSynth::new(&s, &Timeline::new(&s), Path::new("/nonexistent/x.sf2"))
        .err()
        .unwrap();
    assert!(matches!(err, super::SynthError::NotFound(_)));
}

fn bundled_soundfont() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("Resources/Bundle/SoundFont.sf2")
}

/// Needs `just soundfont`.
#[test]
#[ignore]
fn renders_audio_with_the_bundled_soundfont() {
    let s = four_quarters();
    let synth = TabSynth::new(&s, &Timeline::new(&s), &bundled_soundfont()).unwrap();
    synth.set_playing(true);
    let audio = synth.render(SAMPLE_RATE);
    let peak = audio.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    let rms = (audio.iter().map(|v| v * v).sum::<f32>() / audio.len() as f32).sqrt();
    assert!(peak > 0.01 && peak < 1.0, "peak {peak}");
    assert!(rms > 0.001, "rms {rms}");
    assert!(approx(synth.position_secs(), 1.0));
}
