//! Automatic sync from audio (spike): listen to the video once, work out where every bar is.
//!
//! The shell taps the system audio while the video plays and streams it into an
//! [`AudioCapture`], which keeps only alignment features ([`features`]). [`sync_from_capture`]
//! then predicts the same features from the tab's note events ([`reference`]), aligns the two
//! ([`align`]) and returns per-bar [`SyncSpec::Points`].
//!
//! Limits of the spike: one uninterrupted capture at normal speed; thresholds are tuned on
//! synthetic recordings (see `tests`) and a handful of real ones.

pub mod align;
pub mod features;
pub mod reference;
#[cfg(test)]
mod tests;

use parking_lot::Mutex;

use crate::model::Score;
use crate::playback::events;
use crate::sync::{SyncPoint, SyncSpec};
use crate::timeline::Timeline;
use align::AlignError;
use features::{Extractor, Features};

/// Least capture worth aligning (seconds).
const MIN_CAPTURE_SECS: f64 = 15.0;
/// A capture quieter than this (RMS, full scale = 1) is silence: nothing was playing, or the
/// system refused the tap and handed over zeros.
const SILENCE_RMS: f64 = 1e-4;
/// How far the capture's and the video's clocks may disagree before the capture is rejected
/// (the video was paused, seeked or changed speed while listening).
const MAX_CLOCK_SPREAD_SECS: f64 = 0.3;
/// How a path's chroma similarity becomes the "alike" part of the confidence: nothing at the
/// floor, then approaching 1 without reaching it, so that good and better stay apart. Correct
/// alignments of 19 real recordings score 0.39…0.73 (→ about 0.6…0.93); synthetic ones 0.7.
const ALIKE_FLOOR: f64 = 0.2;
const ALIKE_SCALE: f64 = 0.2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum AudioSyncError {
    #[error("not enough audio was captured")]
    TooShort,
    #[error("the capture is silent")]
    Silent,
    #[error("the video was paused or seeked while listening")]
    UnsteadyClock,
    #[error("the capture is too long to align")]
    TooLong,
    #[error("the capture doesn't sound like the tab")]
    NoMatch,
}

/// What [`sync_from_capture`] found.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct AudioSync {
    /// One point per played bar.
    pub sync: SyncSpec,
    /// 0…1: how alike the capture and the tab are along the alignment, and how many of the
    /// bars the capture should hold had their onsets found. A rough "trust this?" hint; never
    /// quite 1.
    pub confidence: f64,
    /// Mean chroma similarity along the alignment (−1…1; unrelated audio scores near 0).
    pub similarity: f64,
    /// The played bars the capture was matched against; the rest continue at the same tempo.
    pub first_bar: u32,
    pub last_bar: u32,
    /// Bars in that range whose onsets were found in the capture.
    pub matched_bars: u32,
    /// Semitones the recording sounds above the tab.
    pub transpose: i8,
    /// Video seconds per score second over the aligned range.
    pub stretch: f64,
}

struct CaptureState {
    extractor: Extractor,
    /// Per pushed chunk with a known video time: video seconds minus capture seconds.
    clock_offsets: Vec<f64>,
    /// Every sample pushed, when the capture was made with [`AudioCapture::recording`].
    recording: Option<Vec<f32>>,
}

/// System audio captured while a video plays, reduced to alignment features as it arrives.
/// Thread-safe.
#[derive(uniffi::Object)]
pub struct AudioCapture {
    state: Mutex<CaptureState>,
}

#[uniffi::export]
impl AudioCapture {
    /// A capture of mono audio at `sample_rate` Hz.
    #[uniffi::constructor]
    pub fn new(sample_rate: f64) -> Self {
        Self {
            state: Mutex::new(CaptureState {
                extractor: Extractor::new(sample_rate),
                clock_offsets: Vec::new(),
                recording: None,
            }),
        }
    }

    /// A capture that also keeps the audio itself, for [`save_debug`](Self::save_debug).
    /// Debugging only: a normal capture never holds on to what it hears.
    #[uniffi::constructor]
    pub fn recording(sample_rate: f64) -> Self {
        let capture = Self::new(sample_rate);
        capture.state.lock().recording = Some(Vec::new());
        capture
    }

    /// Write what a [`recording`](Self::recording) capture heard to `<stem>.wav` (16-bit mono)
    /// and its clock data to `<stem>.json`, so a sync can be reproduced with the `audio_sync`
    /// example. `tab_path` and `video` are only noted in the JSON. False if nothing was written.
    pub fn save_debug(&self, stem: String, tab_path: String, video: String) -> bool {
        let state = self.state.lock();
        let Some(samples) = &state.recording else {
            return false;
        };
        let rate = state.extractor.sample_rate() as u32;
        let data_len = (samples.len() * 2) as u32;
        let mut wav = Vec::with_capacity(44 + samples.len() * 2);
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_len).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&1u16.to_le_bytes()); // mono
        wav.extend_from_slice(&rate.to_le_bytes());
        wav.extend_from_slice(&(rate * 2).to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());
        for s in samples {
            let v = (s.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16;
            wav.extend_from_slice(&v.to_le_bytes());
        }
        let mut sorted = state.clock_offsets.clone();
        sorted.sort_by(f64::total_cmp);
        let notes = serde_json::json!({
            "tab": tab_path,
            "video": video,
            "sample_rate": rate,
            "seconds": state.extractor.duration_secs(),
            "rms": state.extractor.rms(),
            "clock_origin": sorted.get(sorted.len() / 2),
            "clock_offsets": state.clock_offsets,
        });
        std::fs::write(format!("{stem}.wav"), wav).is_ok()
            && std::fs::write(format!("{stem}.json"), notes.to_string()).is_ok()
    }

    /// Append mono samples: `pcm` is native-endian `f32`s (a trailing partial sample is
    /// ignored). `video_secs` is the video time at which the chunk's first sample was heard,
    /// if known.
    pub fn push(&self, pcm: Vec<u8>, video_secs: Option<f64>) {
        let samples: Vec<f32> = pcm
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_ne_bytes(*b))
            .collect();
        self.push_samples(&samples, video_secs);
    }

    /// Seconds captured so far.
    pub fn duration_secs(&self) -> f64 {
        self.state.lock().extractor.duration_secs()
    }

    /// RMS level of everything captured so far (full scale = 1).
    pub fn level(&self) -> f64 {
        self.state.lock().extractor.rms()
    }
}

impl AudioCapture {
    pub fn push_samples(&self, samples: &[f32], video_secs: Option<f64>) {
        let mut state = self.state.lock();
        if let Some(video) = video_secs.filter(|v| v.is_finite()) {
            let at = state.extractor.duration_secs();
            state.clock_offsets.push(video - at);
        }
        if let Some(recording) = &mut state.recording {
            recording.extend_from_slice(samples);
        }
        state.extractor.push(samples);
    }

    /// The features so far, and the video time of the capture's first sample (`None` when no
    /// chunk carried a video time).
    fn snapshot(&self) -> Result<(Features, Option<f64>), AudioSyncError> {
        let state = self.state.lock();
        if state.extractor.duration_secs() < MIN_CAPTURE_SECS {
            return Err(AudioSyncError::TooShort);
        }
        if state.extractor.rms() < SILENCE_RMS {
            return Err(AudioSyncError::Silent);
        }
        let mut offsets = state.clock_offsets.clone();
        let origin = if offsets.is_empty() {
            None
        } else {
            offsets.sort_by(f64::total_cmp);
            let median = offsets[offsets.len() / 2];
            // Tolerate the odd late timestamp, not a clock that moved.
            let mut spread: Vec<f64> = offsets.iter().map(|o| (o - median).abs()).collect();
            spread.sort_by(f64::total_cmp);
            if spread[spread.len() * 9 / 10] > MAX_CLOCK_SPREAD_SECS {
                return Err(AudioSyncError::UnsteadyClock);
            }
            Some(median)
        };
        Ok((state.extractor.features().clone(), origin))
    }
}

/// Align `capture` to the score and return a sync for the video it was captured from.
pub fn sync_from_capture(
    score: &Score,
    timeline: &Timeline,
    capture: &AudioCapture,
) -> Result<AudioSync, AudioSyncError> {
    let (audio, origin) = capture.snapshot()?;
    let reference = reference::reference(&events::build(score, timeline));
    let bars = timeline.played_bars();
    let starts: Vec<f64> = bars.iter().map(|b| b.start_secs).collect();
    let found =
        align::align(&reference, &audio, &starts, origin.map(|o| -o)).map_err(|e| match e {
            AlignError::TooLong => AudioSyncError::TooLong,
            AlignError::NoMatch => AudioSyncError::NoMatch,
        })?;

    // The bars the capture should hold: those the sync puts inside it, whether or not the
    // alignment reached them. A full listen that only lined up with the first verse hasn't
    // tracked the tab, however well that verse fits. Not the rests a tab opens or ends with:
    // there is nothing in them to hear.
    let listened = audio.onset.len() as f64 / features::ONSET_RATE;
    let note_at = |frame: usize| frame as f64 / features::ONSET_RATE;
    let first_note = reference.onset.iter().position(|v| *v > 0.0).map(note_at);
    let last_note = reference.onset.iter().rposition(|v| *v > 0.0).map(note_at);
    let expected = bars
        .iter()
        .zip(&found.bar_times)
        .filter(|(bar, t)| {
            (0.0..=listened).contains(*t)
                && first_note.is_some_and(|first| bar.end_secs > first)
                && last_note.is_some_and(|last| bar.start_secs <= last)
        })
        .count();
    let confidence = confidence(found.similarity, found.matched_bars, expected);

    let points = found
        .bar_times
        .iter()
        .enumerate()
        .map(|(k, &t)| SyncPoint {
            played_bar: k as u32,
            video_secs: origin.unwrap_or(0.0) + t,
            heard: found.heard[k],
        })
        .collect();
    Ok(AudioSync {
        sync: SyncSpec::Points { points },
        confidence,
        similarity: found.similarity,
        first_bar: found.first_bar as u32,
        last_bar: found.last_bar as u32,
        matched_bars: found.matched_bars as u32,
        transpose: found.transpose,
        stretch: found.stretch,
    })
}

/// The confidence of an alignment with chroma `similarity` in which `matched` of the `expected`
/// bars were found by their onsets.
fn confidence(similarity: f64, matched: usize, expected: usize) -> f64 {
    let matched = (matched as f64 / expected.max(1) as f64).min(1.0);
    let alike = 1.0 - (-(similarity - ALIKE_FLOOR).max(0.0) / ALIKE_SCALE).exp();
    alike * (0.5 + 0.5 * matched)
}
