//! Score + timeline → a flat, time-sorted list of MIDI note events in score seconds.
//!
//! Events follow the *played* timeline (repeats and jumps unrolled), so the engine only ever
//! walks forward through one sorted list.

use crate::model::{Beat, Harmonic, Note, Score, Slide, TICKS_PER_QUARTER, Track, TripletFeel};
use crate::timeline::{PlayedBar, Timeline};

/// MIDI channel 10 (0-based 9): General MIDI percussion.
pub const PERCUSSION_CHANNEL: u8 = 9;
/// `Event::track` of metronome clicks.
pub const METRONOME_TRACK: u16 = u16::MAX;
/// GM Hi Wood Block (accented click) and Low Wood Block.
pub const CLICK_ACCENT_KEY: u8 = 76;
pub const CLICK_KEY: u8 = 77;
const CLICK_SECS: f64 = 0.05;

/// How long a grace note sounds before its main beat.
const GRACE_SECS: f64 = 0.06;
/// Dead notes: a short, quiet thud.
const DEAD_SECS: f64 = 0.05;
const DEAD_VELOCITY: u8 = 40;
/// Shortest note we emit (avoids zero-length on/off pairs).
const MIN_NOTE_SECS: f64 = 0.01;
/// Velocity when the file doesn't give one.
const DEFAULT_VELOCITY: u8 = 95;

/// A note-on (`velocity > 0`) or note-off (`velocity == 0`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Event {
    pub secs: f64,
    /// Index into `Score::tracks`, or [`METRONOME_TRACK`].
    pub track: u16,
    pub channel: u8,
    pub key: u8,
    pub velocity: u8,
}

impl Event {
    pub fn is_on(&self) -> bool {
        self.velocity > 0
    }
}

/// Per played bar: what a count-in starting in it sounds like.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BarBeats {
    pub start_secs: f64,
    /// Length of one metronome beat at the start of the bar.
    pub beat_secs: f64,
    pub beats_per_bar: u8,
}

#[derive(Debug, Clone, Default)]
pub struct Events {
    /// Note events of every track, sorted by time (note-offs before note-ons at equal times).
    pub notes: Vec<Event>,
    /// Metronome clicks for every played beat, sorted.
    pub clicks: Vec<Event>,
    pub bars: Vec<BarBeats>,
    /// `(channel, program)` to set up before playing.
    pub programs: Vec<(u8, u8)>,
    /// MIDI channel of each track.
    pub track_channels: Vec<u8>,
    pub duration_secs: f64,
}

/// A sounding note before it's split into on/off events.
#[derive(Debug, Clone, Copy)]
struct Span {
    start: f64,
    end: f64,
    track: u16,
    channel: u8,
    key: u8,
    velocity: u8,
}

/// What the last note on a string left behind, for ties, let ring and legato.
#[derive(Debug, Clone, Copy, Default)]
struct StringState {
    /// Index into the spans of the note sounding (or last sounded) on this string.
    open: Option<usize>,
    /// The next note on this string is hammered/pulled/slid into (no re-pick).
    legato_next: bool,
}

/// Channels for melodic tracks: every channel except percussion, reused round-robin.
fn assign_channels(score: &Score) -> (Vec<u8>, Vec<(u8, u8)>) {
    const MELODIC: [u8; 15] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 12, 13, 14, 15];
    let mut next = 0;
    let mut programs: Vec<(u8, u8)> = Vec::new();
    let channels = score
        .tracks
        .iter()
        .map(|t| {
            if t.is_percussion {
                return PERCUSSION_CHANNEL;
            }
            let ch = MELODIC[next % MELODIC.len()];
            next += 1;
            if !programs.iter().any(|&(c, _)| c == ch) {
                programs.push((ch, t.program.min(127)));
            }
            ch
        })
        .collect();
    (channels, programs)
}

/// Swing as (unit ticks, where the unit's midpoint moves to), per `MasterBar::triplet_feel`.
fn swing(feel: TripletFeel) -> Option<(u32, u32)> {
    let q = TICKS_PER_QUARTER;
    match feel {
        TripletFeel::None => None,
        TripletFeel::Triplet8th => Some((q, q * 2 / 3)),
        TripletFeel::Triplet16th => Some((q / 2, q / 3)),
        TripletFeel::Dotted8th => Some((q, q * 3 / 4)),
        TripletFeel::Dotted16th => Some((q / 2, q * 3 / 8)),
        TripletFeel::Scottish8th => Some((q, q / 4)),
        TripletFeel::Scottish16th => Some((q / 2, q / 8)),
    }
}

/// Move a tick position within a bar according to the swing: piecewise linear within each
/// unit, so the unit's halves become long–short (or short–long for Scottish).
fn warp(tick: u32, (unit, to): (u32, u32)) -> u32 {
    let half = unit / 2;
    let base = tick / unit * unit;
    let r = tick % unit;
    base + if r <= half {
        r * to / half
    } else {
        to + (r - half) * (unit - to) / half
    }
}

fn scale(velocity: u8, factor: f32) -> u8 {
    (f32::from(velocity) * factor).round().clamp(1.0, 127.0) as u8
}

/// Build the event list for a score.
pub fn build(score: &Score, timeline: &Timeline) -> Events {
    let (track_channels, programs) = assign_channels(score);
    let mut spans: Vec<Span> = Vec::new();
    for (ti, track) in score.tracks.iter().enumerate() {
        let mut generator = TrackGen {
            track,
            track_index: ti.min(usize::from(u16::MAX - 1)) as u16,
            channel: track_channels[ti],
            spans: &mut spans,
            strings: Vec::new(),
        };
        for pb in timeline.played_bars() {
            generator.bar(score, timeline, pb);
        }
    }
    let notes = to_events(spans);

    let mut clicks = Vec::new();
    let mut bars = Vec::with_capacity(timeline.played_bars().len());
    for pb in timeline.played_bars() {
        let ts = score
            .master_bars
            .get(pb.master_bar as usize)
            .map(|m| m.time_signature)
            .unwrap_or_default();
        let numerator = ts.numerator.max(1);
        let beat_ticks = u64::from((ts.bar_ticks() / u32::from(numerator)).max(1));
        let len = pb.end_tick - pb.start_tick;
        bars.push(BarBeats {
            start_secs: pb.start_secs,
            beat_secs: timeline.tick_to_secs(pb.start_tick + beat_ticks)
                - timeline.tick_to_secs(pb.start_tick),
            beats_per_bar: numerator,
        });
        let mut k = 0;
        while k * beat_ticks < len {
            let secs = timeline.tick_to_secs(pb.start_tick + k * beat_ticks);
            let (key, velocity) = if k == 0 {
                (CLICK_ACCENT_KEY, 115)
            } else {
                (CLICK_KEY, 85)
            };
            let click = Event {
                secs,
                track: METRONOME_TRACK,
                channel: PERCUSSION_CHANNEL,
                key,
                velocity,
            };
            clicks.push(click);
            clicks.push(Event {
                secs: secs + CLICK_SECS,
                velocity: 0,
                ..click
            });
            k += 1;
        }
    }
    sort_events(&mut clicks);

    Events {
        notes,
        clicks,
        bars,
        programs,
        track_channels,
        duration_secs: timeline.duration_secs(),
    }
}

fn sort_events(events: &mut [Event]) {
    events.sort_by(|a, b| a.secs.total_cmp(&b.secs).then(a.is_on().cmp(&b.is_on())));
}

/// Resolve overlaps of the same key on the same channel (the later note cuts the earlier one,
/// so a note-off can never silence a re-attack), then split into sorted on/off events.
fn to_events(mut spans: Vec<Span>) -> Vec<Event> {
    spans.sort_by(|a, b| {
        (a.channel, a.key)
            .cmp(&(b.channel, b.key))
            .then(a.start.total_cmp(&b.start))
    });
    let mut kept: Vec<Span> = Vec::with_capacity(spans.len());
    for s in spans {
        if let Some(prev) = kept.last_mut()
            && (prev.channel, prev.key) == (s.channel, s.key)
            && prev.end > s.start
        {
            if s.start - prev.start < 1e-6 {
                // Same note struck twice at once (two voices): keep the louder, longer one.
                prev.velocity = prev.velocity.max(s.velocity);
                prev.end = prev.end.max(s.end);
                continue;
            }
            prev.end = s.start;
        }
        kept.push(s);
    }
    let mut events = Vec::with_capacity(kept.len() * 2);
    for s in kept {
        let on = Event {
            secs: s.start,
            track: s.track,
            channel: s.channel,
            key: s.key,
            velocity: s.velocity.max(1),
        };
        events.push(on);
        events.push(Event {
            secs: s.end.max(s.start + MIN_NOTE_SECS / 2.0),
            velocity: 0,
            ..on
        });
    }
    sort_events(&mut events);
    events
}

struct TrackGen<'a> {
    track: &'a Track,
    track_index: u16,
    channel: u8,
    spans: &'a mut Vec<Span>,
    /// `[voice][string]`; persists across bars so ties and let ring carry over.
    strings: Vec<Vec<StringState>>,
}

impl TrackGen<'_> {
    fn bar(&mut self, score: &Score, timeline: &Timeline, pb: &PlayedBar) {
        let Some(bar) = self.track.bars.get(pb.master_bar as usize) else {
            return;
        };
        let feel = score
            .master_bars
            .get(pb.master_bar as usize)
            .and_then(|m| swing(m.triplet_feel));
        let len = pb.end_tick - pb.start_tick;
        let secs_at = |tick: u32| timeline.tick_to_secs(pb.start_tick + u64::from(tick).min(len));
        for (vi, voice) in bar.voices.iter().enumerate() {
            if self.strings.len() <= vi {
                self.strings.resize(vi + 1, Vec::new());
            }
            let mut offset = 0u32;
            for (bi, beat) in voice.beats.iter().enumerate() {
                if beat.effects.grace {
                    // Graces lead into the next real beat: k-th of n graces starts n−k slots early.
                    let after = voice.beats[bi + 1..]
                        .iter()
                        .take_while(|b| b.effects.grace)
                        .count();
                    let main = secs_at(offset);
                    let start = (main - GRACE_SECS * (after + 1) as f64).max(0.0);
                    self.beat(vi, beat, start, start + GRACE_SECS, pb.end_secs);
                    continue;
                }
                let ticks = beat.duration.ticks();
                let (s, e) = match feel {
                    Some(f) if beat.duration.tuplet.is_none() => {
                        (warp(offset, f), warp(offset + ticks, f))
                    }
                    _ => (offset, offset + ticks),
                };
                offset += ticks;
                if beat.is_rest() {
                    continue;
                }
                self.beat(vi, beat, secs_at(s), secs_at(e), pb.end_secs);
            }
        }
    }

    fn beat(&mut self, voice: usize, beat: &Beat, start: f64, end: f64, bar_end: f64) {
        for note in &beat.notes {
            self.note(voice, beat, note, start, end, bar_end);
        }
    }

    fn note(&mut self, voice: usize, beat: &Beat, note: &Note, start: f64, end: f64, bar_end: f64) {
        let fx = &note.effects;
        let percussion = self.track.is_percussion;
        let key = if percussion {
            Some(note.fret.min(127))
        } else {
            self.track.pitch(note.string, note.fret).map(|p| {
                p.saturating_add(harmonic_offset(fx.harmonic, note.fret))
                    .min(127)
            })
        };
        let Some(key) = key else { return };

        let string = usize::from(note.string);
        let strings = &mut self.strings[voice];
        if strings.len() <= string {
            strings.resize(string + 1, StringState::default());
        }
        let state = &mut strings[string];

        if note.tie_destination {
            if percussion {
                return;
            }
            if let Some(i) = state.open {
                let span = &mut self.spans[i];
                let target = if fx.let_ring { bar_end } else { end };
                span.end = span.end.max(target);
                state.legato_next = fx.hammer_pull || fx.slide == Some(Slide::Legato);
                return;
            }
        }
        // A new note on a string stops whatever still rings on it.
        if let Some(i) = state.open {
            let span = &mut self.spans[i];
            if span.end > start {
                span.end = start.max(span.start + MIN_NOTE_SECS);
            }
        }

        let base = if note.velocity == 0 {
            DEFAULT_VELOCITY
        } else {
            note.velocity
        };
        let mut factor = 1.0;
        if fx.ghost {
            factor *= 0.6;
        }
        if fx.accent {
            factor *= 1.15;
        }
        if fx.heavy_accent {
            factor *= 1.3;
        }
        if fx.palm_mute {
            factor *= 0.8;
        }
        if state.legato_next {
            factor *= 0.7;
        }
        let velocity = if fx.dead {
            DEAD_VELOCITY.min(base)
        } else {
            scale(base, factor)
        };

        let length = (end - start).max(MIN_NOTE_SECS);
        let end = if fx.dead {
            start + DEAD_SECS.min(length)
        } else if fx.palm_mute {
            start + length * 0.4
        } else if fx.staccato {
            start + length * 0.5
        } else if fx.let_ring {
            bar_end.max(end)
        } else {
            start + length
        };

        let span = Span {
            start,
            end,
            track: self.track_index,
            channel: self.channel,
            key,
            velocity,
        };
        match beat.effects.tremolo_picking {
            Some(value) if value > 0 && !fx.dead => {
                // Re-pick every `value`th note for the beat's whole length.
                let step_ticks = TICKS_PER_QUARTER * 4 / u32::from(value);
                let beat_ticks = beat.duration.ticks().max(1);
                let count = (beat_ticks / step_ticks.max(1)).max(1);
                let step = length / f64::from(count);
                for k in 0..count {
                    let s = start + step * f64::from(k);
                    self.spans.push(Span {
                        start: s,
                        end: s + step,
                        ..span
                    });
                }
            }
            _ => self.spans.push(span),
        }
        state.open = Some(self.spans.len() - 1);
        state.legato_next = fx.hammer_pull || fx.slide == Some(Slide::Legato);
    }
}

/// Natural harmonics at frets 7 and 5 sound an octave / octave+fifth above the fretted pitch
/// (fret 12's harmonic is the fretted pitch); other harmonics are approximated as fretted.
fn harmonic_offset(harmonic: Option<Harmonic>, fret: u8) -> u8 {
    match (harmonic, fret) {
        (Some(Harmonic::Natural), 7) => 12,
        (Some(Harmonic::Natural), 5) => 19,
        _ => 0,
    }
}
