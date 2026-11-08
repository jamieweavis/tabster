//! GPIF XML (`score.gpif`) → [`Score`]. Shared by `.gp` (GP7/8) and `.gpx` (GP6).
//! Owner: Phase 1 agent A.
//!
//! The mapping follows alphaTab's `GpifParser` (MPL-2.0, <https://github.com/CoderLine/alphaTab>),
//! which is the de-facto reference for the format. GPIF stores every object once in a flat,
//! id-keyed table (`MasterBars`, `Bars`, `Voices`, `Beats`, `Notes`, `Rhythms`) and links them by
//! space-separated id lists. We read the document into a small arena DOM in one streaming
//! quick-xml pass ([`dom`]), collect the tables into id-keyed maps borrowing from it, then
//! resolve the references into the model.
//!
//! Robustness rules: unknown elements are ignored, dangling references skip the item that
//! references them (with a `debug!` log), and malformed numbers fall back to defaults. Only
//! XML that isn't well-formed, or a root that isn't `<GPIF>`, is an error.
//!
//! Format notes (where GPIF differs from the model):
//! - GPIF string indices are 0-based from the *lowest* string and `Pitches` lists the lowest
//!   string first; the model numbers strings from 1 = highest.
//! - Tempo automations carry a beat-unit reference (`"120 2"`): 1 = eighth, 2 = quarter,
//!   3 = dotted quarter, 4 = half, 5 = dotted half. We normalise to quarter notes per minute.
//! - Bend values are in 1/100 of a whole tone (100 = full bend = 2 semitones); offsets are
//!   0–100 of the note length.
//! - GP8 `SyncPoint` frame offsets are 44.1 kHz frames (Guitar Pro always uses 44100 regardless
//!   of the audio file), minus the backing track's `FramePadding`.
//! - Percussion notes have no string/fret. Like GP3–5 files, the model stores them with
//!   `fret` = General MIDI drum note and strings assigned 1, 2, … in beat order on a 6-string
//!   all-zero tuning, so `Track::pitch` yields the drum note.
//! - Tracks without a tuning (piano, synths …) get a single zero-pitched string, and each note's
//!   `fret` is its MIDI pitch.

mod dom;

use std::collections::HashMap;

use log::debug;

use super::ParseError;
use crate::model::{
    Bar, Beat, Bend, Direction, Duration, FileSyncPoint, Harmonic, MasterBar, Note, Score, Section,
    Slide, Stroke, TempoChange, TimeSignature, Track, TripletFeel, Voice,
};
use dom::{Doc, El};

/// Guitar Pro measures sync point frame offsets at this rate, whatever the audio file's rate.
const SYNC_SAMPLE_RATE: f64 = 44100.0;
const DEFAULT_TEMPO: f64 = 120.0;
/// MIDI velocity for `F`, the default dynamic of a beat without a `<Dynamic>`.
const DEFAULT_VELOCITY: u8 = 95;
const INVALID_ID: &str = "-1";

/// Parse the contents of a `score.gpif` document.
pub fn parse_gpif(xml: &[u8]) -> Result<Score, ParseError> {
    let xml = xml.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(xml);
    let text = String::from_utf8_lossy(xml);
    let doc = Doc::parse(&text).map_err(ParseError::Invalid)?;
    let root = doc.root();
    if root.name() != "GPIF" {
        return Err(ParseError::Invalid(format!(
            "root element is <{}>, expected <GPIF>",
            root.name()
        )));
    }
    Ok(Tables::read(root).build())
}

// ---------------------------------------------------------------------------------------------
// Pass 1: read the document into id-keyed tables.
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct Tables<'d> {
    score: Score,
    /// `MasterTrack/Tracks`: track ids in score order.
    track_order: Vec<&'d str>,
    anacrusis: bool,
    automations: Vec<Automation>,
    /// Backing-track padding to subtract from sync point offsets, in milliseconds.
    sync_padding_ms: f64,
    /// Tracks in document order.
    tracks: Vec<(&'d str, RawTrack<'d>)>,
    master_bars: Vec<(MasterBar, Vec<&'d str>)>,
    /// Bar id → voice ids.
    bars: HashMap<&'d str, Vec<&'d str>>,
    /// Voice id → beat ids.
    voices: HashMap<&'d str, Vec<&'d str>>,
    beats: HashMap<&'d str, RawBeat<'d>>,
    notes: HashMap<&'d str, RawNote>,
    rhythms: HashMap<&'d str, Duration>,
}

enum AutomationKind {
    Tempo(f64),
    Sync {
        occurrence: u32,
        frame_offset: Option<f64>,
        millisecond_offset: Option<f64>,
    },
}

struct Automation {
    bar: i64,
    position: f64,
    kind: AutomationKind,
}

#[derive(Default)]
struct RawTrack<'d> {
    track: Track,
    staff_count: usize,
    /// Chord diagram id → chord name.
    chords: HashMap<&'d str, &'d str>,
    /// Drum kit articulation index → output MIDI note (GP7+ percussion).
    drum_articulations: Vec<Option<u8>>,
}

#[derive(Default)]
struct RawBeat<'d> {
    beat: Beat,
    rhythm: Option<&'d str>,
    notes: Vec<&'d str>,
    chord: Option<&'d str>,
    velocity: u8,
}

#[derive(Default)]
struct RawNote {
    note: Note,
    string: Option<i64>,
    fret: Option<i64>,
    midi: Option<i64>,
    octave: Option<i64>,
    tone: Option<i64>,
    /// MIDI pitch from `ConcertPitch` (step letter, accidental, octave).
    concert_pitch: Option<i64>,
    /// GP7+ drum articulation index into the track's instrument set.
    articulation: Option<i64>,
    /// GP6 drum element/variation pair.
    element: Option<i64>,
    variation: Option<i64>,
    /// `<Trill>` holds the MIDI pitch of the trill note.
    trill_pitch: Option<i64>,
    /// Right-hand tapping (`Tapped` property): marks the beat as tapped.
    tapped: bool,
}

fn split_ids(s: &str) -> Vec<&str> {
    s.split_ascii_whitespace().collect()
}

fn int(s: Option<&str>) -> Option<i64> {
    let s = s?.trim();
    s.parse::<i64>()
        .ok()
        .or_else(|| float(Some(s)).map(|f| f as i64))
}

fn float(s: Option<&str>) -> Option<f64> {
    s?.trim().parse::<f64>().ok().filter(|f| f.is_finite())
}

fn clamp_u8(v: i64) -> u8 {
    v.clamp(0, 255) as u8
}

fn is_true(s: Option<&str>) -> bool {
    s.is_some_and(|s| s.trim().eq_ignore_ascii_case("true"))
}

impl<'d> Tables<'d> {
    fn read(root: El<'d>) -> Self {
        let mut t = Tables::default();
        for c in root.children() {
            match c.name() {
                "Score" => t.read_score(c),
                "MasterTrack" => t.read_master_track(c),
                "BackingTrack" => {
                    if let Some(frames) = float(c.child_text("FramePadding")) {
                        t.sync_padding_ms = frames / SYNC_SAMPLE_RATE * 1000.0;
                    }
                }
                "Tracks" => {
                    for tr in c.children().filter(|e| e.name() == "Track") {
                        t.tracks.push((tr.attr("id").unwrap_or(""), read_track(tr)));
                    }
                }
                "MasterBars" => {
                    for mb in c.children().filter(|e| e.name() == "MasterBar") {
                        t.master_bars.push(read_master_bar(mb));
                    }
                }
                "Bars" => {
                    for b in c.children().filter(|e| e.name() == "Bar") {
                        let voices = b.child_text("Voices").map(split_ids).unwrap_or_default();
                        t.bars.insert(b.attr("id").unwrap_or(""), voices);
                    }
                }
                "Voices" => {
                    for v in c.children().filter(|e| e.name() == "Voice") {
                        let beats = v.child_text("Beats").map(split_ids).unwrap_or_default();
                        t.voices.insert(v.attr("id").unwrap_or(""), beats);
                    }
                }
                "Beats" => {
                    for b in c.children().filter(|e| e.name() == "Beat") {
                        t.beats.insert(b.attr("id").unwrap_or(""), read_beat(b));
                    }
                }
                "Notes" => {
                    for n in c.children().filter(|e| e.name() == "Note") {
                        t.notes.insert(n.attr("id").unwrap_or(""), read_note(n));
                    }
                }
                "Rhythms" => {
                    for r in c.children().filter(|e| e.name() == "Rhythm") {
                        t.rhythms.insert(r.attr("id").unwrap_or(""), read_rhythm(r));
                    }
                }
                _ => {}
            }
        }
        t
    }

    fn read_score(&mut self, el: El<'d>) {
        let s = &mut self.score;
        let mut words_and_music = "";
        for c in el.children() {
            let text = c.text().to_owned();
            match c.name() {
                "Title" => s.title = text,
                "SubTitle" => s.subtitle = text,
                "Artist" => s.artist = text,
                "Album" => s.album = text,
                "Words" => s.words = text,
                "Music" => s.music = text,
                "WordsAndMusic" => words_and_music = c.text(),
                "Tabber" => s.tab_author = text,
                _ => {}
            }
        }
        if !words_and_music.is_empty() {
            if s.words.is_empty() {
                s.words = words_and_music.to_owned();
            }
            if s.music.is_empty() {
                s.music = words_and_music.to_owned();
            }
        }
    }

    fn read_master_track(&mut self, el: El<'d>) {
        for c in el.children() {
            match c.name() {
                "Tracks" => self.track_order = split_ids(c.text()),
                "Anacrusis" => self.anacrusis = true,
                "Automations" => {
                    for a in c.children().filter(|e| e.name() == "Automation") {
                        if let Some(a) = read_automation(a) {
                            self.automations.push(a);
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

fn read_automation(el: El<'_>) -> Option<Automation> {
    let kind = el.child_text("Type")?;
    let mut bar = int(el.child_text("Bar")).unwrap_or(-1);
    let position = float(el.child_text("Position"))
        .unwrap_or(0.0)
        .clamp(0.0, 1.0);
    let value = el.child("Value");
    let kind = match kind {
        "Tempo" => {
            let mut parts = value
                .map(|v| v.text())
                .unwrap_or("")
                .split_ascii_whitespace();
            let bpm = float(parts.next())?;
            // Beat unit of the tempo marking, in quarter notes (alphaTab's reference table).
            let unit = match int(parts.next()) {
                Some(1) => 0.5,
                Some(3) => 1.5,
                Some(4) => 2.0,
                Some(5) => 3.0,
                _ => 1.0,
            };
            let bpm = bpm * unit;
            if bpm <= 0.0 {
                return None;
            }
            AutomationKind::Tempo(bpm)
        }
        "SyncPoint" => {
            let value = value?;
            if let Some(b) = int(value.child_text("BarIndex")) {
                bar = b;
            }
            AutomationKind::Sync {
                occurrence: int(value.child_text("BarOccurrence"))
                    .unwrap_or(0)
                    .clamp(0, i64::from(u32::MAX)) as u32,
                frame_offset: float(value.child_text("FrameOffset")),
                millisecond_offset: float(value.child_text("MillisecondOffset")),
            }
        }
        _ => return None,
    };
    Some(Automation {
        bar,
        position,
        kind,
    })
}

#[derive(Default)]
struct StaffProps<'d> {
    tuning: Option<Vec<u8>>,
    capo: Option<u8>,
    chords: Vec<(&'d str, &'d str)>,
}

/// `Properties` of a GP6 track or a GP7 staff.
fn read_staff_properties<'d>(el: El<'d>, out: &mut StaffProps<'d>) {
    for p in el.children().filter(|e| e.name() == "Property") {
        match p.attr("name").unwrap_or("") {
            // "Tunning" is a misspelling found in some files; accept both.
            "Tuning" | "Tunning" => {
                if let Some(pitches) = p.child_text("Pitches") {
                    // Lowest string first in GPIF; the model wants string 1 (highest) first.
                    let mut tuning: Vec<u8> = pitches
                        .split_ascii_whitespace()
                        .filter_map(|s| int(Some(s)))
                        .map(|v| v.clamp(0, 127) as u8)
                        .collect();
                    tuning.reverse();
                    out.tuning = Some(tuning);
                }
            }
            "CapoFret" => out.capo = int(p.child_text("Fret")).map(clamp_u8),
            "DiagramCollection" | "ChordCollection" => {
                if let Some(items) = p.child("Items") {
                    for item in items.children().filter(|e| e.name() == "Item") {
                        if let (Some(id), Some(name)) = (item.attr("id"), item.attr("name")) {
                            out.chords.push((id, name));
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

fn read_track(el: El<'_>) -> RawTrack<'_> {
    let mut rt = RawTrack {
        staff_count: 1,
        ..Default::default()
    };
    let mut track_props = StaffProps::default();
    let mut staff_props = StaffProps::default();
    let mut staves = 0usize;
    let mut midi_program = None;
    let mut sound_program = None;
    for c in el.children() {
        match c.name() {
            "Name" => rt.track.name = c.text().to_owned(),
            "ShortName" => rt.track.short_name = c.text().to_owned(),
            "Instrument" => {
                let r = c.attr("ref").unwrap_or("");
                if r.ends_with("-gs") || r.ends_with("GrandStaff") {
                    rt.staff_count = rt.staff_count.max(2);
                }
            }
            "InstrumentSet" => {
                if c.child_text("Type") == Some("drumKit") {
                    rt.track.is_percussion = true;
                }
                if let Some(elements) = c.child("Elements") {
                    for e in elements.children().filter(|e| e.name() == "Element") {
                        let Some(arts) = e.child("Articulations") else {
                            continue;
                        };
                        for a in arts.children().filter(|e| e.name() == "Articulation") {
                            let midi = int(a.child_text("OutputMidiNumber"))
                                .filter(|m| (0..=127).contains(m))
                                .map(|m| m as u8);
                            rt.drum_articulations.push(midi);
                        }
                    }
                }
            }
            "Properties" => read_staff_properties(c, &mut track_props),
            "GeneralMidi" | "MidiConnection" | "MIDISettings" => {
                if let Some(p) = int(c.child_text("Program")) {
                    midi_program = Some(p);
                }
                if c.attr("table") == Some("Percussion")
                    || int(c.child_text("PrimaryChannel")) == Some(9)
                {
                    rt.track.is_percussion = true;
                }
            }
            "Sounds" => {
                let first = c.children().find(|e| e.name() == "Sound");
                if let Some(p) = first
                    .and_then(|s| s.child("MIDI"))
                    .map(|m| m.child_text("Program"))
                {
                    sound_program = int(p);
                }
            }
            "Staves" => {
                for (i, s) in c.children().filter(|e| e.name() == "Staff").enumerate() {
                    staves += 1;
                    if i == 0
                        && let Some(props) = s.child("Properties")
                    {
                        read_staff_properties(props, &mut staff_props);
                    }
                }
            }
            _ => {}
        }
    }
    rt.staff_count = rt.staff_count.max(staves);
    rt.track.program = sound_program.or(midi_program).unwrap_or(0).clamp(0, 127) as u8;
    // GP7 keeps tuning and capo on the first staff; GP6 on the track. Staff values win.
    rt.track.tuning = staff_props
        .tuning
        .or(track_props.tuning)
        .unwrap_or_default();
    rt.track.capo = staff_props.capo.or(track_props.capo).unwrap_or(0);
    rt.chords = track_props
        .chords
        .into_iter()
        .chain(staff_props.chords)
        .collect();
    rt
}

fn read_master_bar(el: El<'_>) -> (MasterBar, Vec<&str>) {
    let mut mb = MasterBar::default();
    let mut bars = Vec::new();
    for c in el.children() {
        match c.name() {
            "Time" => {
                let (n, d) = c.text().split_once('/').unwrap_or((c.text(), ""));
                mb.time_signature = TimeSignature {
                    numerator: int(Some(n)).filter(|n| (1..=255).contains(n)).unwrap_or(4) as u8,
                    denominator: int(Some(d)).filter(|d| (1..=128).contains(d)).unwrap_or(4) as u8,
                };
            }
            "Key" => {
                mb.key_signature = int(c.child_text("AccidentalCount"))
                    .unwrap_or(0)
                    .clamp(-7, 7) as i8;
            }
            "Repeat" => {
                mb.repeat_start = is_true(c.attr("start"));
                if is_true(c.attr("end")) {
                    // `count` is the total number of plays; a closing repeat plays at least twice.
                    mb.repeat_end = int(c.attr("count")).unwrap_or(2).clamp(2, 255) as u8;
                }
            }
            "AlternateEndings" => {
                mb.alternate_endings = c
                    .text()
                    .split_ascii_whitespace()
                    .filter_map(|s| int(Some(s)))
                    .filter(|n| (1..=8).contains(n))
                    .fold(0u8, |acc, n| acc | (1 << (n - 1)));
            }
            "Section" => {
                mb.section = Some(Section {
                    letter: c.child_text("Letter").unwrap_or("").to_owned(),
                    text: c.child_text("Text").unwrap_or("").to_owned(),
                });
            }
            "Directions" => {
                for d in c.children() {
                    let dir = match (d.name(), d.text()) {
                        ("Target", t) => target_direction(t),
                        ("Jump", t) => jump_direction(t),
                        _ => None,
                    };
                    if let Some(dir) = dir
                        && !mb.directions.contains(&dir)
                    {
                        mb.directions.push(dir);
                    }
                }
            }
            "TripletFeel" => {
                mb.triplet_feel = match c.text() {
                    "Triplet8th" => TripletFeel::Triplet8th,
                    "Triplet16th" => TripletFeel::Triplet16th,
                    "Dotted8th" => TripletFeel::Dotted8th,
                    "Dotted16th" => TripletFeel::Dotted16th,
                    "Scottish8th" => TripletFeel::Scottish8th,
                    "Scottish16th" => TripletFeel::Scottish16th,
                    _ => TripletFeel::None,
                };
            }
            "DoubleBar" => mb.double_bar = true,
            "Bars" => bars = split_ids(c.text()),
            _ => {}
        }
    }
    (mb, bars)
}

fn target_direction(s: &str) -> Option<Direction> {
    Some(match s {
        "Coda" => Direction::Coda,
        "DoubleCoda" => Direction::DoubleCoda,
        "Segno" => Direction::Segno,
        "SegnoSegno" => Direction::SegnoSegno,
        "Fine" => Direction::Fine,
        _ => return None,
    })
}

fn jump_direction(s: &str) -> Option<Direction> {
    // GPIF spells "Dal Segno" as "DaSegno"; accept the correct spelling too.
    let s = s
        .strip_prefix("Dal")
        .map_or_else(|| s.to_owned(), |rest| format!("Da{rest}"));
    Some(match s.as_str() {
        "DaCapo" => Direction::DaCapo,
        "DaCapoAlCoda" => Direction::DaCapoAlCoda,
        "DaCapoAlDoubleCoda" => Direction::DaCapoAlDoubleCoda,
        "DaCapoAlFine" => Direction::DaCapoAlFine,
        "DaSegno" => Direction::DalSegno,
        "DaSegnoAlCoda" => Direction::DalSegnoAlCoda,
        "DaSegnoAlDoubleCoda" => Direction::DalSegnoAlDoubleCoda,
        "DaSegnoAlFine" => Direction::DalSegnoAlFine,
        "DaSegnoSegno" => Direction::DalSegnoSegno,
        "DaSegnoSegnoAlCoda" => Direction::DalSegnoSegnoAlCoda,
        "DaSegnoSegnoAlDoubleCoda" => Direction::DalSegnoSegnoAlDoubleCoda,
        "DaSegnoSegnoAlFine" => Direction::DalSegnoSegnoAlFine,
        "DaCoda" => Direction::DaCoda,
        "DaDoubleCoda" => Direction::DaDoubleCoda,
        _ => return None,
    })
}

fn read_rhythm(el: El<'_>) -> Duration {
    let mut d = Duration::default();
    for c in el.children() {
        match c.name() {
            "NoteValue" => {
                d.value = match c.text() {
                    // The model's shortest/longest values; longer notes are clamped to a whole.
                    "Long" | "DoubleWhole" | "Whole" => 1,
                    "Half" => 2,
                    "Quarter" => 4,
                    "Eighth" => 8,
                    "16th" => 16,
                    "32nd" => 32,
                    "64th" => 64,
                    "128th" | "256th" => 128,
                    _ => 4,
                };
            }
            "AugmentationDot" => d.dots = int(c.attr("count")).unwrap_or(0).clamp(0, 2) as u8,
            "PrimaryTuplet" => {
                let n = int(c.attr("num")).unwrap_or(0);
                let m = int(c.attr("den")).unwrap_or(0);
                if (1..=255).contains(&n) && (1..=255).contains(&m) && n != m {
                    d.tuplet = Some((n as u8, m as u8));
                }
            }
            _ => {}
        }
    }
    d
}

/// Collects GPIF bend/whammy points, all values in GPIF units.
#[derive(Default)]
struct BendBuilder {
    enabled: bool,
    origin_offset: Option<f64>,
    origin_value: Option<f64>,
    middle_value: Option<f64>,
    middle_offset1: Option<f64>,
    middle_offset2: Option<f64>,
    destination_offset: Option<f64>,
    destination_value: Option<f64>,
}

impl BendBuilder {
    /// Handle a `<Property name="{prefix}…">` (e.g. `BendOriginValue`). Returns true if consumed.
    fn property(&mut self, name: &str, prefix: &str, flag: &str, prop: El<'_>) -> bool {
        if name == flag {
            self.enabled = true;
            return true;
        }
        let Some(field) = name.strip_prefix(prefix) else {
            return false;
        };
        let v = float(prop.child_text("Float"));
        let slot = match field {
            "OriginOffset" => &mut self.origin_offset,
            "OriginValue" => &mut self.origin_value,
            "MiddleValue" => &mut self.middle_value,
            "MiddleOffset1" => &mut self.middle_offset1,
            "MiddleOffset2" => &mut self.middle_offset2,
            "DestinationOffset" => &mut self.destination_offset,
            "DestinationValue" => &mut self.destination_value,
            _ => return false,
        };
        *slot = v;
        true
    }

    fn build(&self) -> Option<Bend> {
        if !self.enabled {
            return None;
        }
        let pt = |offset: f64, value: f64| {
            (
                (offset / 100.0).clamp(0.0, 1.0) as f32,
                // 100 GPIF units = a whole tone = 2 semitones.
                (value / 50.0) as f32,
            )
        };
        let nonzero = |v: Option<f64>| v.filter(|v| *v != 0.0);
        let mut points = vec![pt(
            self.origin_offset.unwrap_or(0.0),
            self.origin_value.unwrap_or(0.0),
        )];
        if let Some(mid) = nonzero(self.middle_value) {
            let o1 = nonzero(self.middle_offset1);
            let o2 = nonzero(self.middle_offset2);
            if o1.is_none() && o2.is_none() {
                points.push(pt(50.0, mid));
            }
            points.extend(o1.into_iter().chain(o2).map(|o| pt(o, mid)));
        }
        points.push(pt(
            self.destination_offset.unwrap_or(100.0),
            self.destination_value.unwrap_or(0.0),
        ));
        points.sort_by(|a, b| a.0.total_cmp(&b.0));
        Some(Bend { points })
    }
}

fn velocity_for_dynamic(s: &str) -> u8 {
    match s {
        "PPP" => 15,
        "PP" => 31,
        "P" => 47,
        "MP" => 63,
        "MF" => 79,
        "F" => 95,
        "FF" => 111,
        "FFF" => 127,
        _ => DEFAULT_VELOCITY,
    }
}

fn read_beat(el: El<'_>) -> RawBeat<'_> {
    let mut rb = RawBeat {
        velocity: DEFAULT_VELOCITY,
        ..Default::default()
    };
    let fx = &mut rb.beat.effects;
    for c in el.children() {
        match c.name() {
            "Rhythm" => rb.rhythm = c.attr("ref"),
            "Notes" => rb.notes = split_ids(c.text()),
            "Dynamic" => rb.velocity = velocity_for_dynamic(c.text()),
            "GraceNotes" => fx.grace = matches!(c.text(), "OnBeat" | "BeforeBeat"),
            "FreeText" => {
                if !c.text().is_empty() {
                    rb.beat.text = Some(c.text().to_owned());
                }
            }
            "Chord" => rb.chord = Some(c.text()),
            "Fadding" => fx.fade_in = c.text() == "FadeIn",
            "Tremolo" => {
                fx.tremolo_picking = match c.text() {
                    "1/2" => Some(8),
                    "1/4" => Some(16),
                    "1/8" => Some(32),
                    _ => None,
                };
            }
            "Arpeggio" => {
                fx.stroke = Some(if c.text() == "Up" {
                    Stroke::Up
                } else {
                    Stroke::Down
                });
            }
            "Whammy" => {
                // GP6 stores the whammy as attributes of a <Whammy> element.
                let a = |n: &str| float(c.attr(n));
                let b = BendBuilder {
                    enabled: true,
                    origin_value: a("originValue"),
                    origin_offset: a("originOffset"),
                    middle_value: a("middleValue"),
                    middle_offset1: a("middleOffset1"),
                    middle_offset2: a("middleOffset2"),
                    destination_value: a("destinationValue"),
                    destination_offset: a("destinationOffset"),
                };
                fx.whammy = b.build();
            }
            "Properties" => {
                let mut whammy = BendBuilder::default();
                for p in c.children().filter(|e| e.name() == "Property") {
                    let name = p.attr("name").unwrap_or("");
                    match name {
                        "Brush" => {
                            fx.stroke = Some(if p.child_text("Direction") == Some("Up") {
                                Stroke::Up
                            } else {
                                Stroke::Down
                            });
                        }
                        "Slapped" => fx.slap = p.child("Enable").is_some(),
                        "Popped" => fx.pop = p.child("Enable").is_some(),
                        _ => {
                            whammy.property(name, "WhammyBar", "WhammyBar", p);
                        }
                    }
                }
                if whammy.enabled {
                    fx.whammy = whammy.build();
                }
            }
            _ => {}
        }
    }
    rb
}

fn read_note(el: El<'_>) -> RawNote {
    let mut rn = RawNote::default();
    let mut bend = BendBuilder::default();
    for c in el.children() {
        let fx = &mut rn.note.effects;
        match c.name() {
            "Properties" => {
                for p in c.children().filter(|e| e.name() == "Property") {
                    let name = p.attr("name").unwrap_or("");
                    let enabled = p.child("Enable").is_some();
                    let fx = &mut rn.note.effects;
                    match name {
                        "String" => rn.string = int(p.child_text("String")),
                        "Fret" => rn.fret = int(p.child_text("Fret")),
                        "Midi" => rn.midi = int(p.child_text("Number")),
                        "Octave" => rn.octave = int(p.child_text("Number")),
                        "Tone" => rn.tone = int(p.child_text("Step")),
                        "ConcertPitch" => {
                            rn.concert_pitch = p.child("Pitch").and_then(concert_pitch);
                        }
                        "Element" => rn.element = int(p.child_text("Element")),
                        "Variation" => rn.variation = int(p.child_text("Variation")),
                        "Tapped" => {
                            rn.tapped = true;
                            fx.tapped = true;
                        }
                        "LeftHandTapped" => fx.tapped = true,
                        "Muted" => fx.dead = enabled,
                        "PalmMuted" => fx.palm_mute = enabled,
                        "HopoOrigin" => fx.hammer_pull = enabled,
                        "HarmonicType" => {
                            fx.harmonic = match p
                                .child_text("HType")
                                .unwrap_or("")
                                .to_ascii_lowercase()
                                .as_str()
                            {
                                "natural" => Some(Harmonic::Natural),
                                "artificial" => Some(Harmonic::Artificial),
                                "pinch" => Some(Harmonic::Pinch),
                                "tap" => Some(Harmonic::Tapped),
                                "semi" => Some(Harmonic::Semi),
                                _ => None,
                            };
                        }
                        "Slide" => {
                            let flags = int(p.child_text("Flags")).unwrap_or(0);
                            fx.slide = slide_from_flags(flags);
                        }
                        _ => {
                            bend.property(name, "Bend", "Bended", p);
                        }
                    }
                }
            }
            "AntiAccent" => fx.ghost = c.text().eq_ignore_ascii_case("normal"),
            "LetRing" => fx.let_ring = true,
            "Vibrato" => fx.vibrato = matches!(c.text(), "Slight" | "Wide"),
            "Trill" => rn.trill_pitch = int(Some(c.text())),
            "Accent" => {
                let flags = int(Some(c.text())).unwrap_or(0);
                fx.staccato = flags & 0x01 != 0;
                fx.heavy_accent = flags & 0x04 != 0;
                fx.accent = flags & 0x08 != 0;
            }
            "Tie" => rn.note.tie_destination = is_true(c.attr("destination")),
            "InstrumentArticulation" => rn.articulation = int(Some(c.text())),
            _ => {}
        }
    }
    rn.note.effects.bend = bend.build();
    rn
}

/// `<Pitch><Step>G</Step><Accidental>#</Accidental><Octave>4</Octave></Pitch>` → MIDI (C4 = 60).
fn concert_pitch(el: El<'_>) -> Option<i64> {
    let step = match el.child_text("Step")? {
        "C" => 0,
        "D" => 2,
        "E" => 4,
        "F" => 5,
        "G" => 7,
        "A" => 9,
        "B" => 11,
        _ => return None,
    };
    let accidental = match el.child_text("Accidental").unwrap_or("") {
        "#" => 1,
        "x" => 2,
        "b" => -1,
        "bb" => -2,
        _ => 0,
    };
    let octave = int(el.child_text("Octave"))?;
    Some((octave + 1) * 12 + step + accidental)
}

/// GPIF slide bitmask → the model's single slide. Slides out (to the next note) win over slides
/// in; pick slides (64/128) map to slides out.
fn slide_from_flags(flags: i64) -> Option<Slide> {
    [
        (1, Slide::Shift),
        (2, Slide::Legato),
        (4, Slide::OutDown),
        (8, Slide::OutUp),
        (16, Slide::InFromBelow),
        (32, Slide::InFromAbove),
        (64, Slide::OutDown),
        (128, Slide::OutUp),
    ]
    .into_iter()
    .find(|(bit, _)| flags & bit != 0)
    .map(|(_, s)| s)
}

/// GP6 drum element/variation → General MIDI drum note (from alphaTab's `PercussionMapper`).
fn gp6_drum_midi(element: i64, variation: i64) -> u8 {
    const TABLE: [[u8; 3]; 17] = [
        [35, 35, 35], // kick
        [38, 38, 37], // snare: hit, rim shot, side stick
        [56, 56, 56], // cowbell low
        [56, 56, 56], // cowbell medium
        [56, 56, 56], // cowbell high
        [43, 43, 43], // tom very low
        [45, 45, 45], // tom low
        [47, 47, 47], // tom medium
        [48, 48, 48], // tom high
        [50, 50, 50], // tom very high
        [42, 46, 46], // hi-hat: closed, half, open
        [44, 44, 44], // pedal hi-hat
        [57, 57, 57], // crash medium
        [49, 49, 49], // crash high
        [55, 55, 55], // splash
        [51, 51, 59], // ride: middle, edge, bell
        [52, 52, 52], // china
    ];
    let Some(row) = usize::try_from(element).ok().and_then(|e| TABLE.get(e)) else {
        return 38; // unknown: snare, as alphaTab does
    };
    let v = usize::try_from(variation)
        .ok()
        .filter(|v| *v < 3)
        .unwrap_or(0);
    row[v]
}

// ---------------------------------------------------------------------------------------------
// Pass 2: resolve references into the model.
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum TrackKind {
    Stringed,
    Percussion,
    /// No tuning: piano, synth … Notes are pitches on a single zero-tuned string.
    Pitched,
}

impl<'d> Tables<'d> {
    fn build(mut self) -> Score {
        let mut score = std::mem::take(&mut self.score);

        // Tracks, in MasterTrack order (document order if that's missing or useless).
        let mut by_id: HashMap<&str, RawTrack<'d>> = HashMap::new();
        let mut doc_order = Vec::new();
        for (id, t) in std::mem::take(&mut self.tracks) {
            doc_order.push(id);
            by_id.insert(id, t);
        }
        let mut order: Vec<&str> = self
            .track_order
            .iter()
            .copied()
            .filter(|id| by_id.contains_key(id))
            .collect();
        if order.is_empty() {
            order = doc_order;
        }
        let mut tracks: Vec<(RawTrack<'d>, TrackKind)> = Vec::new();
        for id in order {
            let Some(mut t) = by_id.remove(id) else {
                continue; // listed twice
            };
            let kind = if t.track.is_percussion {
                if t.track.tuning.is_empty() {
                    t.track.tuning = vec![0; 6];
                }
                TrackKind::Percussion
            } else if t.track.tuning.is_empty() {
                t.track.tuning = vec![0];
                TrackKind::Pitched
            } else {
                TrackKind::Stringed
            };
            tracks.push((t, kind));
        }

        // Master bars and, per track, the bar at that track's slot in each master bar.
        let master_bars = std::mem::take(&mut self.master_bars);
        let bar_count = master_bars.len();
        for (t, _) in &mut tracks {
            t.track.bars = Vec::with_capacity(bar_count);
        }
        for (_, bar_ids) in &master_bars {
            let mut slot = 0;
            for (t, kind) in &mut tracks {
                let bar = match bar_ids.get(slot) {
                    Some(&id) if id != INVALID_ID => self.resolve_bar(id, t, *kind),
                    _ => Bar::default(),
                };
                t.track.bars.push(bar);
                // Extra staves (e.g. a piano's bass clef) aren't represented in the model.
                slot += t.staff_count;
            }
        }
        score.master_bars = master_bars.into_iter().map(|(mb, _)| mb).collect();
        if let Some(last) = score.master_bars.last_mut() {
            // Guitar Pro flags the final bar as a double bar; it's just the end barline.
            last.double_bar = false;
        }
        if self.anacrusis
            && let Some(first) = score.master_bars.first_mut()
        {
            first.anacrusis = true;
        }
        score.tracks = tracks.into_iter().map(|(t, _)| t.track).collect();

        // Automations.
        score.initial_tempo = DEFAULT_TEMPO;
        for a in &self.automations {
            let Ok(bar) = u32::try_from(a.bar) else {
                continue;
            };
            if bar as usize >= bar_count {
                debug!("gpif: automation on missing bar {bar}");
                continue;
            }
            match a.kind {
                AutomationKind::Tempo(bpm) => score.tempo_changes.push(TempoChange {
                    bar,
                    position: a.position,
                    bpm,
                }),
                AutomationKind::Sync {
                    occurrence,
                    frame_offset,
                    millisecond_offset,
                } => {
                    let ms = match (frame_offset, millisecond_offset) {
                        (Some(f), _) => f / SYNC_SAMPLE_RATE * 1000.0 - self.sync_padding_ms,
                        (None, Some(ms)) => ms,
                        (None, None) => continue,
                    };
                    score.sync_points.push(FileSyncPoint {
                        bar,
                        bar_occurrence: occurrence,
                        bar_position: a.position,
                        millisecond_offset: ms,
                    });
                }
            }
        }
        score.tempo_changes.sort_by(|a, b| {
            (a.bar, a.position)
                .partial_cmp(&(b.bar, b.position))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        if let Some(first) = score.tempo_changes.first()
            && first.bar == 0
            && first.position == 0.0
        {
            score.initial_tempo = first.bpm;
        }
        score
            .sync_points
            .sort_by(|a, b| a.millisecond_offset.total_cmp(&b.millisecond_offset));
        score
    }

    fn resolve_bar(&self, id: &str, track: &RawTrack<'d>, kind: TrackKind) -> Bar {
        let Some(voice_ids) = self.bars.get(id) else {
            debug!("gpif: missing bar {id}");
            return Bar::default();
        };
        let mut bar = Bar::default();
        let mut placeholders = 0;
        for &vid in voice_ids.iter().take(4) {
            let voice = (vid != INVALID_ID).then(|| self.voices.get(vid)).flatten();
            let Some(beat_ids) = voice else {
                placeholders += 1;
                continue;
            };
            // Keep voice indices stable: an absent voice before a present one stays empty.
            bar.voices
                .extend(std::iter::repeat_with(Voice::default).take(placeholders));
            placeholders = 0;
            let beats = beat_ids
                .iter()
                .filter(|&&b| b != INVALID_ID)
                .filter_map(|&b| self.resolve_beat(b, track, kind))
                .collect();
            bar.voices.push(Voice { beats });
        }
        bar
    }

    fn resolve_beat(&self, id: &str, track: &RawTrack<'d>, kind: TrackKind) -> Option<Beat> {
        let Some(raw) = self.beats.get(id) else {
            debug!("gpif: missing beat {id}");
            return None;
        };
        let Some(duration) = raw.rhythm.and_then(|r| self.rhythms.get(r)) else {
            debug!("gpif: beat {id} has no valid rhythm");
            return None;
        };
        let mut beat = raw.beat.clone();
        beat.duration = *duration;
        beat.chord = raw
            .chord
            .and_then(|c| track.chords.get(c))
            .filter(|name| !name.is_empty())
            .map(|name| (*name).to_owned());
        for (i, &nid) in raw.notes.iter().enumerate() {
            if nid == INVALID_ID {
                continue;
            }
            let Some(rn) = self.notes.get(nid) else {
                debug!("gpif: missing note {nid}");
                continue;
            };
            if let Some(note) = resolve_note(rn, i, raw.velocity, track, kind) {
                beat.effects.tapping |= rn.tapped;
                beat.notes.push(note);
            }
        }
        Some(beat)
    }
}

fn resolve_note(
    rn: &RawNote,
    index_in_beat: usize,
    velocity: u8,
    track: &RawTrack<'_>,
    kind: TrackKind,
) -> Option<Note> {
    let mut note = rn.note.clone();
    note.velocity = velocity;
    let tuning = &track.track.tuning;
    let pitch = || {
        rn.midi
            .or(rn.concert_pitch)
            .or_else(|| Some(rn.octave? * 12 + rn.tone.unwrap_or(0)))
            .filter(|p| (0..=127).contains(p))
    };
    match kind {
        TrackKind::Stringed => {
            let capo = i64::from(track.track.capo);
            let open_of = |string: usize| i64::from(tuning[string - 1]) + capo;
            let n = tuning.len() as i64;
            let (string, fret) = match rn.string.filter(|s| (0..n).contains(s)) {
                Some(s) => {
                    let string = (n - s) as usize;
                    let fret = rn.fret.or_else(|| pitch().map(|p| p - open_of(string)))?;
                    (string, fret)
                }
                None => {
                    // No (valid) string: put the pitch on the highest string that can play it.
                    let p = pitch()?;
                    let string = (1..=tuning.len()).find(|&s| p >= open_of(s))?;
                    (string, p - open_of(string))
                }
            };
            if !(0..=255).contains(&fret) {
                return None;
            }
            note.string = string as u8;
            note.fret = fret as u8;
            let open = open_of(string);
            note.effects.trill = rn
                .trill_pitch
                .map(|p| p - open)
                .filter(|f| (0..=255).contains(f))
                .map(|f| f as u8);
        }
        TrackKind::Percussion => {
            let midi = rn
                .articulation
                .and_then(|a| usize::try_from(a).ok())
                .and_then(|a| track.drum_articulations.get(a).copied().flatten())
                .or_else(|| {
                    let (e, v) = (rn.element?, rn.variation.unwrap_or(0));
                    Some(gp6_drum_midi(e, v))
                })
                .or_else(|| pitch().map(|p| p as u8))?;
            note.string = (index_in_beat + 1).min(tuning.len()) as u8;
            note.fret = midi;
        }
        TrackKind::Pitched => {
            note.string = 1;
            note.fret = pitch()? as u8;
        }
    }
    Some(note)
}

#[cfg(test)]
mod tests;
