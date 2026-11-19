//! Audio → alignment features: an onset-strength envelope and a chromagram.
//!
//! Both are computed from short-time spectra of mono PCM, streamed in as it's captured so a whole
//! song never has to be held as samples. Frame `k` of each feature is centred on time
//! `k / rate` seconds from the first sample.

use std::sync::Arc;

use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};

/// Onset envelope frames per second.
pub const ONSET_RATE: f64 = 100.0;
/// Chroma frames per second.
pub const CHROMA_RATE: f64 = 10.0;
/// Onset frames per chroma frame.
const ONSETS_PER_CHROMA: usize = 10;

/// Chroma is read from this band: below it an FFT bin is wider than a semitone, above it
/// partials stop saying much about the notes played.
const CHROMA_MIN_HZ: f64 = 130.0;
const CHROMA_MAX_HZ: f64 = 2100.0;
/// The bass register gets its own chroma, from a longer window (semitones are a few Hz apart
/// down there): a bass line is drowned out by everything else in the main band.
const BASS_MIN_HZ: f64 = 55.0;
const BASS_MAX_HZ: f64 = 260.0;
/// Spectral flux ignores everything above this (hiss, codec artefacts).
const ONSET_MAX_HZ: f64 = 10_000.0;
/// Log compression of magnitudes before taking the flux.
const ONSET_COMPRESSION: f32 = 100.0;

/// Raw features of one signal (a capture, or the tab's reference).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Features {
    /// Onset strength at [`ONSET_RATE`], non-negative.
    pub onset: Vec<f32>,
    /// Energy per pitch class (C = 0) at [`CHROMA_RATE`], non-negative.
    pub chroma: Vec<[f32; 12]>,
    /// The same for the bass register alone, frame for frame with `chroma`.
    pub bass: Vec<[f32; 12]>,
}

struct Stft {
    size: usize,
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    buf: Vec<Complex32>,
    scratch: Vec<Complex32>,
}

impl Stft {
    fn new(planner: &mut FftPlanner<f32>, size: usize) -> Self {
        let fft = planner.plan_fft_forward(size);
        // Hann, scaled so a full-scale sine reads as magnitude 1.
        let window = (0..size)
            .map(|i| {
                let phase = std::f32::consts::TAU * i as f32 / size as f32;
                (0.5 - 0.5 * phase.cos()) * 4.0 / size as f32
            })
            .collect();
        Self {
            size,
            scratch: vec![Complex32::default(); fft.get_inplace_scratch_len()],
            fft,
            window,
            buf: vec![Complex32::default(); size],
        }
    }

    /// Transform `samples` (exactly `size` of them); the spectrum is left in `buf`.
    fn run(&mut self, samples: &[f32]) {
        for ((out, &s), &w) in self.buf.iter_mut().zip(samples).zip(&self.window) {
            *out = Complex32::new(s * w, 0.0);
        }
        self.fft
            .process_with_scratch(&mut self.buf, &mut self.scratch);
    }
}

/// Streaming feature extractor for mono PCM at a fixed sample rate.
pub struct Extractor {
    sample_rate: f64,
    onset: Stft,
    chroma: Stft,
    bass: Stft,
    /// `(bin, pitch class)` for every chroma bin.
    chroma_bins: Vec<(usize, u8)>,
    bass_bins: Vec<(usize, u8)>,
    onset_bins: usize,
    prev_log: Vec<f32>,
    /// Pending samples; `pending[0]` is sample `base` of the stream (negative: zero padding
    /// that centres the first frames on the first sample).
    pending: Vec<f32>,
    base: i64,
    /// Samples pushed so far.
    pushed: u64,
    /// Sum of squares of everything pushed (for "was anything captured at all").
    energy: f64,
    next_onset: usize,
    features: Features,
}

impl Extractor {
    pub fn new(sample_rate: f64) -> Self {
        let sample_rate = if sample_rate.is_finite() {
            sample_rate.clamp(8_000.0, 192_000.0)
        } else {
            48_000.0
        };
        // ~23 ms for onsets (time resolution), ~170 ms for chroma (semitone resolution).
        let onset_size = ((sample_rate * 0.023) as usize).next_power_of_two();
        let chroma_size = ((sample_rate * 0.17) as usize).next_power_of_two();
        let mut planner = FftPlanner::new();
        let onset = Stft::new(&mut planner, onset_size);
        let chroma = Stft::new(&mut planner, chroma_size);
        // ~340 ms for the bass: its window is the longest, so it sets the padding below.
        let bass_size = 2 * chroma_size;
        let bass = Stft::new(&mut planner, bass_size);

        let bins = |size: usize, band: std::ops::RangeInclusive<f64>| -> Vec<(usize, u8)> {
            let bin_hz = sample_rate / size as f64;
            (1..size / 2)
                .filter_map(|bin| {
                    let hz = bin as f64 * bin_hz;
                    if !band.contains(&hz) {
                        return None;
                    }
                    let midi = 69.0 + 12.0 * (hz / 440.0).log2();
                    Some((bin, (midi.round() as i64).rem_euclid(12) as u8))
                })
                .collect()
        };
        let chroma_bins = bins(chroma_size, CHROMA_MIN_HZ..=CHROMA_MAX_HZ);
        let bass_bins = bins(bass_size, BASS_MIN_HZ..=BASS_MAX_HZ);
        let onset_bins =
            (((ONSET_MAX_HZ / (sample_rate / onset_size as f64)) as usize) + 1).min(onset_size / 2);

        let pad = bass_size / 2;
        Self {
            sample_rate,
            onset,
            chroma,
            bass,
            chroma_bins,
            bass_bins,
            onset_bins,
            prev_log: vec![0.0; onset_bins],
            pending: vec![0.0; pad],
            base: -(pad as i64),
            pushed: 0,
            energy: 0.0,
            next_onset: 0,
            features: Features::default(),
        }
    }

    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    pub fn duration_secs(&self) -> f64 {
        self.pushed as f64 / self.sample_rate
    }

    /// Root mean square of everything pushed.
    pub fn rms(&self) -> f64 {
        if self.pushed == 0 {
            0.0
        } else {
            (self.energy / self.pushed as f64).sqrt()
        }
    }

    pub fn features(&self) -> &Features {
        &self.features
    }

    /// Append samples, emitting every frame they complete.
    pub fn push(&mut self, samples: &[f32]) {
        for &s in samples {
            let s = if s.is_finite() { s } else { 0.0 };
            self.energy += f64::from(s) * f64::from(s);
            self.pending.push(s);
        }
        self.pushed += samples.len() as u64;
        self.drain();
    }

    /// Centre sample of onset frame `k`.
    fn centre(&self, k: usize) -> i64 {
        (k as f64 * self.sample_rate / ONSET_RATE).round() as i64
    }

    fn drain(&mut self) {
        let end = self.base + self.pending.len() as i64;
        loop {
            let k = self.next_onset;
            let centre = self.centre(k);
            let is_chroma = k.is_multiple_of(ONSETS_PER_CHROMA);
            // The bass window is the longest one: wait for it on chroma frames so all the
            // features stay in step.
            let half = if is_chroma {
                self.bass.size / 2
            } else {
                self.onset.size / 2
            } as i64;
            if centre + half > end {
                break;
            }
            let at = |start: i64| (start - self.base) as usize;

            let start = at(centre - (self.onset.size / 2) as i64);
            self.onset
                .run(&self.pending[start..start + self.onset.size]);
            let mut flux = 0.0f32;
            for (prev, bin) in self
                .prev_log
                .iter_mut()
                .zip(&self.onset.buf[1..=self.onset_bins])
            {
                let log = (ONSET_COMPRESSION * bin.norm()).ln_1p();
                flux += (log - *prev).max(0.0);
                *prev = log;
            }
            self.features.onset.push(flux);

            if is_chroma {
                let start = at(centre - (self.chroma.size / 2) as i64);
                self.chroma
                    .run(&self.pending[start..start + self.chroma.size]);
                let mut frame = [0.0f32; 12];
                for &(bin, pc) in &self.chroma_bins {
                    frame[usize::from(pc)] += self.chroma.buf[bin].norm_sqr();
                }
                self.features.chroma.push(frame);

                let start = at(centre - (self.bass.size / 2) as i64);
                self.bass.run(&self.pending[start..start + self.bass.size]);
                let mut frame = [0.0f32; 12];
                for &(bin, pc) in &self.bass_bins {
                    frame[usize::from(pc)] += self.bass.buf[bin].norm_sqr();
                }
                self.features.bass.push(frame);
            }
            self.next_onset += 1;
        }

        // Drop samples no later frame needs (in big steps: draining is a memmove).
        let keep_from = self.centre(self.next_onset) - (self.bass.size / 2) as i64;
        let stale = keep_from - self.base;
        if stale > 65_536 {
            self.pending.drain(..stale as usize);
            self.base = keep_from;
        }
    }
}
