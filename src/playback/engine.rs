//! The playback engine: a score clock that walks the event list while a [`Sink`] (the
//! SoundFont synthesizer, or a recorder in tests) renders audio.
//!
//! Events fire at the exact frame they fall on: a render call is split into sub-blocks at every
//! event, loop end and count-in click. (rustysynth itself applies messages at its 64-frame
//! internal block boundaries, so audible timing is within ~1.5 ms.)

use super::events::{
    CLICK_ACCENT_KEY, Event, Events, METRONOME_CHANNEL, METRONOME_TRACK, MetronomeSound,
    PERCUSSION_CHANNEL, TabKit,
};

pub const SAMPLE_RATE: u32 = 44_100;
/// Frames rendered per sink call at most; the scratch buffers are this long.
const CHUNK: usize = 512;
const EPS: f64 = 1e-9;
pub const MIN_SPEED: f64 = 0.25;
pub const MAX_SPEED: f64 = 2.0;

/// Where audio is made. Implemented by `rustysynth::Synthesizer`.
pub trait Sink {
    fn note_on(&mut self, channel: u8, key: u8, velocity: u8);
    fn note_off(&mut self, channel: u8, key: u8);
    /// Stop every voice: `immediate` cuts them, otherwise they release.
    fn all_notes_off(&mut self, immediate: bool);
    fn channel_notes_off(&mut self, channel: u8);
    fn program_change(&mut self, channel: u8, program: u8);
    fn control_change(&mut self, channel: u8, controller: u8, value: u8);
    /// 14-bit pitch bend, 8192 = none.
    fn pitch_bend(&mut self, channel: u8, value: u16);
    fn set_volume(&mut self, volume: f32);
    fn render(&mut self, left: &mut [f32], right: &mut [f32]);
}

impl Sink for rustysynth::Synthesizer {
    fn note_on(&mut self, channel: u8, key: u8, velocity: u8) {
        rustysynth::Synthesizer::note_on(self, channel.into(), key.into(), velocity.into());
    }
    fn note_off(&mut self, channel: u8, key: u8) {
        rustysynth::Synthesizer::note_off(self, channel.into(), key.into());
    }
    fn all_notes_off(&mut self, immediate: bool) {
        self.note_off_all(immediate);
    }
    fn channel_notes_off(&mut self, channel: u8) {
        self.note_off_all_channel(channel.into(), false);
    }
    fn program_change(&mut self, channel: u8, program: u8) {
        self.process_midi_message(channel.into(), 0xC0, program.into(), 0);
    }
    fn control_change(&mut self, channel: u8, controller: u8, value: u8) {
        self.process_midi_message(channel.into(), 0xB0, controller.into(), value.into());
    }
    fn pitch_bend(&mut self, channel: u8, value: u16) {
        let v = i32::from(value.min(0x3FFF));
        self.process_midi_message(channel.into(), 0xE0, v & 0x7F, v >> 7);
    }
    fn set_volume(&mut self, volume: f32) {
        self.set_master_volume(volume);
    }
    fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
        rustysynth::Synthesizer::render(self, left, right);
    }
}

/// Master volume at `set_volume(1.0)`: leaves headroom for dense mixes.
const BASE_VOLUME: f32 = 0.7;
/// Metronome pitch range either way, in semitones (the channel's pitch-bend range).
pub const METRONOME_PITCH_RANGE: f32 = 12.0;
/// Loudest tab volume: 1.5× the default (CC 7 ≈ 122).
pub const MAX_TAB_VOLUME: f32 = 1.5;
/// Loudest metronome volume: 3× (≈ +10 dB) the default click.
pub const MAX_METRONOME_VOLUME: f32 = 3.0;
/// Channel volume (CC 7) of the default click and tab: MIDI's default.
const METRONOME_CC_VOLUME: f32 = 100.0;

#[derive(Debug, Clone, Copy)]
struct CountIn {
    /// Score seconds into the count-in.
    elapsed: f64,
    beat_secs: f64,
    beats_per_bar: u32,
    total_beats: u32,
    next_beat: u32,
}

impl CountIn {
    fn total_secs(&self) -> f64 {
        self.beat_secs * f64::from(self.total_beats)
    }
}

pub struct Engine<S: Sink> {
    sink: S,
    events: Events,
    /// Next note event to fire.
    cursor: usize,
    /// Next metronome click event.
    click_cursor: usize,
    /// Score seconds of the audio rendered so far.
    clock: f64,
    speed: f64,
    playing: bool,
    metronome: bool,
    metronome_sound: MetronomeSound,
    kit: TabKit,
    /// Identical voices per click: 2 doubles its amplitude, past what channel volume allows.
    metronome_layers: u8,
    /// Every track silent (only the metronome and count-in sound), e.g. over a video's audio.
    notes_muted: bool,
    muted: Vec<bool>,
    soloed: Vec<bool>,
    loop_span: Option<(f64, f64)>,
    count_in: Option<CountIn>,
    left: Vec<f32>,
    right: Vec<f32>,
}

impl<S: Sink> Engine<S> {
    pub fn new(mut sink: S, events: Events) -> Self {
        for &(channel, program) in &events.programs {
            sink.program_change(channel, program);
        }
        sink.set_volume(BASE_VOLUME);
        // The metronome channel plays the drum kit (bank 128), with a pitch-bend range of
        // ±METRONOME_PITCH_RANGE semitones (RPN 0).
        sink.control_change(METRONOME_CHANNEL, 0x00, 128);
        sink.control_change(METRONOME_CHANNEL, 0x65, 0);
        sink.control_change(METRONOME_CHANNEL, 0x64, 0);
        sink.control_change(METRONOME_CHANNEL, 0x06, METRONOME_PITCH_RANGE as u8);
        sink.control_change(METRONOME_CHANNEL, 0x26, 0);
        sink.control_change(METRONOME_CHANNEL, 0x65, 127);
        sink.control_change(METRONOME_CHANNEL, 0x64, 127);
        sink.control_change(METRONOME_CHANNEL, 0x07, METRONOME_CC_VOLUME as u8);
        let tracks = events.track_channels.len();
        Self {
            sink,
            events,
            cursor: 0,
            click_cursor: 0,
            clock: 0.0,
            speed: 1.0,
            playing: false,
            metronome: false,
            metronome_sound: MetronomeSound::default(),
            kit: TabKit::default(),
            metronome_layers: 1,
            notes_muted: false,
            muted: vec![false; tracks],
            soloed: vec![false; tracks],
            loop_span: None,
            count_in: None,
            left: vec![0.0; CHUNK],
            right: vec![0.0; CHUNK],
        }
    }

    pub fn sink(&self) -> &S {
        &self.sink
    }

    pub fn events(&self) -> &Events {
        &self.events
    }

    pub fn position_secs(&self) -> f64 {
        self.clock
    }

    pub fn duration_secs(&self) -> f64 {
        self.events.duration_secs
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }

    pub fn is_counting_in(&self) -> bool {
        self.count_in.is_some()
    }

    pub fn set_playing(&mut self, playing: bool) {
        if self.playing == playing {
            return;
        }
        self.playing = playing;
        if !playing {
            self.count_in = None;
            self.sink.all_notes_off(false);
        }
    }

    /// Jump the score clock; every sounding voice is cut.
    pub fn seek(&mut self, secs: f64) {
        let secs = if secs.is_finite() { secs } else { 0.0 };
        self.clock = secs.clamp(0.0, self.duration_secs());
        self.reposition();
        self.sink.all_notes_off(true);
    }

    fn reposition(&mut self) {
        let t = self.clock;
        self.cursor = self.events.notes.partition_point(|e| e.secs < t - EPS);
        self.click_cursor = self.events.clicks.partition_point(|e| e.secs < t - EPS);
    }

    pub fn set_speed(&mut self, speed: f64) {
        if speed.is_finite() {
            self.speed = speed.clamp(MIN_SPEED, MAX_SPEED);
        }
    }

    pub fn speed(&self) -> f64 {
        self.speed
    }

    pub fn set_volume(&mut self, volume: f32) {
        let v = if volume.is_finite() {
            volume.clamp(0.0, 2.0)
        } else {
            1.0
        };
        self.sink.set_volume(v * BASE_VOLUME);
    }

    pub fn set_metronome(&mut self, on: bool) {
        self.metronome = on;
    }

    /// Play the tab with another set of guitars, basses and drums. Sounding notes keep their
    /// instrument; the next ones use the new one.
    pub fn set_kit(&mut self, kit: TabKit) {
        if kit == self.kit {
            return;
        }
        self.kit = kit;
        for &(channel, program) in &self.events.programs {
            self.sink.program_change(channel, kit.program(program));
        }
        self.sink.program_change(PERCUSSION_CHANNEL, kit.drum_kit());
    }

    pub fn kit(&self) -> TabKit {
        self.kit
    }

    /// Loudness of the tab's notes (not the metronome) as an amplitude factor of the default,
    /// 0–[`MAX_TAB_VOLUME`]: CC 7 = 100·√v on every channel but the metronome's.
    pub fn set_tab_volume(&mut self, volume: f32) {
        let v = if volume.is_finite() {
            volume.clamp(0.0, MAX_TAB_VOLUME)
        } else {
            1.0
        };
        let cc = (METRONOME_CC_VOLUME * v.sqrt()).round().min(127.0) as u8;
        for channel in (0..16).filter(|&c| c != METRONOME_CHANNEL) {
            self.sink.control_change(channel, 0x07, cc);
        }
    }

    pub fn set_metronome_sound(&mut self, sound: MetronomeSound) {
        if sound != self.metronome_sound {
            // Its note-offs would go to the new sound's keys.
            self.sink.channel_notes_off(METRONOME_CHANNEL);
            self.metronome_sound = sound;
        }
    }

    /// Metronome (and count-in) loudness as an amplitude factor of the default click,
    /// 0–[`MAX_METRONOME_VOLUME`], independent of the tab's.
    ///
    /// Channel gain goes with the square of CC 7, so CC 7 = 100·√v; above 1× each click sounds
    /// as two identical voices (twice the amplitude) and CC 7 makes up the rest.
    pub fn set_metronome_volume(&mut self, volume: f32) {
        let v = if volume.is_finite() {
            volume.clamp(0.0, MAX_METRONOME_VOLUME)
        } else {
            1.0
        };
        self.metronome_layers = if v > 1.0 { 2 } else { 1 };
        let per_layer = v / f32::from(self.metronome_layers);
        let cc = (METRONOME_CC_VOLUME * per_layer.sqrt()).round().min(127.0);
        self.sink.control_change(METRONOME_CHANNEL, 0x07, cc as u8);
    }

    /// Shift the metronome's pitch by `semitones` (±[`METRONOME_PITCH_RANGE`]).
    pub fn set_metronome_pitch(&mut self, semitones: f32) {
        let st = if semitones.is_finite() {
            semitones.clamp(-METRONOME_PITCH_RANGE, METRONOME_PITCH_RANGE)
        } else {
            0.0
        };
        let value = (8192.0 + st / METRONOME_PITCH_RANGE * 8192.0)
            .round()
            .clamp(0.0, 16383.0);
        self.sink.pitch_bend(METRONOME_CHANNEL, value as u16);
    }

    /// Sound one accented click now, whether or not playing (to audition the settings).
    pub fn preview_click(&mut self) {
        self.click(true, 115);
    }

    fn click(&mut self, accent: bool, velocity: u8) {
        let key = self.metronome_sound.key(accent);
        self.sink.note_off(METRONOME_CHANNEL, key);
        self.click_on(key, velocity);
    }

    fn click_on(&mut self, key: u8, velocity: u8) {
        for _ in 0..self.metronome_layers {
            self.sink.note_on(METRONOME_CHANNEL, key, velocity);
        }
    }

    /// Silence every track, leaving only the metronome and count-in clicks.
    pub fn set_notes_muted(&mut self, muted: bool) {
        self.notes_muted = muted;
        self.silence_inaudible();
    }

    pub fn set_track_mute(&mut self, track: usize, muted: bool) {
        if let Some(m) = self.muted.get_mut(track) {
            *m = muted;
            self.silence_inaudible();
        }
    }

    pub fn set_track_solo(&mut self, track: usize, soloed: bool) {
        if let Some(s) = self.soloed.get_mut(track) {
            *s = soloed;
            self.silence_inaudible();
        }
    }

    fn audible(&self, track: u16) -> bool {
        let t = usize::from(track);
        let any_solo = self.soloed.iter().any(|&s| s);
        !self.notes_muted
            && !self.muted.get(t).copied().unwrap_or(false)
            && (!any_solo || self.soloed.get(t).copied().unwrap_or(false))
    }

    fn silence_inaudible(&mut self) {
        for t in 0..self.events.track_channels.len() {
            if !self.audible(t as u16) {
                let channel = self.events.track_channels[t];
                self.sink.channel_notes_off(channel);
            }
        }
    }

    /// Loop between two score times (`None` = start / end of the score). Both `None` clears it.
    pub fn set_loop(&mut self, start: Option<f64>, end: Option<f64>) {
        if start.is_none() && end.is_none() {
            self.loop_span = None;
            return;
        }
        let d = self.duration_secs();
        let s = start.filter(|v| v.is_finite()).unwrap_or(0.0).clamp(0.0, d);
        let e = end.filter(|v| v.is_finite()).unwrap_or(d).clamp(0.0, d);
        self.loop_span = (e - s > 1e-3).then_some((s, e));
    }

    /// Count in `bars` bars of clicks (in the time signature and tempo at the current position)
    /// before the score clock moves again. 0 cancels a pending count-in.
    pub fn count_in(&mut self, bars: u32) {
        if bars == 0 {
            self.count_in = None;
            return;
        }
        let bars_list = &self.events.bars;
        let i = bars_list
            .partition_point(|b| b.start_secs <= self.clock + EPS)
            .saturating_sub(1);
        let (beat_secs, beats_per_bar) = bars_list
            .get(i)
            .map_or((0.5, 4), |b| (b.beat_secs, u32::from(b.beats_per_bar)));
        if beat_secs <= 0.0 {
            return;
        }
        self.count_in = Some(CountIn {
            elapsed: 0.0,
            beat_secs,
            beats_per_bar,
            total_beats: bars.saturating_mul(beats_per_bar),
            next_beat: 0,
        });
    }

    /// Render interleaved stereo into `out` (length = 2 × frames), advancing the clock while
    /// playing. Doesn't allocate.
    pub fn render_into(&mut self, out: &mut [f32]) {
        let frames = out.len() / 2;
        let mut done = 0;
        while done < frames {
            let max = (frames - done).min(CHUNK);
            let (n, advance) = if self.playing {
                self.fire_due(max)
            } else {
                (max, false)
            };
            let (left, right) = (&mut self.left[..n], &mut self.right[..n]);
            self.sink.render(left, right);
            for (k, (l, r)) in left.iter().zip(right.iter()).enumerate() {
                out[(done + k) * 2] = *l;
                out[(done + k) * 2 + 1] = *r;
            }
            done += n;
            if advance {
                let dt = n as f64 * self.speed / f64::from(SAMPLE_RATE);
                match &mut self.count_in {
                    Some(ci) => ci.elapsed += dt,
                    None => self.clock = (self.clock + dt).min(self.duration_secs()),
                }
            }
        }
    }

    /// Fire everything due at the current time; returns how many frames (1..=max) to render
    /// before the next thing is due, and whether the clock moves during them.
    fn fire_due(&mut self, max: usize) -> (usize, bool) {
        let step = self.speed / f64::from(SAMPLE_RATE);
        let frames_until = |secs: f64| -> usize {
            // The tolerance keeps float error from pushing an event a frame late.
            let f = (secs / step - 1e-6).ceil();
            if f.is_finite() && f >= 1.0 {
                (f as usize).min(max)
            } else {
                1
            }
        };

        if let Some(mut ci) = self.count_in {
            while ci.next_beat < ci.total_beats
                && f64::from(ci.next_beat) * ci.beat_secs <= ci.elapsed + EPS
            {
                let accent = ci.next_beat % ci.beats_per_bar.max(1) == 0;
                self.click(accent, if accent { 115 } else { 85 });
                ci.next_beat += 1;
            }
            if ci.elapsed + EPS < ci.total_secs() {
                let next = (f64::from(ci.next_beat) * ci.beat_secs).min(ci.total_secs());
                self.count_in = Some(ci);
                return (frames_until(next - ci.elapsed), true);
            }
            self.count_in = None;
        }

        if let Some((start, end)) = self.loop_span
            && self.clock + EPS >= end
        {
            self.clock = start;
            self.reposition();
            self.sink.all_notes_off(false);
        }

        let t = self.clock;
        while let Some(&e) = self.events.notes.get(self.cursor) {
            if e.secs > t + EPS {
                break;
            }
            self.cursor += 1;
            self.fire(e);
        }
        while let Some(&e) = self.events.clicks.get(self.click_cursor) {
            if e.secs > t + EPS {
                break;
            }
            self.click_cursor += 1;
            if self.metronome {
                self.fire(e);
            }
        }

        let duration = self.duration_secs();
        if t >= duration {
            return (max, false);
        }
        let mut next = duration;
        if let Some(e) = self.events.notes.get(self.cursor) {
            next = next.min(e.secs);
        }
        if let Some(e) = self.events.clicks.get(self.click_cursor) {
            next = next.min(e.secs);
        }
        if let Some((_, end)) = self.loop_span {
            next = next.min(end);
        }
        (frames_until(next - t), true)
    }

    fn fire(&mut self, e: Event) {
        if e.track == METRONOME_TRACK {
            let key = self.metronome_sound.key(e.key == CLICK_ACCENT_KEY);
            if e.is_on() {
                self.click_on(key, e.velocity);
            } else {
                self.sink.note_off(e.channel, key);
            }
            return;
        }
        if e.is_on() {
            if self.audible(e.track) {
                self.sink.note_on(e.channel, e.key, e.velocity);
            }
        } else {
            self.sink.note_off(e.channel, e.key);
        }
    }
}
