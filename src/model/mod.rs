//! Format-agnostic score model. Every importer in `formats` produces a [`Score`]; everything
//! downstream (timeline, sync, layout, library, playback) only ever reads this model.
//!
//! Conventions (frozen contract — changes go through the lead):
//! - Durations are measured in ticks, [`TICKS_PER_QUARTER`] per quarter note.
//! - Strings are numbered from 1 = the highest-pitched string (as in tab, and Guitar Pro 3–5).
//!   `Track::tuning[0]` is string 1.
//! - Pitches are MIDI note numbers (60 = middle C). Fretted pitch = `tuning[string-1] + capo + fret`.
//! - Every `Track::bars` has exactly `Score::master_bars.len()` entries, index-aligned.
//! - Tempo is always normalised to quarter notes per minute.

use serde::{Deserialize, Serialize};

pub const TICKS_PER_QUARTER: u32 = 960;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Score {
    pub title: String,
    pub subtitle: String,
    pub artist: String,
    pub album: String,
    pub words: String,
    pub music: String,
    pub tab_author: String,
    /// Tempo at the start of bar 0, quarter notes per minute.
    pub initial_tempo: f64,
    pub tracks: Vec<Track>,
    pub master_bars: Vec<MasterBar>,
    /// Sorted by (bar, position). The first entry, if at bar 0 position 0, overrides `initial_tempo`.
    pub tempo_changes: Vec<TempoChange>,
    /// Video/audio sync points stored in the file (GP8 `SyncPoint` automations). Usually empty.
    pub sync_points: Vec<FileSyncPoint>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub name: String,
    pub short_name: String,
    /// General MIDI program (0–127). Ignored when `is_percussion`.
    pub program: u8,
    pub is_percussion: bool,
    /// MIDI pitch of each open string, index 0 = string 1 (highest pitch).
    pub tuning: Vec<u8>,
    pub capo: u8,
    /// One per master bar, index-aligned with `Score::master_bars`.
    pub bars: Vec<Bar>,
}

impl Track {
    pub fn string_count(&self) -> usize {
        self.tuning.len()
    }

    /// MIDI pitch of a fretted note, or `None` if the string doesn't exist.
    pub fn pitch(&self, string: u8, fret: u8) -> Option<u8> {
        let open = *self.tuning.get(usize::from(string).checked_sub(1)?)?;
        Some(open.saturating_add(self.capo).saturating_add(fret))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeSignature {
    pub numerator: u8,
    /// 1, 2, 4, 8, 16, 32.
    pub denominator: u8,
}

impl Default for TimeSignature {
    fn default() -> Self {
        Self {
            numerator: 4,
            denominator: 4,
        }
    }
}

impl TimeSignature {
    /// The nominal length of a bar in this time signature.
    pub fn bar_ticks(&self) -> u32 {
        u32::from(self.numerator) * TICKS_PER_QUARTER * 4 / u32::from(self.denominator.max(1))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MasterBar {
    pub time_signature: TimeSignature,
    /// Key signature: number of sharps (positive) or flats (negative).
    pub key_signature: i8,
    pub repeat_start: bool,
    /// Number of times the repeated section is played in total, set on the closing bar.
    /// 0 = this bar doesn't close a repeat. (`|: … :|` played twice = 2.)
    pub repeat_end: u8,
    /// Bitmask of alternate ending numbers this bar belongs to: bit 0 = 1st ending, bit 1 = 2nd …
    /// 0 = not part of an alternate ending.
    pub alternate_endings: u8,
    pub section: Option<Section>,
    /// Navigation markers and jumps (segno, coda, D.S. al coda …) attached to this bar.
    pub directions: Vec<Direction>,
    pub triplet_feel: TripletFeel,
    pub double_bar: bool,
    /// Pickup bar: shorter than its time signature. The actual length comes from its beats.
    pub anacrusis: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Section {
    /// Short label, e.g. "A". May be empty.
    pub letter: String,
    /// e.g. "Verse", "Chorus".
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    // Targets (where a jump lands / marks a location).
    Coda,
    DoubleCoda,
    Segno,
    SegnoSegno,
    Fine,
    // Jumps (performed at the end of the bar they're attached to).
    DaCapo,
    DaCapoAlCoda,
    DaCapoAlDoubleCoda,
    DaCapoAlFine,
    DalSegno,
    DalSegnoAlCoda,
    DalSegnoAlDoubleCoda,
    DalSegnoAlFine,
    DalSegnoSegno,
    DalSegnoSegnoAlCoda,
    DalSegnoSegnoAlDoubleCoda,
    DalSegnoSegnoAlFine,
    DaCoda,
    DaDoubleCoda,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TripletFeel {
    #[default]
    None,
    Triplet8th,
    Triplet16th,
    Dotted8th,
    Dotted16th,
    Scottish8th,
    Scottish16th,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TempoChange {
    /// Master bar index.
    pub bar: u32,
    /// Position within the bar as a fraction 0.0..1.0 of the bar's length.
    pub position: f64,
    /// Quarter notes per minute.
    pub bpm: f64,
}

/// A sync point stored in the score file (GP8 / alphaTab model).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FileSyncPoint {
    pub bar: u32,
    /// Which pass through this bar (0 = first time), for bars inside repeats.
    pub bar_occurrence: u32,
    /// Fraction 0.0..1.0 within the bar.
    pub bar_position: f64,
    /// Media time in milliseconds.
    pub millisecond_offset: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Bar {
    /// Up to 4 voices. Voice 0 is primary; empty voices may be omitted.
    pub voices: Vec<Voice>,
}

impl Bar {
    /// Actual length in ticks, from the longest voice. 0 if the bar has no beats.
    pub fn ticks(&self) -> u32 {
        self.voices.iter().map(Voice::ticks).max().unwrap_or(0)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Voice {
    pub beats: Vec<Beat>,
}

impl Voice {
    /// Length in ticks. Grace beats are played in the time of the beat around them, so they
    /// don't count.
    pub fn ticks(&self) -> u32 {
        self.beats
            .iter()
            .filter(|b| !b.effects.grace)
            .map(|b| b.duration.ticks())
            .sum()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Beat {
    pub duration: Duration,
    /// Empty = rest.
    pub notes: Vec<Note>,
    pub text: Option<String>,
    pub chord: Option<String>,
    pub effects: BeatEffects,
}

impl Beat {
    pub fn is_rest(&self) -> bool {
        self.notes.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Duration {
    /// Note value as a denominator: 1 = whole, 2 = half, 4 = quarter … 128.
    pub value: u8,
    /// 0, 1 or 2.
    pub dots: u8,
    /// Tuplet as (n, m): n notes in the time of m, e.g. (3, 2) for a triplet. `None` = no tuplet.
    pub tuplet: Option<(u8, u8)>,
}

impl Default for Duration {
    fn default() -> Self {
        Self {
            value: 4,
            dots: 0,
            tuplet: None,
        }
    }
}

impl Duration {
    pub fn ticks(&self) -> u32 {
        let base = TICKS_PER_QUARTER * 4 / u32::from(self.value.max(1));
        let dotted = match self.dots {
            0 => base,
            1 => base * 3 / 2,
            _ => base * 7 / 4,
        };
        match self.tuplet {
            Some((n, m)) if n > 0 => dotted * u32::from(m) / u32::from(n),
            _ => dotted,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BeatEffects {
    pub stroke: Option<Stroke>,
    pub tapping: bool,
    pub slap: bool,
    pub pop: bool,
    pub fade_in: bool,
    /// Tremolo picking speed as a note value (8, 16 or 32).
    pub tremolo_picking: Option<u8>,
    pub whammy: Option<Bend>,
    /// Grace beat played before the main beat.
    pub grace: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Stroke {
    Up,
    Down,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub string: u8,
    pub fret: u8,
    /// MIDI velocity 1–127.
    pub velocity: u8,
    /// This note continues (is tied from) the previous note on the same string.
    pub tie_destination: bool,
    pub effects: NoteEffects,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NoteEffects {
    pub dead: bool,
    pub ghost: bool,
    pub accent: bool,
    pub heavy_accent: bool,
    pub palm_mute: bool,
    pub let_ring: bool,
    pub staccato: bool,
    pub vibrato: bool,
    /// Hammer-on/pull-off to the next note on this string.
    pub hammer_pull: bool,
    pub slide: Option<Slide>,
    pub bend: Option<Bend>,
    pub harmonic: Option<Harmonic>,
    /// Trill with the given fret.
    pub trill: Option<u8>,
    pub tapped: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Slide {
    /// Legato slide to the next note.
    Legato,
    /// Shift slide to the next note (re-picked).
    Shift,
    InFromBelow,
    InFromAbove,
    OutDown,
    OutUp,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Bend {
    /// (position 0.0..1.0 within the note, value in semitones). At least 2 points.
    pub points: Vec<(f32, f32)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Harmonic {
    Natural,
    Artificial,
    Pinch,
    Tapped,
    Semi,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        let q = Duration::default();
        assert_eq!(q.ticks(), 960);
        assert_eq!(
            Duration {
                value: 8,
                dots: 1,
                tuplet: None
            }
            .ticks(),
            720
        );
        assert_eq!(
            Duration {
                value: 8,
                dots: 0,
                tuplet: Some((3, 2))
            }
            .ticks(),
            320
        );
        assert_eq!(
            TimeSignature {
                numerator: 6,
                denominator: 8
            }
            .bar_ticks(),
            2880
        );
    }

    #[test]
    fn pitch() {
        let t = Track {
            tuning: vec![64, 59, 55, 50, 45, 40],
            capo: 2,
            ..Default::default()
        };
        assert_eq!(t.pitch(6, 0), Some(42));
        assert_eq!(t.pitch(1, 3), Some(69));
        assert_eq!(t.pitch(7, 0), None);
        assert_eq!(t.pitch(0, 0), None);
    }
}
