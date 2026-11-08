//! Tab audio playback ("hear the tab"): the score is turned into MIDI note events along the
//! played timeline ([`events`]) and rendered by a SoundFont synthesizer (`rustysynth`) driven
//! by a sample-accurate score clock ([`engine`]).
//!
//! The shell pulls audio with [`TabSynth::render`] from its audio thread and may use
//! [`TabSynth::position_secs`] as the playback clock when there's no video. Everything works
//! without a SoundFont except creating a `TabSynth`, which then fails (the feature is
//! unavailable).

pub mod engine;
pub mod events;
#[cfg(test)]
mod tests;

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use rustysynth::{SoundFont, Synthesizer, SynthesizerSettings};

use crate::model::Score;
use crate::timeline::Timeline;
pub use engine::{Engine, SAMPLE_RATE, Sink};

#[derive(Debug, thiserror::Error)]
pub enum SynthError {
    #[error("SoundFont not found: {0}")]
    NotFound(PathBuf),
    #[error("couldn't load the SoundFont: {0}")]
    Invalid(String),
}

/// The last SoundFont loaded, so reopening tabs doesn't re-read ~30 MB.
static SOUNDFONT_CACHE: Mutex<Option<(PathBuf, Arc<SoundFont>)>> = Mutex::new(None);

fn load_soundfont(path: &Path) -> Result<Arc<SoundFont>, SynthError> {
    let mut cache = SOUNDFONT_CACHE.lock();
    if let Some((p, sf)) = cache.as_ref()
        && p == path
    {
        return Ok(Arc::clone(sf));
    }
    let file = File::open(path).map_err(|_| SynthError::NotFound(path.to_path_buf()))?;
    let sf = SoundFont::new(&mut BufReader::new(file))
        .map_err(|e| SynthError::Invalid(e.to_string()))?;
    let sf = Arc::new(sf);
    *cache = Some((path.to_path_buf(), Arc::clone(&sf)));
    Ok(sf)
}

/// A tab's audio: one synthesizer plus the score clock. Thread-safe (one lock); `render` is
/// meant to be called from a real-time audio thread and doesn't allocate beyond its result.
#[derive(uniffi::Object)]
pub struct TabSynth {
    engine: Mutex<Engine<Synthesizer>>,
}

impl TabSynth {
    pub fn new(score: &Score, timeline: &Timeline, soundfont: &Path) -> Result<Self, SynthError> {
        let sf = load_soundfont(soundfont)?;
        let mut settings = SynthesizerSettings::new(SAMPLE_RATE as i32);
        settings.maximum_polyphony = 128;
        let synth =
            Synthesizer::new(&sf, &settings).map_err(|e| SynthError::Invalid(e.to_string()))?;
        Ok(Self {
            engine: Mutex::new(Engine::new(synth, events::build(score, timeline))),
        })
    }

    /// Render interleaved stereo into a caller buffer (2 × frames samples). No allocation.
    pub fn render_into(&self, out: &mut [f32]) {
        self.engine.lock().render_into(out);
    }
}

#[uniffi::export]
impl TabSynth {
    /// Output sample rate (Hz) of `render`.
    pub fn sample_rate(&self) -> u32 {
        SAMPLE_RATE
    }

    /// Start or pause the score clock. Pausing releases sounding notes and cancels a count-in.
    pub fn set_playing(&self, playing: bool) {
        self.engine.lock().set_playing(playing);
    }

    pub fn is_playing(&self) -> bool {
        self.engine.lock().is_playing()
    }

    /// True while a count-in is clicking (the score clock is held).
    pub fn is_counting_in(&self) -> bool {
        self.engine.lock().is_counting_in()
    }

    /// Jump to a score time; cuts every sounding voice.
    pub fn seek(&self, score_secs: f64) {
        self.engine.lock().seek(score_secs);
    }

    /// Playback rate, clamped to 0.25–2.0.
    pub fn set_speed(&self, speed: f64) {
        self.engine.lock().set_speed(speed);
    }

    pub fn set_track_mute(&self, track: u32, muted: bool) {
        self.engine.lock().set_track_mute(track as usize, muted);
    }

    /// While any track is soloed, only soloed tracks sound.
    pub fn set_track_solo(&self, track: u32, soloed: bool) {
        self.engine.lock().set_track_solo(track as usize, soloed);
    }

    /// Output gain, 1.0 = normal (0–2).
    pub fn set_volume(&self, volume: f32) {
        self.engine.lock().set_volume(volume);
    }

    /// Click on every beat of the time signature (accent on beat 1).
    pub fn set_metronome(&self, enabled: bool) {
        self.engine.lock().set_metronome(enabled);
    }

    /// Click `bars` bars (time signature and tempo at the current position) before the score
    /// clock moves again; call just before `set_playing(true)`. 0 cancels.
    pub fn count_in(&self, bars: u32) {
        self.engine.lock().count_in(bars);
    }

    /// Loop between two score times (`None` = start / end of the score); both `None` clears.
    /// Wraps sample-accurately inside `render`.
    pub fn set_loop(&self, start_secs: Option<f64>, end_secs: Option<f64>) {
        self.engine.lock().set_loop(start_secs, end_secs);
    }

    /// Render `frames` frames of interleaved stereo at 44.1 kHz, advancing the score clock by
    /// frames / 44100 × speed while playing.
    pub fn render(&self, frames: u32) -> Vec<f32> {
        let mut out = vec![0.0; frames as usize * 2];
        self.render_into(&mut out);
        out
    }

    /// The synth's score clock: score seconds at the end of the audio rendered so far.
    pub fn position_secs(&self) -> f64 {
        self.engine.lock().position_secs()
    }

    pub fn duration_secs(&self) -> f64 {
        self.engine.lock().duration_secs()
    }
}
