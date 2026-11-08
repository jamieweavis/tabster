//! Guitar Pro 5 `.gp5` binary format (versions 5.00 and 5.10).
//! Owner: Phase 1 agent B.
//!
//! The byte layout follows alphaTab's `Gp3To5Importer.ts` (<https://github.com/CoderLine/alphaTab>,
//! Copyright © CoderLine and contributors, MPL-2.0) and PyGuitarPro's `gp5.py`/`gp4.py`/`gp3.py`
//! (<https://github.com/Perlence/PyGuitarPro>, used as a format specification). This is an
//! independent implementation: everything is read with bounds checks, corrupt counts are capped,
//! and the result is mapped straight into [`crate::model`].
//!
//! Mapping notes:
//! - GP numbers strings from 1 = highest, the same as the model.
//! - Bend and whammy values are stored in 1/100 of a whole tone (100 = full bend), positions in
//!   0..=60; the model gets semitones (`raw / 50`) and 0.0..=1.0 positions.
//! - Grace notes become a separate beat inserted *before* the main beat, with
//!   `effects.grace = true` and a 64th-note duration (GP stores them as a note effect). Grace beats
//!   are not counted when positioning tempo changes.
//! - "Empty" beats (GP's placeholder status) are dropped; rests are kept as beats without notes.
//! - `BeatEffects::tremolo_picking` holds the note value of the tremolo (8, 16 or 32).
//! - Beat-level vibrato is applied to every note of the beat.
//! - Mix-table instrument/volume/pan changes, lyrics, RSE, page setup, chord diagrams (only the
//!   chord name is kept), fingering and ottava are skipped.

use super::ParseError;
use crate::model::{
    Bar, Beat, BeatEffects, Bend, Direction, Duration, Harmonic, MasterBar, Note, Score, Section,
    Slide, Stroke, TempoChange, TimeSignature, Track, TripletFeel, Voice,
};

const MAX_BARS: i32 = 10_000;
const MAX_TRACKS: i32 = 128;
const MAX_BEATS_PER_VOICE: i32 = 1_024;
const MAX_NOTICE_LINES: i32 = 1_000;
const MAX_BEND_POINTS: i32 = 240;
/// Ties are resolved by looking this many bars back at most.
const TIE_LOOKBACK_BARS: usize = 16;
const DEFAULT_VELOCITY: u8 = 95; // forte

const DIRECTION_ORDER: [Direction; 19] = [
    Direction::Coda,
    Direction::DoubleCoda,
    Direction::Segno,
    Direction::SegnoSegno,
    Direction::Fine,
    Direction::DaCapo,
    Direction::DaCapoAlCoda,
    Direction::DaCapoAlDoubleCoda,
    Direction::DaCapoAlFine,
    Direction::DalSegno,
    Direction::DalSegnoAlCoda,
    Direction::DalSegnoAlDoubleCoda,
    Direction::DalSegnoAlFine,
    Direction::DalSegnoSegno,
    Direction::DalSegnoSegnoAlCoda,
    Direction::DalSegnoSegnoAlDoubleCoda,
    Direction::DalSegnoSegnoAlFine,
    Direction::DaCoda,
    Direction::DaDoubleCoda,
];

pub fn parse(bytes: &[u8]) -> Result<Score, ParseError> {
    Parser {
        r: Reader::new(bytes),
        version: 0,
        score: Score::default(),
    }
    .parse()
}

// ---------------------------------------------------------------------------------------------
// Byte reader
// ---------------------------------------------------------------------------------------------

/// Little-endian reader over a byte slice. Every read is bounds-checked and returns
/// [`ParseError::Eof`] past the end; nothing is allocated before the bytes are known to exist.
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn bytes(&mut self, n: usize) -> Result<&'a [u8], ParseError> {
        let end = self.pos.checked_add(n).ok_or(ParseError::Eof)?;
        let slice = self.data.get(self.pos..end).ok_or(ParseError::Eof)?;
        self.pos = end;
        Ok(slice)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], ParseError> {
        let mut out = [0; N];
        out.copy_from_slice(self.bytes(N)?);
        Ok(out)
    }

    fn skip(&mut self, n: usize) -> Result<(), ParseError> {
        self.bytes(n).map(|_| ())
    }

    fn u8(&mut self) -> Result<u8, ParseError> {
        Ok(self.array::<1>()?[0])
    }

    fn i8(&mut self) -> Result<i8, ParseError> {
        Ok(i8::from_le_bytes(self.array()?))
    }

    fn i16(&mut self) -> Result<i16, ParseError> {
        Ok(i16::from_le_bytes(self.array()?))
    }

    fn i32(&mut self) -> Result<i32, ParseError> {
        Ok(i32::from_le_bytes(self.array()?))
    }

    /// A count that must lie in `0..=max`.
    fn count(&mut self, max: i32, what: &str) -> Result<usize, ParseError> {
        let n = self.i32()?;
        if !(0..=max).contains(&n) {
            return Err(ParseError::Invalid(format!(
                "{what} {n} out of range 0..={max}"
            )));
        }
        Ok(n as usize)
    }

    /// Length byte followed by a fixed `field`-byte buffer.
    fn byte_size_string(&mut self, field: usize) -> Result<String, ParseError> {
        let len = usize::from(self.u8()?);
        let buf = self.bytes(field)?;
        Ok(decode_string(&buf[..len.min(field)]))
    }

    /// `i32` total size (length byte + text), then a length byte, then the text.
    fn int_byte_size_string(&mut self) -> Result<String, ParseError> {
        let total = self.i32()?;
        let len = usize::from(self.u8()?);
        // Normally total == len + 1. Trust `total` (it is what GP skips by) unless it's bogus.
        let field = match usize::try_from(total) {
            Ok(t) if t >= 1 => t - 1,
            _ => len,
        };
        let buf = self.bytes(field)?;
        Ok(decode_string(&buf[..len.min(field)]))
    }

    /// `i32` length, then the text.
    fn int_size_string(&mut self) -> Result<String, ParseError> {
        let len = usize::try_from(self.i32()?).unwrap_or(0);
        Ok(decode_string(self.bytes(len)?))
    }
}

/// GP files are usually Windows-1252; accept UTF-8 when the bytes are valid UTF-8.
fn decode_string(bytes: &[u8]) -> String {
    if let Ok(s) = std::str::from_utf8(bytes) {
        return s.to_owned();
    }
    const CP1252_80_9F: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž',
        '\u{8f}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}',
        'ž', 'Ÿ',
    ];
    bytes
        .iter()
        .map(|&b| match b {
            0x80..=0x9F => CP1252_80_9F[usize::from(b - 0x80)],
            _ => char::from(b),
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default)]
struct MidiChannel {
    program: i32,
}

struct Parser<'a> {
    r: Reader<'a>,
    /// 500 or 510.
    version: u32,
    score: Score,
}

/// A note as read from the file, plus the parts the model stores elsewhere.
struct NoteRead {
    /// `None` when the note has no fret (GP ignores such notes).
    note: Option<Note>,
    grace: Option<Note>,
    /// Tremolo picking note value (8, 16, 32).
    tremolo: Option<u8>,
}

/// What the beat reader hands back besides the beat itself.
struct BeatRead {
    /// `None` for GP's "empty" placeholder beats.
    beat: Option<Beat>,
    grace: Option<Beat>,
    tempo: Option<i32>,
}

impl Parser<'_> {
    fn parse(mut self) -> Result<Score, ParseError> {
        self.read_version()?;
        self.read_info()?;
        self.read_lyrics()?;
        if self.version >= 510 {
            self.r.skip(19)?; // RSE master effect: volume, reserved, 11-band equaliser.
        }
        self.read_page_setup()?;
        let _tempo_name = self.r.int_byte_size_string()?;
        let tempo = self.r.i32()?;
        self.score.initial_tempo = if tempo > 0 { f64::from(tempo) } else { 120.0 };
        if self.version >= 510 {
            self.r.skip(1)?; // hide tempo
        }
        let key = self.r.i8()?;
        self.r.skip(4)?; // octave
        let channels = self.read_midi_channels()?;
        let directions = self.read_directions()?;
        self.r.skip(4)?; // master reverb

        let bar_count = self.r.count(MAX_BARS, "bar count")?;
        let track_count = self.r.count(MAX_TRACKS, "track count")?;
        self.read_master_bars(bar_count, key, &directions)?;
        for _ in 0..track_count {
            let track = self.read_track(&channels)?;
            self.score.tracks.push(track);
        }
        self.read_bars()?;
        Ok(self.score)
    }

    fn read_version(&mut self) -> Result<(), ParseError> {
        let version = self.r.byte_size_string(30)?;
        let number = version
            .strip_prefix("FICHIER GUITAR PRO v")
            .ok_or_else(|| ParseError::Invalid(format!("not a Guitar Pro 5 file: {version:?}")))?;
        let (major, minor) = number.split_once('.').unwrap_or((number, "0"));
        let major: u32 = major.trim().parse().unwrap_or(0);
        let minor: u32 = minor.trim().parse().unwrap_or(0);
        if major != 5 {
            return Err(ParseError::Invalid(format!(
                "unsupported Guitar Pro version {version:?}"
            )));
        }
        self.version = if minor >= 10 { 510 } else { 500 };
        Ok(())
    }

    fn read_info(&mut self) -> Result<(), ParseError> {
        let r = &mut self.r;
        let s = &mut self.score;
        s.title = r.int_byte_size_string()?;
        s.subtitle = r.int_byte_size_string()?;
        s.artist = r.int_byte_size_string()?;
        s.album = r.int_byte_size_string()?;
        s.words = r.int_byte_size_string()?;
        s.music = r.int_byte_size_string()?;
        let _copyright = r.int_byte_size_string()?;
        s.tab_author = r.int_byte_size_string()?;
        let _instructions = r.int_byte_size_string()?;
        let notices = r.count(MAX_NOTICE_LINES, "notice line count")?;
        for _ in 0..notices {
            r.int_byte_size_string()?;
        }
        Ok(())
    }

    fn read_lyrics(&mut self) -> Result<(), ParseError> {
        self.r.skip(4)?; // lyrics track
        for _ in 0..5 {
            self.r.skip(4)?; // start bar
            self.r.int_size_string()?;
        }
        Ok(())
    }

    fn read_page_setup(&mut self) -> Result<(), ParseError> {
        self.r.skip(28)?; // page size, margins, score size proportion
        self.r.skip(2)?; // header/footer flags
        for _ in 0..10 {
            self.r.int_byte_size_string()?;
        }
        Ok(())
    }

    /// 4 ports × 16 channels.
    fn read_midi_channels(&mut self) -> Result<Vec<MidiChannel>, ParseError> {
        let mut channels = Vec::with_capacity(64);
        for _ in 0..64 {
            let program = self.r.i32()?;
            self.r.skip(6)?; // volume, balance, chorus, reverb, phaser, tremolo
            self.r.skip(2)?;
            channels.push(MidiChannel { program });
        }
        Ok(channels)
    }

    /// `(bar index, direction)` pairs from the GP5 directions block.
    fn read_directions(&mut self) -> Result<Vec<(usize, Direction)>, ParseError> {
        let mut out = Vec::new();
        for direction in DIRECTION_ORDER {
            let bar = self.r.i16()?;
            if bar > 0 {
                out.push((usize::from(bar.unsigned_abs()) - 1, direction));
            }
        }
        Ok(out)
    }

    fn read_master_bars(
        &mut self,
        count: usize,
        initial_key: i8,
        directions: &[(usize, Direction)],
    ) -> Result<(), ParseError> {
        let mut time_signature = TimeSignature::default();
        let mut key = initial_key;
        self.score.master_bars.reserve(count);
        for index in 0..count {
            let r = &mut self.r;
            let flags = r.u8()?;
            if flags & 0x01 != 0 {
                time_signature.numerator = r.u8()?.max(1);
            }
            if flags & 0x02 != 0 {
                time_signature.denominator = r.u8()?.max(1);
            }
            let mut bar = MasterBar {
                repeat_start: flags & 0x04 != 0,
                double_bar: flags & 0x80 != 0,
                ..Default::default()
            };
            if flags & 0x08 != 0 {
                // GP5 stores the total number of plays.
                bar.repeat_end = r.u8()?.max(1);
            }
            if flags & 0x20 != 0 {
                let text = r.int_byte_size_string()?;
                r.skip(4)?; // colour
                bar.section = Some(Section {
                    letter: String::new(),
                    text,
                });
            }
            if flags & 0x40 != 0 {
                key = r.i8()?;
                r.skip(1)?; // major/minor
            }
            if flags & 0x03 != 0 {
                r.skip(4)?; // beam groups
            }
            bar.alternate_endings = r.u8()?;
            bar.triplet_feel = match r.u8()? {
                1 => TripletFeel::Triplet8th,
                2 => TripletFeel::Triplet16th,
                _ => TripletFeel::None,
            };
            r.skip(1)?;
            bar.time_signature = time_signature;
            bar.key_signature = key;
            bar.directions = directions
                .iter()
                .filter(|(i, _)| *i == index)
                .map(|&(_, d)| d)
                .collect();
            self.score.master_bars.push(bar);
        }
        Ok(())
    }

    fn read_track(&mut self, channels: &[MidiChannel]) -> Result<Track, ParseError> {
        let r = &mut self.r;
        let flags = r.u8()?;
        let name = r.byte_size_string(40)?;
        let string_count = usize::try_from(r.i32()?).unwrap_or(0).min(7);
        let mut tuning = Vec::with_capacity(string_count);
        for i in 0..7 {
            let pitch = r.i32()?;
            if i < string_count {
                tuning.push(pitch.clamp(0, 127) as u8);
            }
        }
        let port = r.i32()?.wrapping_sub(1);
        let channel = r.i32()?.wrapping_sub(1);
        r.skip(4)?; // effect channel
        r.skip(4)?; // fret count
        let capo = r.i32()?.clamp(0, 30) as u8;
        r.skip(4)?; // colour
        r.skip(29)?; // staff flags, auto accentuation, bank, humanise, clef, etc.
        r.skip(16)?; // RSE instrument
        if self.version >= 510 {
            r.skip(4)?; // equaliser
            r.int_byte_size_string()?; // RSE effect
            r.int_byte_size_string()?; // RSE effect category
        }
        let program = if (0..4).contains(&port) && (0..16).contains(&channel) {
            channels
                .get((port * 16 + channel) as usize)
                .map_or(0, |c| c.program.clamp(0, 127) as u8)
        } else {
            0
        };
        Ok(Track {
            name,
            short_name: String::new(),
            program,
            is_percussion: flags & 0x01 != 0 || channel == 9,
            tuning,
            capo,
            bars: Vec::with_capacity(self.score.master_bars.len()),
        })
    }

    fn read_bars(&mut self) -> Result<(), ParseError> {
        for bar_index in 0..self.score.master_bars.len() {
            for track_index in 0..self.score.tracks.len() {
                self.r.skip(1)?;
                let mut bar = Bar::default();
                for voice_index in 0..2 {
                    let voice = self.read_voice(bar_index, track_index, voice_index)?;
                    bar.voices.push(voice);
                }
                if bar.voices[1].beats.is_empty() {
                    bar.voices.pop();
                }
                self.score.tracks[track_index].bars.push(bar);
            }
        }
        self.score.tempo_changes.sort_by(|a, b| {
            (a.bar, a.position)
                .partial_cmp(&(b.bar, b.position))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(())
    }

    fn read_voice(
        &mut self,
        bar_index: usize,
        track_index: usize,
        voice_index: usize,
    ) -> Result<Voice, ParseError> {
        let count = self.r.count(MAX_BEATS_PER_VOICE, "beat count")?;
        let mut voice = Voice::default();
        let bar_ticks = self.score.master_bars[bar_index].time_signature.bar_ticks();
        let mut position = 0u32;
        for _ in 0..count {
            let read = self.read_beat(track_index)?;
            if let Some(bpm) = read.tempo.filter(|&t| t > 0) {
                let change = TempoChange {
                    bar: bar_index as u32,
                    position: (f64::from(position) / f64::from(bar_ticks.max(1))).min(0.999_999),
                    bpm: f64::from(bpm),
                };
                if !self.score.tempo_changes.contains(&change) {
                    self.score.tempo_changes.push(change);
                }
            }
            let Some(mut beat) = read.beat else { continue };
            self.resolve_ties(&mut beat, &voice, bar_index, track_index, voice_index);
            if let Some(grace) = read.grace {
                voice.beats.push(grace);
            }
            position = position.saturating_add(beat.duration.ticks());
            voice.beats.push(beat);
        }
        Ok(voice)
    }

    /// GP stores tied notes with a placeholder fret; copy it from the note they continue.
    fn resolve_ties(
        &self,
        beat: &mut Beat,
        current: &Voice,
        bar_index: usize,
        track_index: usize,
        voice_index: usize,
    ) {
        let track = &self.score.tracks[track_index];
        for note in beat.notes.iter_mut().filter(|n| n.tie_destination) {
            let previous_bars = track
                .bars
                .iter()
                .rev()
                .take(TIE_LOOKBACK_BARS.min(bar_index))
                .filter_map(|b| b.voices.get(voice_index));
            let fret = std::iter::once(current)
                .chain(previous_bars)
                .flat_map(|v| v.beats.iter().rev())
                .filter(|b| !b.effects.grace)
                .find_map(|b| b.notes.iter().find(|n| n.string == note.string))
                .map(|n| n.fret);
            if let Some(fret) = fret {
                note.fret = fret;
            }
        }
    }

    fn read_beat(&mut self, track_index: usize) -> Result<BeatRead, ParseError> {
        let string_count = self.score.tracks[track_index].tuning.len();
        let flags = self.r.u8()?;
        let empty = if flags & 0x40 != 0 {
            self.r.u8()? == 0
        } else {
            false
        };
        let mut beat = Beat {
            duration: self.read_duration(flags)?,
            ..Default::default()
        };
        if flags & 0x02 != 0 {
            beat.chord = self.read_chord()?;
        }
        if flags & 0x04 != 0 {
            beat.text = Some(self.r.int_byte_size_string()?);
        }
        let mut beat_vibrato = false;
        if flags & 0x08 != 0 {
            beat_vibrato = self.read_beat_effects(&mut beat.effects)?;
        }
        let mut tempo = None;
        if flags & 0x10 != 0 {
            tempo = self.read_mix_table_change()?;
        }
        let string_flags = self.r.u8()?;
        let mut grace_notes = Vec::new();
        for bit in (0..7u8).rev() {
            let string = 7 - bit;
            if string_flags & (1 << bit) == 0 || usize::from(string) > string_count {
                continue;
            }
            let read = self.read_note(string)?;
            if let Some(mut note) = read.note {
                note.effects.vibrato |= beat_vibrato;
                beat.notes.push(note);
            }
            grace_notes.extend(read.grace);
            if read.tremolo.is_some() {
                beat.effects.tremolo_picking = read.tremolo;
            }
        }
        let flags2 = self.r.i16()?;
        if flags2 & 0x0800 != 0 {
            self.r.skip(1)?; // break secondary beams
        }
        let grace = (!grace_notes.is_empty() && !empty).then(|| Beat {
            duration: Duration {
                value: 64,
                dots: 0,
                tuplet: None,
            },
            notes: grace_notes,
            effects: BeatEffects {
                grace: true,
                ..Default::default()
            },
            ..Default::default()
        });
        Ok(BeatRead {
            beat: (!empty).then_some(beat),
            grace,
            tempo,
        })
    }

    fn read_duration(&mut self, flags: u8) -> Result<Duration, ParseError> {
        let value = match self.r.i8()? {
            d @ -2..=4 => 1u8 << (d + 2),
            _ => 4,
        };
        let mut duration = Duration {
            value,
            dots: u8::from(flags & 0x01 != 0),
            tuplet: None,
        };
        if flags & 0x20 != 0 {
            duration.tuplet = match self.r.i32()? {
                3 => Some((3, 2)),
                n @ 5..=7 => Some((n as u8, 4)),
                n @ 9..=13 => Some((n as u8, 8)),
                _ => None,
            };
        }
        Ok(duration)
    }

    /// GP5 chord diagram (always the "new" 107-byte format). Only the name is kept.
    fn read_chord(&mut self) -> Result<Option<String>, ParseError> {
        let r = &mut self.r;
        r.skip(17)?; // format flag, sharp, padding, root, type, extension, bass, tonality, add
        let name = r.byte_size_string(21)?;
        r.skip(4)?; // fifth, ninth, eleventh, padding
        r.skip(4)?; // first fret
        r.skip(28)?; // 7 string frets
        r.skip(1 + 5)?; // barre count, barre frets
        r.skip(26)?; // barre starts/ends, omissions, fingerings, show
        Ok((!name.is_empty()).then_some(name))
    }

    /// Returns whether the beat has vibrato.
    fn read_beat_effects(&mut self, effects: &mut BeatEffects) -> Result<bool, ParseError> {
        let flags1 = self.r.u8()?;
        let flags2 = self.r.u8()?;
        effects.fade_in = flags1 & 0x10 != 0;
        if flags1 & 0x20 != 0 {
            match self.r.i8()? {
                1 => effects.tapping = true,
                2 => effects.slap = true,
                3 => effects.pop = true,
                _ => {}
            }
        }
        if flags2 & 0x04 != 0 {
            effects.whammy = self.read_bend()?;
        }
        if flags1 & 0x40 != 0 {
            let up = self.r.u8()?;
            let down = self.r.u8()?;
            effects.stroke = if up > 0 {
                Some(Stroke::Up)
            } else if down > 0 {
                Some(Stroke::Down)
            } else {
                None
            };
        }
        if flags2 & 0x02 != 0 {
            self.r.skip(1)?; // pick stroke
        }
        Ok(flags1 & 0x02 != 0)
    }

    /// Bend or tremolo-bar points. `None` unless there are at least two points.
    fn read_bend(&mut self) -> Result<Option<Bend>, ParseError> {
        self.r.skip(1)?; // type
        self.r.skip(4)?; // value
        let count = self.r.count(MAX_BEND_POINTS, "bend point count")?;
        let mut points = Vec::with_capacity(count);
        for _ in 0..count {
            let position = self.r.i32()?;
            let value = self.r.i32()?;
            self.r.skip(1)?; // vibrato
            points.push((
                (position as f32 / 60.0).clamp(0.0, 1.0),
                value as f32 / 50.0,
            ));
        }
        Ok((points.len() >= 2).then_some(Bend { points }))
    }

    /// Returns the tempo, if the change sets one.
    fn read_mix_table_change(&mut self) -> Result<Option<i32>, ParseError> {
        let r = &mut self.r;
        r.skip(1)?; // instrument
        r.skip(16)?; // RSE instrument
        let values = r.bytes(6)?; // volume, balance, chorus, reverb, phaser, tremolo
        let set = values.iter().filter(|&&v| (v as i8) >= 0).count();
        r.int_byte_size_string()?; // tempo name
        let tempo = r.i32()?;
        r.skip(set)?; // transition durations
        if tempo >= 0 {
            r.skip(1)?; // tempo transition duration
            if self.version >= 510 {
                r.skip(1)?; // hide tempo
            }
        }
        r.skip(1)?; // apply-to-all-tracks flags
        r.skip(1)?; // wah
        if self.version >= 510 {
            r.int_byte_size_string()?; // RSE effect
            r.int_byte_size_string()?; // RSE effect category
        }
        Ok((tempo >= 0).then_some(tempo))
    }

    fn read_note(&mut self, string: u8) -> Result<NoteRead, ParseError> {
        let flags = self.r.u8()?;
        let mut note = Note {
            string,
            velocity: DEFAULT_VELOCITY,
            ..Default::default()
        };
        note.effects.heavy_accent = flags & 0x02 != 0;
        note.effects.accent = flags & 0x40 != 0;
        note.effects.ghost = flags & 0x04 != 0;
        let has_fret = flags & 0x20 != 0;
        if has_fret {
            match self.r.u8()? {
                2 => note.tie_destination = true,
                3 => note.effects.dead = true,
                _ => {}
            }
        }
        if flags & 0x10 != 0 {
            note.velocity = velocity(self.r.i8()?);
        }
        if has_fret {
            let fret = self.r.i8()?;
            if fret < 0 {
                note.effects.dead = true;
            } else {
                note.fret = fret as u8;
            }
        }
        if flags & 0x80 != 0 {
            self.r.skip(2)?; // fingering
        }
        if flags & 0x01 != 0 {
            self.r.skip(8)?; // duration percent (f64)
        }
        self.r.skip(1)?; // flags2: swap accidentals
        let mut read = NoteRead {
            note: None,
            grace: None,
            tremolo: None,
        };
        if flags & 0x08 != 0 {
            self.read_note_effects(&mut note, &mut read)?;
        }
        read.note = has_fret.then_some(note);
        Ok(read)
    }

    /// Reads note effects into `note`, and its grace note / tremolo picking into `read`.
    fn read_note_effects(
        &mut self,
        note: &mut Note,
        read: &mut NoteRead,
    ) -> Result<(), ParseError> {
        let flags1 = self.r.u8()?;
        let flags2 = self.r.u8()?;
        let fx = &mut note.effects;
        if flags1 & 0x01 != 0 {
            fx.bend = self.read_bend()?;
        }
        if flags1 & 0x10 != 0 {
            let r = &mut self.r;
            let fret = r.i8()?;
            let dynamic = r.i8()?;
            let transition = r.i8()?;
            r.skip(1)?; // duration
            let grace_flags = r.u8()?; // 0x01 dead, 0x02 on the beat
            let mut g = Note {
                string: note.string,
                fret: fret.max(0) as u8,
                velocity: velocity(dynamic),
                ..Default::default()
            };
            g.effects.dead = fret < 0 || grace_flags & 0x01 != 0;
            match transition {
                1 => g.effects.slide = Some(Slide::Legato),
                3 => g.effects.hammer_pull = true,
                _ => {}
            }
            read.grace = Some(g);
        }
        let fx = &mut note.effects;
        if flags2 & 0x04 != 0 {
            // Tremolo picking is a beat effect in the model.
            read.tremolo = match self.r.u8()? {
                1 => Some(8),
                2 => Some(16),
                3 => Some(32),
                _ => None,
            };
        }
        if flags2 & 0x08 != 0 {
            let slides = self.r.u8()?;
            fx.slide = [
                (0x01, Slide::Shift),
                (0x02, Slide::Legato),
                (0x04, Slide::OutDown),
                (0x08, Slide::OutUp),
                (0x10, Slide::InFromBelow),
                (0x20, Slide::InFromAbove),
            ]
            .into_iter()
            .find(|&(bit, _)| slides & bit != 0)
            .map(|(_, s)| s);
        }
        if flags2 & 0x10 != 0 {
            fx.harmonic = match self.r.u8()? {
                1 => Some(Harmonic::Natural),
                2 => {
                    self.r.skip(3)?; // pitch class, accidental, octave
                    Some(Harmonic::Artificial)
                }
                3 => {
                    self.r.skip(1)?; // tapped fret
                    Some(Harmonic::Tapped)
                }
                4 => Some(Harmonic::Pinch),
                5 => Some(Harmonic::Semi),
                _ => None,
            };
        }
        if flags2 & 0x20 != 0 {
            let fret = self.r.u8()?;
            self.r.skip(1)?; // period
            fx.trill = Some(fret);
        }
        fx.let_ring = flags1 & 0x08 != 0;
        fx.hammer_pull = flags1 & 0x02 != 0;
        fx.vibrato = flags2 & 0x40 != 0;
        fx.palm_mute = flags2 & 0x02 != 0;
        fx.staccato = flags2 & 0x01 != 0;
        Ok(())
    }
}

/// GP dynamics 1 (ppp) … 8 (fff) → MIDI velocity 15 … 127.
fn velocity(dynamic: i8) -> u8 {
    (i32::from(dynamic) * 16 - 1).clamp(1, 127) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const BASICS: &[u8] = include_bytes!("../../testdata/gp5/basics.gp5");
    const V500: &[u8] = include_bytes!("../../testdata/gp5/v500.gp5");

    #[test]
    fn reader_primitives() {
        let mut r = Reader::new(&[0xFF, 0x34, 0x12, 0x78, 0x56, 0x34, 0x12, 0x01]);
        assert_eq!(r.i8().unwrap(), -1);
        assert_eq!(r.i16().unwrap(), 0x1234);
        assert_eq!(r.i32().unwrap(), 0x1234_5678);
        assert_eq!(r.u8().unwrap(), 1);
        assert!(matches!(r.u8(), Err(ParseError::Eof)));
        let mut r = Reader::new(&[1, 2, 3]);
        assert!(matches!(r.i32(), Err(ParseError::Eof)));
        assert!(matches!(r.skip(usize::MAX), Err(ParseError::Eof)));
        assert_eq!(r.u8().unwrap(), 1, "failed reads don't consume");
    }

    #[test]
    fn reader_strings() {
        // Byte-size string in a fixed 5-byte field.
        let mut r = Reader::new(b"\x03abcde!");
        assert_eq!(r.byte_size_string(5).unwrap(), "abc");
        assert_eq!(r.u8().unwrap(), b'!');
        // Length byte larger than the field is clamped.
        assert_eq!(Reader::new(b"\xffab").byte_size_string(2).unwrap(), "ab");
        // Int-byte-size string: total (len + 1), len, text.
        let mut r = Reader::new(b"\x04\x00\x00\x00\x03Hey!");
        assert_eq!(r.int_byte_size_string().unwrap(), "Hey");
        assert_eq!(r.u8().unwrap(), b'!');
        // Bogus total falls back to the length byte.
        let mut r = Reader::new(b"\x00\x00\x00\x00\x02hi");
        assert_eq!(r.int_byte_size_string().unwrap(), "hi");
        // Huge lengths fail without allocating.
        let mut r = Reader::new(b"\xff\xff\xff\x7f\x05abc");
        assert!(matches!(r.int_byte_size_string(), Err(ParseError::Eof)));
        let mut r = Reader::new(b"\xff\xff\xff\x7f");
        assert!(matches!(r.int_size_string(), Err(ParseError::Eof)));
        let mut r = Reader::new(b"\x05\x00\x00\x00");
        assert!(matches!(r.count(4, "x"), Err(ParseError::Invalid(_))));
        let mut r = Reader::new(b"\xff\xff\xff\xff");
        assert!(matches!(r.count(4, "x"), Err(ParseError::Invalid(_))));
    }

    #[test]
    fn string_decoding() {
        assert_eq!(decode_string(b"plain"), "plain");
        assert_eq!(decode_string("Beyoncé".as_bytes()), "Beyoncé");
        assert_eq!(decode_string(b"Caf\xe9 \x80\x99"), "Café €™");
    }

    #[test]
    fn velocities() {
        assert_eq!(velocity(1), 15);
        assert_eq!(velocity(6), 95);
        assert_eq!(velocity(8), 127);
        assert_eq!(velocity(-100), 1);
        assert_eq!(velocity(127), 127);
    }

    fn check_alignment(score: &Score) {
        for track in &score.tracks {
            assert_eq!(track.bars.len(), score.master_bars.len(), "{}", track.name);
        }
    }

    #[test]
    fn basics_fixture() {
        let score = parse(BASICS).unwrap();
        check_alignment(&score);
        assert_eq!(score.title, "Tabster Basics");
        assert_eq!(score.artist, "Tabster");
        assert_eq!(score.tab_author, "generate.py");
        assert_eq!(score.initial_tempo, 120.0);
        assert_eq!(score.master_bars.len(), 7);

        let mb = &score.master_bars;
        assert!(mb[0].repeat_start);
        assert_eq!(mb[0].section.as_ref().unwrap().text, "Intro");
        assert_eq!((mb[2].repeat_end, mb[2].alternate_endings), (2, 0b01));
        assert_eq!((mb[3].repeat_end, mb[3].alternate_endings), (0, 0b10));
        assert!(mb[3].double_bar);
        assert_eq!(
            mb[4].time_signature,
            TimeSignature {
                numerator: 6,
                denominator: 8
            }
        );
        assert_eq!(mb[4].key_signature, 1);
        assert_eq!(mb[0].key_signature, 0);
        assert_eq!(mb[4].directions, [Direction::Segno]);
        assert_eq!(mb[5].directions, [Direction::DalSegnoAlFine]);
        assert_eq!(mb[6].directions, [Direction::Fine]);
        assert_eq!(mb[5].triplet_feel, TripletFeel::Triplet8th);

        assert_eq!(
            score.tempo_changes,
            [TempoChange {
                bar: 4,
                position: 0.5,
                bpm: 90.0
            }]
        );

        let [gtr, bass] = &score.tracks[..] else {
            panic!("two tracks")
        };
        assert_eq!(gtr.name, "Guitar");
        assert_eq!(gtr.tuning, [64, 59, 55, 50, 45, 40]);
        assert_eq!(gtr.capo, 2);
        assert_eq!(gtr.program, 29);
        assert!(!gtr.is_percussion);
        assert_eq!(bass.tuning, [43, 38, 33, 28]);
        assert_eq!(bass.program, 33);

        // Bar 1: hammer-on, legato slide, text.
        let b1 = &gtr.bars[0].voices[0].beats;
        assert_eq!(b1[0].text.as_deref(), Some("hammer"));
        assert!(b1[0].notes[0].effects.hammer_pull);
        assert_eq!(b1[1].notes[0].effects.slide, Some(Slide::Legato));
        assert_eq!(b1[3].notes.len(), 3);
        // Bar 2: triplets, palm mute, dead note, dotted, rest, dynamics.
        let b2 = &gtr.bars[1].voices[0].beats;
        assert_eq!(b2[0].duration.tuplet, Some((3, 2)));
        assert!(b2[0].notes[0].effects.palm_mute);
        assert!(b2[3].notes[0].effects.dead);
        assert_eq!(b2[3].duration.dots, 1);
        assert!(b2[4].is_rest());
        assert_eq!(b2[5].notes[0].velocity, 47);
        assert_eq!(gtr.bars[1].ticks(), 3840);
        // Tie keeps the fret of the note it continues.
        let tied = &bass.bars[1].voices[0].beats[1].notes[0];
        assert!(tied.tie_destination);
        assert_eq!(tied.fret, 3);
        // Bar 3: a full bend (one tone = 2 semitones).
        let bend = gtr.bars[2].voices[0].beats[0].notes[0].effects.bend.clone();
        assert_eq!(bend.unwrap().points, [(0.0, 0.0), (0.5, 2.0), (1.0, 2.0)]);
        // Bar 4: harmonics, grace beat, trill.
        let b4 = &gtr.bars[3].voices[0].beats;
        assert_eq!(b4[0].notes[0].effects.harmonic, Some(Harmonic::Natural));
        assert_eq!(b4[1].notes[0].effects.harmonic, Some(Harmonic::Pinch));
        assert!(b4[2].effects.grace);
        assert_eq!(b4[2].duration.value, 64);
        assert_eq!((b4[2].notes[0].fret, b4[2].notes[0].string), (5, 1));
        assert!(b4[2].notes[0].effects.hammer_pull);
        assert!(!b4[3].effects.grace);
        assert_eq!(b4[4].notes[0].effects.trill, Some(9));
        // Bar 5: two voices, accents, slap/pop.
        assert_eq!(gtr.bars[4].voices.len(), 2);
        assert_eq!(gtr.bars[4].voices[1].beats.len(), 4);
        let fx = &gtr.bars[4].voices[0].beats[0].notes[0].effects;
        assert!(fx.accent && fx.ghost);
        let fx = &gtr.bars[4].voices[0].beats[1].notes[0].effects;
        assert!(fx.heavy_accent && fx.staccato);
        assert!(bass.bars[4].voices[0].beats[0].effects.slap);
        assert!(bass.bars[4].voices[0].beats[1].effects.pop);
        // Bar 6: strokes, tapping, fade in, whammy, tremolo picking, slide in.
        let b6 = &gtr.bars[5].voices[0].beats;
        assert_eq!(b6[0].effects.stroke, Some(Stroke::Down));
        assert_eq!(b6[1].effects.stroke, Some(Stroke::Up));
        assert!(b6[2].effects.tapping);
        assert!(b6[3].effects.fade_in);
        assert_eq!(b6[4].effects.tremolo_picking, Some(32));
        let whammy = b6[4].effects.whammy.clone().unwrap();
        assert_eq!(whammy.points, [(0.0, 0.0), (0.5, -2.0), (1.0, 0.0)]);
        assert_eq!(
            bass.bars[5].voices[0].beats[0].notes[0].effects.slide,
            Some(Slide::InFromBelow)
        );
        // Bar 7: slide out; the empty placeholder beat in voice 2 is dropped.
        assert_eq!(
            gtr.bars[6].voices[0].beats[0].notes[0].effects.slide,
            Some(Slide::OutDown)
        );
        assert_eq!(gtr.bars[6].voices.len(), 1);

        insta::assert_debug_snapshot!("gp5_basics", score);
    }

    #[test]
    fn v500_fixture() {
        let score = parse(V500).unwrap();
        check_alignment(&score);
        assert_eq!(score.title, "Tabster v5.00");
        assert_eq!(score.initial_tempo, 100.0);
        assert_eq!(score.master_bars.len(), 2);
        assert_eq!(score.master_bars[0].time_signature.numerator, 3);
        assert_eq!(
            score.tempo_changes,
            [TempoChange {
                bar: 1,
                position: 1.0 / 3.0,
                bpm: 140.0
            }]
        );
        let [gtr, drums] = &score.tracks[..] else {
            panic!("two tracks")
        };
        assert_eq!(gtr.program, 27);
        assert!(!gtr.is_percussion);
        assert!(drums.is_percussion);
        let kick = &drums.bars[0].voices[0].beats[0].notes;
        assert_eq!(
            kick.iter().map(|n| (n.string, n.fret)).collect::<Vec<_>>(),
            [(1, 42), (6, 36)]
        );
        assert_eq!(
            gtr.bars[1].voices[0].beats[2].notes[0].effects.slide,
            Some(Slide::Shift)
        );
        insta::assert_debug_snapshot!("gp5_v500", score);
    }

    #[test]
    fn truncated_input_is_an_error() {
        for fixture in [BASICS, V500] {
            // The last byte (a line-break flag of the final bar) is never needed.
            for len in 0..fixture.len() - 1 {
                assert!(parse(&fixture[..len]).is_err(), "prefix of {len} bytes");
            }
        }
    }

    #[test]
    fn wrong_version_is_an_error() {
        let mut bytes = BASICS.to_vec();
        bytes[20] = b'4'; // "FICHIER GUITAR PRO v4.10"
        assert!(matches!(parse(&bytes), Err(ParseError::Invalid(_))));
        assert!(parse(b"\x05hello").is_err());
        assert!(parse(b"").is_err());
        assert!(parse(b"PK\x03\x04").is_err());
    }

    /// Length of the version field; the fuzzers keep it intact.
    const VERSION_LEN: usize = 31;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        #[test]
        fn random_bytes_after_header_never_panic(
            keep in VERSION_LEN..BASICS.len(),
            tail in proptest::collection::vec(any::<u8>(), 0..512),
        ) {
            let mut bytes = BASICS[..keep].to_vec();
            bytes.extend_from_slice(&tail);
            let _ = parse(&bytes);
        }

        #[test]
        fn corrupted_bytes_never_panic(
            edits in proptest::collection::vec((VERSION_LEN..BASICS.len(), any::<u8>()), 1..8),
        ) {
            let mut bytes = BASICS.to_vec();
            for (at, value) in edits {
                bytes[at] = value;
            }
            if let Ok(score) = parse(&bytes) {
                for track in &score.tracks {
                    prop_assert_eq!(track.bars.len(), score.master_bars.len());
                }
            }
        }
    }
}
